use std::cell::UnsafeCell;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};
use std::{array, cmp, ptr};

/// A set of values that can be indexed into by a key of type `K`.
///
pub struct KeyedArena<K: ArenaKey<Value = V>, V: Sized, const N: usize> {
    inner: [UnsafeCell<ArenaItem<V>>; N],
    next_key: usize,
    max_key: usize,
    _phantom: PhantomData<K>,
}

impl<K: ArenaKey<Value = V>, V: Sized + Debug, const N: usize> Debug for KeyedArena<K, V, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut list = f.debug_list();

        for i in 0..=self.max_key {
            unsafe {
                if (*(self.inner[i].get() as *const ArenaItem<V>)).ref_cnt > 0 {
                    list.entry(&(
                        i,
                        (*(self.inner[i].get() as *const ArenaItem<V>))
                            .value
                            .assume_init_ref(),
                    ));
                }
            }
        }

        list.finish()
    }
}

impl<K: ArenaKey<Value = V>, V: Sized, const N: usize> KeyedArena<K, V, N> {
    pub fn new() -> Self {
        Self {
            inner: array::from_fn(|_| {
                UnsafeCell::new(ArenaItem {
                    value: MaybeUninit::uninit(),
                    ref_cnt: 0,
                })
            }),
            max_key: 0usize,
            next_key: 0usize,
            _phantom: Default::default(),
        }
    }

    pub fn keys(&self) -> ArenaKeyIter<'_, K, V, N> {
        ArenaKeyIter::new(self)
    }

    pub fn values(&self) -> ArenaValueIter<'_, K, V, N> {
        ArenaValueIter::new(self)
    }

    pub fn values_mut(&mut self) -> ArenaValueIterMut<'_, K, V, N> {
        ArenaValueIterMut::new(self)
    }

    /// Initializes the given ValueIndex's contents in-place.
    ///
    /// # Safety
    ///
    /// The caller of this method must ensure that `value_idx` points to a correctly allocated
    /// ValueIndex.
    pub unsafe fn initialize(value_idx: *mut KeyedArena<K, V, N>) {
        for i in 0..N {
            (*(ptr::addr_of_mut!((*value_idx).inner) as *mut ArenaItem<V>).add(i)).ref_cnt = 0;
        }
        *ptr::addr_of_mut!((*value_idx).next_key) = 0;
        *ptr::addr_of_mut!((*value_idx).max_key) = 0;
        *ptr::addr_of_mut!((*value_idx)._phantom) = Default::default();
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        let key: usize = (*key).to_usize();
        if key >= self.inner.len() {
            return None;
        }
        unsafe {
            let item = &*(self.inner[key].get() as *const ArenaItem<V>);
            if item.ref_cnt == 0 {
                None
            } else {
                Some(item.value.assume_init_ref())
            }
        }
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let key: usize = (*key).to_usize();
        if key >= self.inner.len() {
            return None;
        }
        unsafe {
            let item = self.inner[key].get_mut();
            if item.ref_cnt == 0 {
                None
            } else {
                Some(item.value.assume_init_mut())
            }
        }
    }

    pub fn allocate_with_key(&mut self, key: K, value: V) -> Result<(), KeyedArenaError> {
        let idx: usize = key.to_usize();
        if idx >= self.inner.len() {
            return Err(KeyedArenaError::OutOfRange);
        }

        self.max_key = cmp::max(self.max_key, idx);

        let item = self.inner[idx].get_mut();
        if item.ref_cnt == 0 {
            item.ref_cnt = 1;
            item.value.write(value);
            Ok(())
        } else {
            Err(KeyedArenaError::KeyError)
        }
    }

    pub fn allocate(&mut self, value: V) -> Option<Rc<K>> {
        let idx = self.next_key()?;
        self.max_key = cmp::max(self.max_key, idx);

        let item = self.inner[idx].get_mut();
        debug_assert!(item.ref_cnt == 0);
        item.ref_cnt = 1;
        item.value.write(value);

        Some(Rc {
            key: K::from_usize(idx),
            ptr: ptr::addr_of!(self.inner[idx]),
        })
    }

    pub fn ref_count(&self, key: &K) -> usize {
        let idx: usize = (*key).to_usize();
        assert!(idx < self.inner.len());

        unsafe { (*(self.inner[idx].get() as *const ArenaItem<V>)).ref_cnt as usize }
    }

    pub fn upref(&mut self, key: &K) {
        let idx: usize = (*key).to_usize();
        assert!(idx < self.inner.len());

        let item = self.inner[idx].get_mut();
        assert!(item.ref_cnt > 0);
        item.ref_cnt += 1;
    }

    /// Decrements the reference count for the given key, returning the new reference count.
    pub fn downref(&mut self, key: &K) -> usize {
        let idx: usize = (*key).to_usize();
        assert!(idx < self.inner.len());

        let item = self.inner[idx].get_mut();
        assert!(item.ref_cnt > 0);
        item.ref_cnt -= 1;
        if item.ref_cnt == 0 {
            unsafe {
                drop(item.value.assume_init_read());
            }
        }

        item.ref_cnt as usize
    }

    /// Indicates whether the arena area for the given key is occupied.
    pub fn is_occupied(&mut self, key: &K) -> bool {
        let idx: usize = (*key).to_usize();
        assert!(idx < self.inner.len());

        let item = self.inner[idx].get_mut();
        item.ref_cnt > 0
    }

    /// Retrieves the next available key from the value index.
    ///
    /// This algorithm has an average temporal complexity of O(N/K), where N is the number of
    /// places in the ValueIndex and K is the average number of available slots at the time a key
    /// needs to be procured over many trials. So, assuming you only use half of the available
    /// capacity of your Value Index, the average key procurement time should be N / (N / 2) = 2
    /// indexes.
    fn next_key(&mut self) -> Option<usize> {
        let mut curr_key = self.next_key;
        while self.inner[curr_key].get_mut().ref_cnt > 0 {
            curr_key = (curr_key + 1) % N;
            if curr_key == self.next_key {
                return None; // All keys are exhausted
            }
        }
        self.next_key = (curr_key + 1) % N;
        Some(curr_key)
    }
}

impl<K: ArenaKey<Value = V>, V: Sized, const N: usize> Default for KeyedArena<K, V, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: ArenaKey<Value = V>, V: Sized + Clone, const N: usize> Clone for KeyedArena<K, V, N> {
    fn clone(&self) -> Self {
        Self {
            inner: array::from_fn(|i| unsafe {
                UnsafeCell::new((*(self.inner[i].get() as *const ArenaItem<V>)).clone())
            }),
            max_key: self.max_key,
            next_key: self.next_key,
            _phantom: self._phantom,
        }
    }
}

struct ArenaItem<V: Sized> {
    value: MaybeUninit<V>,
    ref_cnt: u16,
}

impl<V: Sized + Clone> Clone for ArenaItem<V> {
    fn clone(&self) -> Self {
        let value = if self.ref_cnt > 0 {
            unsafe { MaybeUninit::new(self.value.assume_init_ref().clone()) }
        } else {
            MaybeUninit::uninit()
        };

        Self {
            value,
            ref_cnt: self.ref_cnt,
        }
    }
}

pub struct ArenaKeyIter<'a, K: ArenaKey<Value = V>, V: Sized, const N: usize> {
    arena: &'a KeyedArena<K, V, N>,
    next_key: Option<K>,
}

impl<'a, K: ArenaKey<Value = V>, V: Sized, const N: usize> ArenaKeyIter<'a, K, V, N> {
    fn new(arena: &'a KeyedArena<K, V, N>) -> Self {
        let mut next_key = None;
        for i in 0..=arena.max_key {
            if unsafe { (*(arena.inner[i].get() as *const ArenaItem<V>)).ref_cnt } > 0 {
                next_key = Some(K::from_usize(i));
                break;
            }
        }

        Self { arena, next_key }
    }
}

impl<'a, K: ArenaKey<Value = V>, V: Sized, const N: usize> Iterator for ArenaKeyIter<'a, K, V, N> {
    type Item = K;

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.next_key?;
        let raw_key: usize = key.to_usize();

        self.next_key = None;
        if raw_key + 1 <= self.arena.max_key {
            for i in raw_key + 1..=self.arena.max_key {
                if unsafe { (*(self.arena.inner[i].get() as *const ArenaItem<V>)).ref_cnt } > 0 {
                    self.next_key = Some(K::from_usize(i));
                    break
                }
            }
        }

        Some(key)
    }
}

pub struct ArenaValueIter<'a, K: ArenaKey<Value = V>, V: Sized, const N: usize> {
    arena: &'a KeyedArena<K, V, N>,
    next_key: Option<K>,
}

impl<'a, K: ArenaKey<Value = V>, V: Sized, const N: usize> ArenaValueIter<'a, K, V, N> {
    fn new(arena: &'a KeyedArena<K, V, N>) -> Self {
        let mut next_key = None;
        for i in 0..=arena.max_key {
            if unsafe { (*(arena.inner[i].get() as *const ArenaItem<V>)).ref_cnt } > 0 {
                next_key = Some(K::from_usize(i));
                break;
            }
        }

        Self { arena, next_key }
    }
}

impl<'a, K: ArenaKey<Value = V>, V: Sized, const N: usize> Iterator
    for ArenaValueIter<'a, K, V, N>
{
    type Item = &'a V;

    fn next(&mut self) -> Option<Self::Item> {
        let key: usize = self.next_key?.to_usize();
        let value = unsafe {
            (*(self.arena.inner[key].get() as *const ArenaItem<V>))
                .value
                .assume_init_ref()
        };

        self.next_key = None;
        if key + 1 <= self.arena.max_key {
            for i in key + 1..=self.arena.max_key {
                if unsafe { (*(self.arena.inner[i].get() as *const ArenaItem<V>)).ref_cnt } > 0 {
                    self.next_key = Some(K::from_usize(i));
                }
            }
        }

        Some(value)
    }
}

pub struct ArenaValueIterMut<'a, K: ArenaKey<Value = V>, V: Sized, const N: usize> {
    arena: &'a mut KeyedArena<K, V, N>,
    next_key: Option<K>,
}

impl<'a, K: ArenaKey<Value = V>, V: Sized, const N: usize> ArenaValueIterMut<'a, K, V, N> {
    fn new(arena: &'a mut KeyedArena<K, V, N>) -> Self {
        let mut next_key = None;
        for i in 0..=arena.max_key {
            if unsafe { (*(arena.inner[i].get() as *const ArenaItem<V>)).ref_cnt } > 0 {
                next_key = Some(K::from_usize(i));
                break;
            }
        }

        Self { arena, next_key }
    }
}

impl<'a, K: ArenaKey<Value = V>, V: Sized, const N: usize> Iterator
    for ArenaValueIterMut<'a, K, V, N>
{
    type Item = &'a mut V;

    fn next(&mut self) -> Option<Self::Item> {
        let key: usize = self.next_key?.to_usize();

        self.next_key = None;
        if key + 1 <= self.arena.max_key {
            for i in key + 1..=self.arena.max_key {
                if unsafe { (*(self.arena.inner[i].get() as *const ArenaItem<V>)).ref_cnt } > 0 {
                    self.next_key = Some(K::from_usize(i));
                }
            }
        }

        let value = unsafe { (*self.arena.inner[key].get()).value.assume_init_mut() };

        Some(value)
    }
}

pub trait ArenaKey:
    Clone + Copy + Debug + PartialEq + Eq + Hash + Sized + private::InnerUsize
{
    type Value: Sized + 'static;
}

impl<T: Sized> Default for ArenaItem<T> {
    fn default() -> Self {
        Self {
            value: MaybeUninit::uninit(),
            ref_cnt: 0,
        }
    }
}

pub struct Rc<K: ArenaKey + 'static> {
    key: K,
    ptr: *const UnsafeCell<ArenaItem<K::Value>>,
}

impl<K: ArenaKey + 'static> Rc<K> {
    pub fn upref(rc: &mut Self) {
        unsafe {
            let item = &mut (*(*rc.ptr).get());
            assert!((*(*rc.ptr).get()).ref_cnt > 0);
            item.ref_cnt
                .checked_add(1)
                .expect("KeyedArena Rc exceeded max number of references");
        }
    }
}

impl<K: ArenaKey> Deref for Rc<K> {
    type Target = K;

    fn deref(&self) -> &Self::Target {
        &self.key
    }
}

impl<K: ArenaKey> DerefMut for Rc<K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.key
    }
}

impl<K: ArenaKey> Drop for Rc<K> {
    fn drop(&mut self) {
        unsafe {
            let item = &mut (*(*self.ptr).get());
            assert!((*(*self.ptr).get()).ref_cnt > 0);
            item.ref_cnt -= 1;
            if item.ref_cnt == 0 {
                drop(item.value.assume_init_read());
            }
        }
    }
}

impl<K: ArenaKey> Clone for Rc<K> {
    fn clone(&self) -> Self {
        unsafe {
            if !(*(*self.ptr).get()).ref_cnt > 0 {
                panic!("ref_cnt <= 0");
            }
            assert!((*(*self.ptr).get()).ref_cnt < u16::MAX);
            (*(*self.ptr).get())
                .ref_cnt
                .checked_add(1)
                .expect("KeyedArena Rc exceeded max number of references");
        }
        Self {
            key: self.key,
            ptr: self.ptr,
        }
    }
}

impl<K: ArenaKey> Debug for Rc<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO: fix this
        f.debug_struct("StaticRc").field("key", &self.key).finish()
    }
}

impl<K: ArenaKey> PartialEq for Rc<K> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key // If they point to the same data, they are the same
    }
}

impl<K: ArenaKey> Eq for Rc<K> {}

impl<K: ArenaKey> Hash for Rc<K> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyedArenaError {
    NoSpace,
    OutOfRange,
    /// The keyed entry was in an unexpected state.
    KeyError,
    /// The reference counter reached its maximum count.
    TooManyRefs,
}

pub(crate) mod private {
    use crate::handlers::buffer::BufferId;
    use crate::handlers::descriptor::Descriptor;
    use crate::handlers::directory::DirectoryId;
    use crate::handlers::epoll::EpollId;
    use crate::handlers::eventfd::EventfdId;
    use crate::handlers::file::{CowId, FileId, OpenFileId};
    use crate::handlers::fuzz_endpoint::FuzzEndpointId;
    use crate::handlers::mq::MqId;
    use crate::handlers::pipe::PipeId;
    use crate::handlers::plugin::PluginEndpointId;
    use crate::handlers::semaphore::SemaphoreId;
    use crate::handlers::socket::SocketId;
    use std::mem;

    /// Indicates that the given type is implemented as a `usize` value and provides conversion
    /// methods to/from `usize`.
    pub trait InnerUsize {
        /// Converts the given value to `usize`.
        fn to_usize(&self) -> usize;

        /// Constructs a new instance from the given `usize`.
        fn from_usize(val: usize) -> Self;
    }

    // We use `transmute` here so that `InnerUsize` can be private outside of this module.

    impl InnerUsize for BufferId {
        fn to_usize(&self) -> usize {
            // SAFETY: `BufferId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `BufferId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for CowId {
        fn to_usize(&self) -> usize {
            // SAFETY: `CowId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `CowId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for Descriptor {
        fn to_usize(&self) -> usize {
            // SAFETY: `DescriptorId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `DescriptorId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for DirectoryId {
        fn to_usize(&self) -> usize {
            // SAFETY: `DirectoryId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `DirectoryId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for EpollId {
        fn to_usize(&self) -> usize {
            // SAFETY: `EpollId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `EpollId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for EventfdId {
        fn to_usize(&self) -> usize {
            // SAFETY: `EventfdId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `EventfdId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for FileId {
        fn to_usize(&self) -> usize {
            // SAFETY: `FileId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `FileId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for OpenFileId {
        fn to_usize(&self) -> usize {
            // SAFETY: `FileId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `FileId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for FuzzEndpointId {
        fn to_usize(&self) -> usize {
            // SAFETY: `FuzzEndpointId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `FuzzEndpointId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for MqId {
        fn to_usize(&self) -> usize {
            // SAFETY: `MessageQueueId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `MessageQueueId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for PipeId {
        fn to_usize(&self) -> usize {
            // SAFETY: `PipeId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `PipeId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for PluginEndpointId {
        fn to_usize(&self) -> usize {
            // SAFETY: `PluginEndpointId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `PluginEndpointId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for SemaphoreId {
        fn to_usize(&self) -> usize {
            // SAFETY: `SemaphoreId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `SemaphoreId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }

    impl InnerUsize for SocketId {
        fn to_usize(&self) -> usize {
            // SAFETY: `SocketId` is a repr(transparent) usize
            unsafe { mem::transmute_copy(self) }
        }

        fn from_usize(val: usize) -> Self {
            // SAFETY: `SocketId` is a repr(transparent) usize
            unsafe { mem::transmute(val) }
        }
    }
}
