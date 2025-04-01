use std::fmt::Display;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Errno(i32);

impl From<Errno> for i32 {
    fn from(value: Errno) -> Self {
        value.0
    }
}

impl Errno {
    pub const SUCCESS: Self = Self(0);
    pub const EPERM: Self = Self(1);
    pub const ENOENT: Self = Self(2);
    pub const ESRCH: Self = Self(3);
    pub const EINTR: Self = Self(4);
    pub const EIO: Self = Self(5);
    pub const ENXIO: Self = Self(6);
    pub const E2BIG: Self = Self(7);
    pub const ENOEXEC: Self = Self(8);
    pub const EBADF: Self = Self(9);
    pub const ECHILD: Self = Self(10);
    pub const EAGAIN: Self = Self(11);
    pub const ENOMEM: Self = Self(12);
    pub const EACCES: Self = Self(13);
    pub const EFAULT: Self = Self(14);
    pub const ENOTBLK: Self = Self(15);
    pub const EBUSY: Self = Self(16);
    pub const EEXIST: Self = Self(17);
    pub const EXDEV: Self = Self(18);
    pub const ENODEV: Self = Self(19);
    pub const ENOTDIR: Self = Self(20);
    pub const EISDIR: Self = Self(21);
    pub const EINVAL: Self = Self(22);
    pub const ENFILE: Self = Self(23);
    pub const EMFILE: Self = Self(24);
    pub const ENOTTY: Self = Self(25);
    pub const ETXTBSY: Self = Self(26);
    pub const EFBIG: Self = Self(27);
    pub const ENOSPC: Self = Self(28);
    pub const ESPIPE: Self = Self(29);
    pub const EROFS: Self = Self(30);
    pub const EMLINK: Self = Self(31);
    pub const EPIPE: Self = Self(32);
    pub const EDOM: Self = Self(33);
    pub const ERANGE: Self = Self(34);
    pub const EDEADLK: Self = Self(35);
    pub const ENAMETOOLONG: Self = Self(36);
    pub const ENOLCK: Self = Self(37);
    pub const ENOSYS: Self = Self(38);
    pub const ENOTEMPTY: Self = Self(39);
    pub const ELOOP: Self = Self(40);
    pub const ENOMSG: Self = Self(42);
    pub const EIDRM: Self = Self(43);
    pub const ECHRNG: Self = Self(44);
    pub const EL2NSYNC: Self = Self(45);
    pub const EL3HLT: Self = Self(46);
    pub const EL3RST: Self = Self(47);
    pub const ELNRNG: Self = Self(48);
    pub const EUNATCH: Self = Self(49);
    pub const ENOCSI: Self = Self(50);
    pub const EL2HLT: Self = Self(51);
    pub const EBADE: Self = Self(52);
    pub const EBADR: Self = Self(53);
    pub const EXFULL: Self = Self(54);
    pub const ENOANO: Self = Self(55);
    pub const EBADRQC: Self = Self(56);
    pub const EBADSLT: Self = Self(57);
    pub const EMULTIHOP: Self = Self(72);
    pub const EOVERFLOW: Self = Self(75);
    pub const ENOTUNIQ: Self = Self(76);
    pub const EBADFD: Self = Self(77);
    pub const EBADMSG: Self = Self(74);
    pub const EREMCHG: Self = Self(78);
    pub const ELIBACC: Self = Self(79);
    pub const ELIBBAD: Self = Self(80);
    pub const ELIBSCN: Self = Self(81);
    pub const ELIBMAX: Self = Self(82);
    pub const ELIBEXEC: Self = Self(83);
    pub const EILSEQ: Self = Self(84);
    pub const ERESTART: Self = Self(85);
    pub const ESTRPIPE: Self = Self(86);
    pub const EUSERS: Self = Self(87);
    pub const ENOTSOCK: Self = Self(88);
    pub const EDESTADDRREQ: Self = Self(89);
    pub const EMSGSIZE: Self = Self(90);
    pub const EPROTOTYPE: Self = Self(91);
    pub const ENOPROTOOPT: Self = Self(92);
    pub const EPROTONOSUPPORT: Self = Self(93);
    pub const ESOCKTNOSUPPORT: Self = Self(94);
    pub const EOPNOTSUPP: Self = Self(95);
    pub const EPFNOSUPPORT: Self = Self(96);
    pub const EAFNOSUPPORT: Self = Self(97);
    pub const EADDRINUSE: Self = Self(98);
    pub const EADDRNOTAVAIL: Self = Self(99);
    pub const ENETDOWN: Self = Self(100);
    pub const ENETUNREACH: Self = Self(101);
    pub const ENETRESET: Self = Self(102);
    pub const ECONNABORTED: Self = Self(103);
    pub const ECONNRESET: Self = Self(104);
    pub const ENOBUFS: Self = Self(105);
    pub const EISCONN: Self = Self(106);
    pub const ENOTCONN: Self = Self(107);
    pub const ESHUTDOWN: Self = Self(108);
    pub const ETOOMANYREFS: Self = Self(109);
    pub const ETIMEDOUT: Self = Self(110);
    pub const ECONNREFUSED: Self = Self(111);
    pub const EHOSTDOWN: Self = Self(112);
    pub const EHOSTUNREACH: Self = Self(113);
    pub const EALREADY: Self = Self(114);
    pub const EINPROGRESS: Self = Self(115);
    pub const ESTALE: Self = Self(116);
    pub const EDQUOT: Self = Self(122);
    pub const ENOMEDIUM: Self = Self(123);
    pub const EMEDIUMTYPE: Self = Self(124);
    pub const ECANCELED: Self = Self(125);
    pub const ENOKEY: Self = Self(126);
    pub const EKEYEXPIRED: Self = Self(127);
    pub const EKEYREVOKED: Self = Self(128);
    pub const EKEYREJECTED: Self = Self(129);
    pub const EOWNERDEAD: Self = Self(130);
    pub const ENOTRECOVERABLE: Self = Self(131);
    pub const EHWPOISON: Self = Self(133);
    pub const ERFKILL: Self = Self(132);

    #[inline]
    pub fn from_raw(errno: i32) -> Self {
        Self(errno)
    }

    // Assigns the given errno value to the thread
    #[inline]
    pub fn set_errno(&self) {
        unsafe {
            *libc::__errno_location() = self.0;
        }
    }

    pub fn get_errno() -> Self {
        Self(unsafe { *libc::__errno_location() })
    }
}

impl Display for Errno {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::EPERM => f.write_str("EPERM"),
            Self::ENOENT => f.write_str("ENOENT"),
            Self::ESRCH => f.write_str("ESRCH"),
            Self::EINTR => f.write_str("EINTR"),
            Self::EIO => f.write_str("EIO"),
            Self::ENXIO => f.write_str("ENXIO"),
            Self::E2BIG => f.write_str("E2BIG"),
            Self::ENOEXEC => f.write_str("ENOEXEC"),
            Self::EBADF => f.write_str("EBADF"),
            Self::ECHILD => f.write_str("ECHILD"),
            Self::EAGAIN => f.write_str("EAGAIN"),
            Self::ENOMEM => f.write_str("ENOMEM"),
            Self::EACCES => f.write_str("EACCES"),
            Self::EFAULT => f.write_str("EFAULT"),
            Self::ENOTBLK => f.write_str("ENOTBLK"),
            Self::EBUSY => f.write_str("EBUSY"),
            Self::EEXIST => f.write_str("EEXIST"),
            Self::EXDEV => f.write_str("EXDEV"),
            Self::ENODEV => f.write_str("ENODEV"),
            Self::ENOTDIR => f.write_str("ENOTDIR"),
            Self::EISDIR => f.write_str("EISDIR"),
            Self::EINVAL => f.write_str("EINVAL"),
            Self::ENFILE => f.write_str("ENFILE"),
            Self::EMFILE => f.write_str("EMFILE"),
            Self::ENOTTY => f.write_str("ENOTTY"),
            Self::ETXTBSY => f.write_str("ETXTBSY"),
            Self::EFBIG => f.write_str("EFBIG"),
            Self::ENOSPC => f.write_str("ENOSPC"),
            Self::ESPIPE => f.write_str("ESPIPE"),
            Self::EROFS => f.write_str("EROFS"),
            Self::EMLINK => f.write_str("EMLINK"),
            Self::EPIPE => f.write_str("EPIPE"),
            Self::EDOM => f.write_str("EDOM"),
            Self::ERANGE => f.write_str("ERANGE"),
            Self::EDEADLK => f.write_str("EDEADLK"),
            Self::ENAMETOOLONG => f.write_str("ENAMETOOLONG"),
            Self::ENOLCK => f.write_str("ENOLCK"),
            Self::ENOSYS => f.write_str("ENOSYS"),
            Self::ENOTEMPTY => f.write_str("ENOTEMPTY"),
            Self::ELOOP => f.write_str("ELOOP"),
            Self::ENOMSG => f.write_str("ENOMSG"),
            Self::EIDRM => f.write_str("EIDRM"),
            Self::ECHRNG => f.write_str("ECHRNG"),
            Self::EL2NSYNC => f.write_str("EL2NSYNC"),
            Self::EL3HLT => f.write_str("EL3HLT"),
            Self::EL3RST => f.write_str("EL3RST"),
            Self::ELNRNG => f.write_str("ELNRNG"),
            Self::EUNATCH => f.write_str("EUNATCH"),
            Self::ENOCSI => f.write_str("ENOCSI"),
            Self::EL2HLT => f.write_str("EL2HLT"),
            Self::EBADE => f.write_str("EBADE"),
            Self::EBADR => f.write_str("EBADR"),
            Self::EXFULL => f.write_str("EXFULL"),
            Self::ENOANO => f.write_str("ENOANO"),
            Self::EBADRQC => f.write_str("EBADRQC"),
            Self::EBADSLT => f.write_str("EBADSLT"),
            Self::EMULTIHOP => f.write_str("EMULTIHOP"),
            Self::EOVERFLOW => f.write_str("EOVERFLOW"),
            Self::ENOTUNIQ => f.write_str("ENOTUNIQ"),
            Self::EBADFD => f.write_str("EBADFD"),
            Self::EBADMSG => f.write_str("EBADMSG"),
            Self::EREMCHG => f.write_str("EREMCHG"),
            Self::ELIBACC => f.write_str("ELIBACC"),
            Self::ELIBBAD => f.write_str("ELIBBAD"),
            Self::ELIBSCN => f.write_str("ELIBSCN"),
            Self::ELIBMAX => f.write_str("ELIBMAX"),
            Self::ELIBEXEC => f.write_str("ELIBEXEC"),
            Self::EILSEQ => f.write_str("EILSEQ"),
            Self::ERESTART => f.write_str("ERESTART"),
            Self::ESTRPIPE => f.write_str("ESTRPIPE"),
            Self::EUSERS => f.write_str("EUSERS"),
            Self::ENOTSOCK => f.write_str("ENOTSOCK"),
            Self::EDESTADDRREQ => f.write_str("EDESTADDRREQ"),
            Self::EMSGSIZE => f.write_str("EMSGSIZE"),
            Self::EPROTOTYPE => f.write_str("EPROTOTYPE"),
            Self::ENOPROTOOPT => f.write_str("ENOPROTOOPT"),
            Self::EPROTONOSUPPORT => f.write_str("EPROTONOSUPPORT"),
            Self::ESOCKTNOSUPPORT => f.write_str("ESOCKTNOSUPPORT"),
            Self::EOPNOTSUPP => f.write_str("EOPNOTSUPP"),
            Self::EPFNOSUPPORT => f.write_str("EPFNOSUPPORT"),
            Self::EAFNOSUPPORT => f.write_str("EAFNOSUPPORT"),
            Self::EADDRINUSE => f.write_str("EADDRINUSE"),
            Self::EADDRNOTAVAIL => f.write_str("EADDRNOTAVAIL"),
            Self::ENETDOWN => f.write_str("ENETDOWN"),
            Self::ENETUNREACH => f.write_str("ENETUNREACH"),
            Self::ENETRESET => f.write_str("ENETRESET"),
            Self::ECONNABORTED => f.write_str("ECONNABORTED"),
            Self::ECONNRESET => f.write_str("ECONNRESET"),
            Self::ENOBUFS => f.write_str("ENOBUFS"),
            Self::EISCONN => f.write_str("EISCONN"),
            Self::ENOTCONN => f.write_str("ENOTCONN"),
            Self::ESHUTDOWN => f.write_str("ESHUTDOWN"),
            Self::ETOOMANYREFS => f.write_str("ETOOMANYREFS"),
            Self::ETIMEDOUT => f.write_str("ETIMEDOUT"),
            Self::ECONNREFUSED => f.write_str("ECONNREFUSED"),
            Self::EHOSTDOWN => f.write_str("EHOSTDOWN"),
            Self::EHOSTUNREACH => f.write_str("EHOSTUNREACH"),
            Self::EALREADY => f.write_str("EALREADY"),
            Self::EINPROGRESS => f.write_str("EINPROGRESS"),
            Self::ESTALE => f.write_str("ESTALE"),
            Self::EDQUOT => f.write_str("EDQUOT"),
            Self::ENOMEDIUM => f.write_str("ENOMEDIUM"),
            Self::EMEDIUMTYPE => f.write_str("EMEDIUMTYPE"),
            Self::ECANCELED => f.write_str("ECANCELED"),
            Self::ENOKEY => f.write_str("ENOKEY"),
            Self::EKEYEXPIRED => f.write_str("EKEYEXPIRED"),
            Self::EKEYREVOKED => f.write_str("EKEYREVOKED"),
            Self::EKEYREJECTED => f.write_str("EKEYREJECTED"),
            Self::EOWNERDEAD => f.write_str("EOWNERDEAD"),
            Self::ENOTRECOVERABLE => f.write_str("ENOTRECOVERABLE"),
            Self::EHWPOISON => f.write_str("EHWPOISON"),
            Self::ERFKILL => f.write_str("ERFKILL"),
            i => f.write_fmt(format_args!("errno {}", i.0)),
        }
    }
}
