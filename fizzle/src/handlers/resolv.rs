use std::{cmp, iter, slice};

use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

use fizzle_plugin::{Context, IoEndpointVariant, StreamId};
use hickory_proto::op::Message;
use hickory_proto::rr::{RData, Record, RecordType};
use hickory_proto::rr::rdata::*;
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use entropic::prelude::*;
use rand_chacha::rand_core::TryRngCore;

#[derive(Debug)]
struct Spf {
    data: Vec<u8>,
}

impl Spf {
    pub fn to_vec(&self) -> Vec<u8> {
        let mut v = Vec::new();

        let mut idx = 0;
        while self.data.len() - idx > 255 {
            v.push(255u8);
            v.extend(&self.data[idx..idx + 255]);
            idx += 255;
        }

        if self.data.len() > 0 {
            v.push((self.data.len() - idx) as u8);
            v.extend(&self.data[idx..]);
        }

        v
    }

    pub fn to_txt(&self) -> Vec<&[u8]> {
        let mut v = Vec::new();

        let mut idx = 0;
        while self.data.len() - idx > 255 {
            v.push(&self.data[idx..idx + 255]);
            idx += 255;
        }

        if self.data.len() > 0 {
            v.push(&self.data[idx..]);
        }

        v
    }
}

const RECORD_QUALIFIERS: &[u8] = &[
    b'+',
    b'-',
    b'~',
    b'?',
];

const RECORD_GRAMMAR: &[&[u8]] = &[
    b"a:",
    b"mx:",
    b"ptr:",
    b"ip4:",
    b"ip6:",
    b"exists:",
    b"all",
    b"exp=",
];

const COMMON: &[&[u8]] = &[
    b"0.0.0.0",
    b"127.0.0.1",
    b"255.255.255.255",
    b"1.1.1.1",
    b"8.8.8.8",
    b"[::1]",
    b"[fe80::b46b:80ff:ce45:112f]",
    b"a.example1.com",
    b"b.example2.com",
    b"c.example3.com",
    b"d.example4.com",
    b"e.example5.com",
];

const MACRO_CHARACTERS: &[u8] = &[
    b's',
    b'l',
    b'o',
    b'd',
    b'i',
    b'p',
    b'v',
    b'h',
    b'c',
    b'r',
    b't',
];

impl Entropic for Spf {
    fn from_entropy_source<'a, I: Iterator<Item = &'a u8>, E: EntropyScheme>(
        source: &mut Source<'a, I, E>,
    ) -> Result<Self, EntropicError> {
        let mut v = Vec::new();
        v.extend(b"v=spf1");

        let b1 = source.get_byte()?;
        let b2 = source.get_byte()?;
        if b1 == 0xce && b2 < 16 {
            return Ok(Spf { data: v })
        } else if b1 == 0xaf && b2 < 16 {
            // Push no starting space
        } else {
            v.push(b' ');
        }
        
        let num_elems = source.get_bounded_len(1..=20)?;
        for _ in 0..num_elems {
            match source.get_uniform_range(0..=7)? {
                idx @ 0..=3 => v.push(RECORD_QUALIFIERS[idx]),
                _ => (),
            }

            match source.get_uniform_range(0..=8)? {
                idx @ 0..=7 => v.extend(RECORD_GRAMMAR[idx]),
                _ if source.get_byte()? & 0x30 > 0 => {
                    // 1 in 64 chance * 1/8 chance to put arbitrary characters
                    let length = source.get_bounded_len(0..=32)?;
                    v.extend(iter::repeat(source.get_byte()?).take(length));
                    v.push(b' ');
                    continue
                }
                _ => continue,
            }

            loop {
                match source.get_uniform_range(0..=7)? {
                    0 => v.push(b'.'),
                    1 => {
                        let idx = source.get_uniform_range(0..=11)?;
                        v.extend(COMMON[idx]);
                        break
                    }
                    2 => {
                        // Macro expansion
                        v.push(b'%');
                        if source.get_byte()? == 0x1f {
                            v.push(source.get_byte()?);
                        }
                        v.push(b'{');
                        let len = source.get_bounded_len(1..=4)?;
                        for _i in 1..=len {
                            match source.get_uniform_range(0..=11)? {
                                idx @  0..=10 => v.push(MACRO_CHARACTERS[idx]),
                                11 => {
                                    let offs = source.get_uniform_range(0..=9)?;
                                    v.push(b'0' + offs);
                                }
                                _ => (),
                            }
                        }

                        v.push(b'}');
                    }
                    3 => {
                        v.extend(b"%-");
                    }
                    4 => {
                        v.extend(b"%_");
                    }
                    5 => {
                        let length = source.get_bounded_len(1..=16)?;
                        // Alphanumeric characters
                        v.extend(iter::repeat(source.get_uniform_ranges(&[0x30..=0x39, 0x41..=0x5A, 0x61..=0x7a])?).take(length));
                    }
                    6 => {
                        // Arbitrary data
                        v.push(source.get_byte()?);
                    }
                    _ => break,
                }
            }
            v.push(b' ');
        }

        Ok(Spf { data: v })
    }

    fn to_entropy_sink<'a, I: Iterator<Item = &'a mut u8>, E: EntropyScheme>(
        &self,
        _sink: &mut Sink<'a, I, E>,
    ) -> Result<usize, EntropicError> {
        todo!()
    }
}


pub enum DnsResolveError {
    MalformedQuery,
    UnhandledRr,
    Unreachable,
}

pub struct DnsResolveEvent<'a> {
    query: &'a [u8],
}

impl<'a> DnsResolveEvent<'a> {
    #[inline]
    pub fn new(query: &'a [u8]) -> Self {
        Self { query }
    }
}

impl Event for DnsResolveEvent<'_> {
    type Success = Vec<u8>;
    type Error = DnsResolveError;

    fn run(&mut self, state: &mut FizzleState) -> Outcome<Self::Success, Self::Error> {
        match &state.global.nameserver_plugin {
            Some(rc) => {
                let mut nameserver = rc.borrow_mut();
                let Ok(num_read) = nameserver.read(self.query, &Context { endpoint: IoEndpointVariant::Nameservers, stream_id: StreamId::from(0) }) else {
                    return Outcome::Error(DnsResolveError::MalformedQuery)
                };

                if num_read < self.query.len() {
                    return Outcome::Error(DnsResolveError::MalformedQuery)
                }

                let mut response: Vec<u8> = Vec::with_capacity(65536);
                let Ok(num_written) = nameserver.write(unsafe { slice::from_raw_parts_mut(response.as_mut_ptr().cast(), 65536) }, &Context { endpoint: IoEndpointVariant::Nameservers, stream_id: StreamId::from(0) }) else {
                    return Outcome::Error(DnsResolveError::Unreachable)
                };

                unsafe {
                    response.set_len(num_written);
                }

                Outcome::Success(response)
            }
            None => {
                run_without_plugin(self.query, state)
            }
        }
    }
}

fn run_without_plugin(query: &[u8], state: &mut FizzleState) -> Outcome<Vec<u8>, DnsResolveError> {
    let m = match Message::from_bytes(query) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("invalid DNS query message received: {:?}", e);
            return Outcome::Error(DnsResolveError::MalformedQuery)
        }
    };

    let queries = m.queries();
    if queries.len() != 1 {
        log::warn!("invalid DNS message with multiple queries received--rejecting");
        return Outcome::Error(DnsResolveError::MalformedQuery)
    } 

    let q = queries[0].clone();

    let rtype = q.query_type;

    let record_bytes = loop {
        let mut data = [0u8; 1024];
        state.global.prefuzz_rng.try_fill_bytes(&mut data).unwrap();

        let amount = cmp::min(state.global.fuzz_input.len(), data.len());
        for i in 0..amount {
            data[i] ^= state.global.fuzz_input[i];
        }

        // This violates DRY SOOOOO much.
        match rtype {
            RecordType::A => {
                match A::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::A(rdata))).to_vec() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS A Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS A Resource Record--retrying...");
                    }
                };
            },
            RecordType::AAAA => {
                match AAAA::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::AAAA(rdata))).to_vec() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS AAAA Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS AAAA Resource Record--retrying...");
                    }
                };
            },
            RecordType::ANAME => {
                match ANAME::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::ANAME(rdata))).to_vec() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS ANAME Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS ANAME Resource Record--retrying...");
                    }
                };
            },
            RecordType::CAA => {
                match CAA::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::CAA(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS CAA Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS CAA Resource Record--retrying...");
                    }
                };
            },
            RecordType::CERT => {
                match CERT::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::CERT(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS CERT Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS CERT Resource Record--retrying...");
                    }
                };
            },
            RecordType::CSYNC => {
                match CNAME::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::CNAME(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS CNAME Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS CNAME Resource Record--retrying...");
                    }
                };
            },
            RecordType::HINFO => {
                match HINFO::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::HINFO(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS HINFO Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS HINFO Resource Record--retrying...");
                    }
                };
            },
            RecordType::HTTPS => {
                match HTTPS::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::HTTPS(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS HTTPS Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS HTTPS Resource Record--retrying...");
                    }
                };
            },
            RecordType::MX => {
                match MX::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::MX(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS MX Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS MX Resource Record--retrying...");
                    }
                };
            }
            RecordType::NAPTR => {
                match NAPTR::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::NAPTR(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS NAPTR Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS NAPTR Resource Record--retrying...");
                    }
                };
            },
            RecordType::NS => {
                match NS::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::NS(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS NS Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS NS Resource Record--retrying...");
                    }
                };
            },
            RecordType::NULL => {
                match NULL::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::NULL(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS NULL Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS NULL Resource Record--retrying...");
                    }
                };
            },
            RecordType::OPT => {
                match OPT::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::OPT(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS OPT Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS OPT Resource Record--retrying...");
                    }
                };
            },
            RecordType::PTR => {
                match PTR::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::PTR(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS PTR Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS PTR Resource Record--retrying...");
                    }
                };
            },
            RecordType::SOA => {
                match SOA::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::SOA(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS SOA Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS SOA Resource Record--retrying...");
                    }
                };
            },
            RecordType::SRV => {
                match SRV::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::SRV(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS SRV Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS SRV Resource Record--retrying...");
                    }
                };
            },
            RecordType::SSHFP => {
                match SSHFP::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::SSHFP(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS SSHFP Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS SSHFP Resource Record--retrying...");
                    }
                };
            },
            RecordType::SVCB => {
                match SVCB::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::SVCB(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS SVCB Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS SVCB Resource Record--retrying...");
                    }
                };
            },
            RecordType::TLSA => {
                match TLSA::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::TLSA(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS TLSA Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS TLSA Resource Record--retrying...");
                    }
                };
            },
            RecordType::TXT => {
                match Spf::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::TXT(TXT::from_bytes(rdata.to_txt())))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS TXT Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS TXT Resource Record--retrying...");
                    }
                };
            },
            RecordType::Unknown(99) => { // SPF
                match Spf::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::Unknown { code: RecordType::Unknown(99), rdata: NULL::with(rdata.to_vec()) })).to_bytes() {
                        Ok(b) => {
                            log::debug!("SPF record {:?} returned", rdata);
                            break b
                        },
                        Err(_e) => {
                            log::error!("Entropic created DNS SPF Resource Record that failed to convert to bytes--retrying...");
                        }
                    }
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS SPF Resource Record--retrying...");
                    }
                };
            },
            RecordType::ZERO => {
                match A::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                    Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::A(rdata))).to_bytes() {
                        Ok(b) => break b,
                        Err(_e) => {
                            log::error!("Entropic created DNS TXT Resource Record that failed to convert to bytes--retrying...");
                        }
                    } 
                    Err(_e) => {
                        log::error!("Entropic failed to generate DNS TXT Resource Record--retrying...");
                    }
                };
            },
            unknown_rr => {
                log::error!("unhandled DNS resource record type {:?}", unknown_rr);
                return Outcome::Error(DnsResolveError::UnhandledRr)
            }
        }
    };

    Outcome::Success(record_bytes)
}
