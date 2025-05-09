use std::{cmp, iter};

use entropic::prelude::*;
use fizzle_plugin::{IoEndpointVariant, Plugin, PluginError, PluginModule};
use hickory_proto::op::Message;
use hickory_proto::rr::{RData, Record, RecordType};
use hickory_proto::rr::rdata::*;
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use rand::{Rng, SeedableRng};

const SPF_RESULT_NONE: u8 = 0;
const SPF_RESULT_NEUTRAL: u8 = 1;
const SPF_RESULT_PASS: u8 = 2;
const SPF_RESULT_FAIL: u8 = 3;
const SPF_RESULT_SOFTFAIL: u8 = 4;
const SPF_RESULT_TEMPERROR: u8 = 5;
const SPF_RESULT_PERMERROR: u8 = 6;

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
                        for _ in 1..=len {
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

pub struct SpfPbtClient {
    input: Vec<u8>,
    input_idx: usize,
    nameserver_rsp: Option<Vec<u8>>,
    rng: rand::rngs::SmallRng,
    query_count: usize,
}

#[allow(non_camel_case_types)]
impl PluginModule for SpfPbtClient {
    fn fuzz_round_start(&mut self, entropy: &[u8]) {
        self.input.clear();
        self.input_idx = 0;
        self.input.extend_from_slice(entropy);

        let mut rng_seed: u64 = 0;
        for i in 0..cmp::min(8, self.input.len()) {
            rng_seed <<= 8;
            rng_seed |= self.input[i] as u64;
        }
        self.rng = rand::rngs::SmallRng::seed_from_u64(rng_seed);
    }

    fn read(
        &mut self,
        buf: &[u8],
        ctx: &fizzle_plugin::Context,
    ) -> Result<usize, fizzle_plugin::PluginError> {
        match &ctx.endpoint {
            IoEndpointVariant::Nameservers => {
                assert!(self.nameserver_rsp.is_none());

                let m = match Message::from_bytes(buf) {
                    Ok(m) => m,
                    Err(e) => {
                        log::warn!("invalid DNS query message received: {:?}", e);
                        return Err(PluginError::ProtocolError)
                    }
                };

                let queries = m.queries();
                if queries.len() != 1 {
                    log::warn!("invalid DNS message with multiple queries received--rejecting");
                    return Err(PluginError::ProtocolError)
                } 

                let q = queries[0].clone();

                let rtype = q.query_type;

                let record_bytes = loop {
                    let mut data = [0u8; 1024];
                    self.rng.fill(&mut data);

                    let amount = cmp::min(self.input.len(), data.len());
                    for i in 0..amount {
                        data[i] ^= self.input[i];
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
                        RecordType::CNAME => {
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
                        RecordType::CSYNC => {
                            match CSYNC::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                                Ok(rdata) => match Message::new().add_query(q.clone()).add_answer(Record::from_rdata(q.name.clone(), 127, RData::CSYNC(rdata))).to_bytes() {
                                    Ok(b) => break b,
                                    Err(_e) => {
                                        log::error!("Entropic created DNS CSYNC Resource Record that failed to convert to bytes--retrying...");
                                    }
                                } 
                                Err(_e) => {
                                    log::error!("Entropic failed to generate DNS CSYNC Resource Record--retrying...");
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
                            return Err(PluginError::ProtocolError)
                        }
                    }
                };

                self.nameserver_rsp = Some(record_bytes);
                self.query_count += 1;
                Ok(buf.len())
            }
            _ => {
                assert_eq!(buf.len(), 1);
                let spf_result = buf[0];
                match spf_result {
                    SPF_RESULT_NONE => {

                    }
                    SPF_RESULT_NEUTRAL => {

                    }
                    SPF_RESULT_PASS => {
                        
                    }
                    SPF_RESULT_FAIL => {

                    }
                    SPF_RESULT_SOFTFAIL => {

                    }
                    SPF_RESULT_TEMPERROR => {

                    }
                    SPF_RESULT_PERMERROR => {

                    }
                    _ => unreachable!("unrecognized spf result byte passed from application")
                }

                if self.query_count > 20 {
                    panic!("property violated: query_count={}", self.query_count)
                }

                Ok(1)
            }
        }
    }

    fn write(
        &mut self,
        buf: &mut [std::mem::MaybeUninit<u8>],
        ctx: &fizzle_plugin::Context,
    ) -> Result<usize, fizzle_plugin::PluginError> {
        match &ctx.endpoint {
            IoEndpointVariant::Nameservers => {
                let rsp = self.nameserver_rsp.take().unwrap();

                let len = cmp::min(buf.len(), rsp.len());
                for i in 0..len {
                    buf[i].write(rsp.as_slice()[i]);
                }
                Ok(len)
            }
            _ => {
                assert_eq!(buf.len(), 1);
                buf[0].write(1);
                Ok(buf.len())
            }
        }
    }

    fn can_read(&self, _ctx: &fizzle_plugin::Context) -> bool {
        true
    }

    fn can_write(&self, _ctx: &fizzle_plugin::Context) -> bool {
        !self.input.is_empty()
    }
}

impl Plugin for SpfPbtClient {
    fn new(
        _config: std::collections::HashMap<fizzle_plugin::IoEndpointVariant, toml::Table>,
    ) -> Self {
        Self {
            input: Default::default(),
            input_idx: 0,
            nameserver_rsp: None,
            rng: rand::rngs::SmallRng::seed_from_u64(0xABAD_5EED_ABAD_5EED_u64),
            query_count: 0,
        }
    }
}
