
#[repr(C)]
#[allow(non_camel_case_types)]
struct sctp_getaddrs_old {
    assoc_id: libc::sctp_assoc_t,
    addr_num: libc::c_int,
    addrs: *const libc::sockaddr,
}

#[repr(C)]
#[allow(non_camel_case_types)]
struct sctp_getaddrs {
    assoc_id: libc::sctp_assoc_t,
    addr_num: libc::__u32,
    addrs: *const libc::__u8,
}



