//! Fixed-size data structures suitable for use in a shared memory context.
//! 

use std::array;
use std::hash::{Hash, Hasher};
use std::mem;
use std::mem::MaybeUninit;
use std::sync::atomic::AtomicBool;
use std::thread::ThreadId;






/// A queue that can contain up to `N` elements of type `T`.
pub struct Queue<T: Sized, const N: usize> {
    pub ringbuf: [MaybeUninit<T>; N],
    pub start: usize,
    pub end: Option<usize>,
}

impl<T: Sized, const N: usize> Queue<T, N> {
    pub fn new() -> Self {
        Self {
            ringbuf: array::from_fn(|_| MaybeUninit::uninit()),
            start: 0,
            end: None,
        }
    }

    /// Adds `value` to the end of the queue.
    /// 
    /// If the queue is full, this method returns back `value` in an error.
    pub fn push(&mut self, value: T) -> Result<(), T> {
        match self.end {
            None => {
                self.ringbuf[self.start] = MaybeUninit::new(value);
                self.end = Some((self.start + 1) % N);
                Ok(())
            }
            Some(e) => if e == self.start {
                Err(value)
            } else {
                self.ringbuf[e] = MaybeUninit::new(value);
                self.end = Some((e + 1) % N);
                Ok(())
            }
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        let Some(end) = self.end else {
            return None
        };

        let mut value = MaybeUninit::uninit();
        mem::swap(&mut value, &mut self.ringbuf[self.start] );
        self.start = (self.start + 1) % N;
        if self.start == end {
            self.end = None;
        }

        Some(unsafe { value.assume_init() })
    }

    pub fn is_empty(&self) -> bool {
        self.end.is_none()
    }

    pub fn is_full(&self) -> bool {
        if let Some(end) = self.end {
            self.start == end
        } else {
            false
        }
    }
}


/*

/// A hashmap for data where keys are integer values that monotonically increment (e.g. file descriptors).
/// 
/// The key value identifier may be chosen
/// 
/// A value is guaranteed not to move in memory once it has been inserted.
/// 
/// NOTE: this map only works for `N` up to 2^(32) - 1; the topmost two bits are reserved for internal purposes.
pub struct IntegerMap<T: Sized, const N: usize> {
    pub map: [IntegerMapEntry<T>; N],
}

pub struct IntegerMapEntry<T: Sized> {
    key: u32,
    ///
    /// The leftmost bit of this indicates whether the given map is occupied.
    next_link: u32,
    value: MaybeUninit<T>,
}

impl<T: Sized> IntegerMapEntry<T> {
    const MAP_OCCUPIED: u32 = 0x80_00_00_00;
    const HAS_CHILD_LINK: u32 = 0x40_00_00_00;

    fn is_occupied(&self) -> bool {
        self.next_link & Self::MAP_OCCUPIED != 0
    }

    
    fn set_occupied(&mut self, occupied: bool) {
        if occupied {
            self.next_link |= Self::MAP_OCCUPIED;
        } else {
            self.next_link &= !Self::MAP_OCCUPIED;
        }
    }

    fn next_link(&self) -> Option<usize> {
        if self.next_link & Self::HAS_CHILD_LINK == 0 {
            None
        } else {
            Some((self.next_link & 0x3f_ff_ff_ff) as usize)
        }
    }

    fn set_next_link(&mut self, link: usize) {
        self.next_link &= !(Self::MAP_OCCUPIED | Self::HAS_CHILD_LINK);
        self.next_link |= link as u32;
        self.next_link |= Self::HAS_CHILD_LINK;
    }

    fn clear_next_link(&mut self) {
        self.next_link &= !Self::MAP_OCCUPIED;
    }
}

impl<T: Sized, const N: usize> IntegerMap<T, N> {


    pub fn new() -> Self {
        Self {
            map: array::from_fn(|_| IntegerMapEntry {
                key: 0,
                next_link: 0,
                value: MaybeUninit::uninit()
            }),
        }
    }


    /*
    /// Inserts the value
    /// 
    /// This hashmap implementation uses chaining to account for collisions.
    pub fn insert(&mut self, key: u32, value: T) -> Result<Option<T>, T> {
        assert!(key < 0x40_00_00_00);

        // The original bucket the key would hash to
        let mut hash_idx = key as usize % N;
        // The current bucket the key is attempting to be inserted in
        let mut insert_idx = hash_idx;
        // The bucket immediately preceding the current bucket (if currently traversing a chain)
        let mut prev_idx = hash_idx;
 
        // Indicates whether the chain traversal phase of insertion is finished.
        let mut finished_chain = false;

        while self.map[insert_idx].is_occupied() {
            if self.map[insert_idx].key == key { // Key match
                let mut value = MaybeUninit::new(value);
                mem::swap(&mut value, &mut self.map[insert_idx].value);

                return Ok(Some(unsafe { value.assume_init() }))
            }

            if finished_chain { // Search linearly for any available slot
                if insert_idx == hash_idx {
                    return Err(value) // The entire hashmap is full
                }

                if !self.map[insert_idx].is_occupied() {
                    let entry = &mut self.map[insert_idx];

                    // Replace the key at the index
                    entry.key = key;

                    // Replace the value at the index
                    let mut value = MaybeUninit::new(value);
                    mem::swap(&mut value, &mut entry.value);

                    // Set the entry as occupied
                    entry.set_occupied(true);

                    // Update linked list (if applicable)
                    if let Some(link) = self.map[prev_idx].next_link() {
                        self.map[prev_idx].set_next_link(insert_idx);
                        if link == hash_idx {
                            // The link was a loop
                            self.map[insert_idx].set_next_link(link); // What if the insert index already has a next_link?
                            if let Some(insert_link) = self.map[insert_idx].next_link() {
                                let root_insert_link = insert_link;
                                while let Some(next_insert_link) = self.map[insert_link].next_link() {
                                    if root_insert_link == next_insert_link {
                                        self.map[insert_link].set_next_link(hash_idx);
                                        return Ok(None)
                                    }
                                    insert_link = next_insert_link;
                                }
                                self.map[insert_link].set_next_link(hash_idx);
                            }
                        }
                    } else if hash_idx != insert_idx {
                        self.map[prev_idx].set_next_link(insert_idx);
                    }


                    return Ok(Some(unsafe { value.assume_init() }))
                }

                insert_idx = (insert_idx + 1) % N;

            } else if let Some(link) = self.map[insert_idx].next_link() { // keep traversing links
                prev_idx = insert_idx;
                insert_idx = link;

            } else { // no more links--now search for any open slot in the map
                insert_idx = (insert_idx + 1) % N;
            }
        }

        Err(value)       
    } 
    */

    pub fn get(&self) -> Option<&T> {

    }

    pub fn get_mut(&mut self) -> Option<&mut T> {

    }

    pub fn remove(&self, index: u32) -> Option<T> {

    }

}

*/


/*
/// A hashmap for data where keys are integer values that monotonically increment (e.g. file descriptors).
/// 
/// 
/// The key value identifier may be chosen
/// 
/// A value is guaranteed not to move in memory once it has been inserted.
/// 
/// NOTE: this map only works for `N` up to 2^(32) - 1; the topmost two bits are reserved for internal purposes.
pub struct IntegerMap<T: Sized, const N: usize> {
    pub map: [IntegerMapEntry<T>; N],
}

pub struct IntegerMapEntry<T: Sized> {
    key: u32,
    value: MaybeUninit<T>,
}

impl<T: Sized> IntegerMapEntry<T> {
    const MAP_OCCUPIED: u32 = 0x80_00_00_00;

    fn is_occupied(&self) -> bool {
        self.key & Self::MAP_OCCUPIED != 0
    }

    
    fn set_occupied(&mut self, occupied: bool) {
        if occupied {
            self.key |= Self::MAP_OCCUPIED;
        } else {
            self.key &= !Self::MAP_OCCUPIED;
        }
    }
}

impl<T: Sized, const N: usize> IntegerMap<T, N> {
    pub fn new() -> Self {
        Self {
            map: array::from_fn(|_| IntegerMapEntry {
                key: 0,
                value: MaybeUninit::uninit()
            }),
        }
    }

    pub fn insert(&mut self, key: u32, value: T) -> Result<Option<T>, T> {
        assert!(key < 0x40_00_00_00);
        assert!((key as usize) < N);

        if self.map[key as usize].is_occupied() {
            if self.map[key as usize].key == key {
                let mut value = MaybeUninit::new(value);
                mem::swap(&mut value, &mut self.map[key as usize].value);
                return Ok(Some(unsafe { value.assume_init() }))

            } else {
                return Err(value)
            }

        } else {
            let mut value = MaybeUninit::new(value);
            mem::swap(&mut value, &mut self.map[key as usize].value);
            self.map[key as usize].set_occupied(true);
            return Ok(None)
        }
    }

    pub fn contains(&mut self, key: u32) -> bool {
        self.get(key).is_some()
    }

    pub fn remove(&mut self, key: u32) -> Option<T> {
        if self.map[key as usize].is_occupied() {
            let mut value = MaybeUninit::uninit();
            mem::swap(&mut value, &mut self.map[key as usize].value);
            Some(unsafe { value.assume_init() })
        } else {
            None
        }
    }

    pub fn get(&self, key: u32) -> Option<&T> {
        if self.map[key as usize].is_occupied() {
            Some(unsafe { self.map[key as usize].value.assume_init_ref() })
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, key: u32) -> Option<&mut T> {
        if self.map[key as usize].is_occupied() {
            Some(unsafe { self.map[key as usize].value.assume_init_mut() })
        } else {
            None
        }
    }
}
*/





/*
/// A static array that indexes by [`ThreadId`].
/// 
/// Inserted values are thread-safe to access
/// 
/// 
/// NOTE: this map only works for `N` up to 2^(32) - 1; the topmost two bits are reserved for internal purposes.
pub struct ThreadMap<V: Sized, const N: usize> {
    pub map: [Option<V>; N],
}

impl<T: Sized, const N: usize> ThreadMap<T, N> {
    pub fn new() -> Self {
        Self {
            map: array::from_fn(|_| None),
        }
    }

    fn thread_index(key: &ThreadId) -> usize {
        let mut hasher = LinearHasher::new();
        key.hash(&mut hasher);
        hasher.finish() as usize
    }

    pub fn insert(&mut self, key: &ThreadId, value: T) -> Result<(), T> {
        let idx = Self::thread_index(key);

        if idx >= N || self.map[idx].is_some() {
            return Err(value)
        }

        self.map[idx] = Some(value);
        return Ok(())
    }

    pub fn remove(&mut self, key: &ThreadId) -> Option<T> {
        let idx = Self::thread_index(key);
        if idx >= N {
            return None
        }

        let mut value: Option<T> = None;
        mem::swap(&mut value, &mut self.map[idx]);
        value
    }

    pub fn get(&self, key: &ThreadId) -> Option<&T> {
        let idx = Self::thread_index(key);
        if idx >= N {
            return None
        }

        self.map[idx].as_ref()
    }

    pub fn get_mut(&mut self, key: &ThreadId) -> Option<&mut T> {
        let idx = Self::thread_index(key);
        if idx >= N {
            return None
        }

        self.map[idx].as_mut()
    }
}
*/




