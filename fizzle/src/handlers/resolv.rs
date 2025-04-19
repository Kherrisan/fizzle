use std::{cmp, iter};

use crate::scheduler::{Event, Outcome};
use crate::state::FizzleState;

use hickory_proto::op::{Message, Query};
use hickory_proto::rr::{RData, Record, RecordType};
use hickory_proto::rr::rdata::*;
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use rand::Rng;
use entropic::prelude::*;



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
        let q = match Query::from_bytes(self.query) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("invalid DNS query received: {:?}", e);
                return Outcome::Error(DnsResolveError::MalformedQuery)
            }
        };

        /*
        let m = match Message::from_bytes(self.query) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("invalid DNS query received: {:?}", e);
                return Outcome::Error(DnsResolveError::MalformedQuery)
            }
        };
        */

        let rtype = q.query_type;

        let record_bytes = loop {
            let mut data = [0u8; 1024];
            state.global.prefuzz_rng.fill(&mut data);

            let amount = cmp::min(state.global.fuzz_input.len(), data.len());
            for i in 0..amount {
                data[i] ^= state.global.fuzz_input[i];
            }

            // This violates DRY SOOOOO much.
            match rtype {
                RecordType::A => {
                    match A::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::A(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS A Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS A Resource Record--retrying...");
                        }
                    };
                },
                RecordType::AAAA => {
                    match AAAA::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::AAAA(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS AAAA Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS AAAA Resource Record--retrying...");
                        }
                    };
                },
                RecordType::ANAME => {
                    match ANAME::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::ANAME(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS ANAME Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS ANAME Resource Record--retrying...");
                        }
                    };
                },
                RecordType::CAA => {
                    match CAA::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::CAA(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS CAA Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS CAA Resource Record--retrying...");
                        }
                    };
                },
                RecordType::CERT => {
                    match CERT::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::CERT(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS CERT Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS CERT Resource Record--retrying...");
                        }
                    };
                },
                RecordType::CSYNC => {
                    match CNAME::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::CNAME(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS CNAME Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS CNAME Resource Record--retrying...");
                        }
                    };
                },
                RecordType::CSYNC => {
                    match CSYNC::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::CSYNC(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS CSYNC Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS CSYNC Resource Record--retrying...");
                        }
                    };
                },
                RecordType::HINFO => {
                    match HINFO::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::HINFO(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS HINFO Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS HINFO Resource Record--retrying...");
                        }
                    };
                },
                RecordType::HTTPS => {
                    match HTTPS::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::HTTPS(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS HTTPS Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS HTTPS Resource Record--retrying...");
                        }
                    };
                },
                RecordType::MX => {
                    match MX::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::MX(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS MX Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS MX Resource Record--retrying...");
                        }
                    };
                }
                RecordType::NAPTR => {
                    match NAPTR::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::NAPTR(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS NAPTR Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS NAPTR Resource Record--retrying...");
                        }
                    };
                },
                RecordType::NS => {
                    match NS::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::NS(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS NS Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS NS Resource Record--retrying...");
                        }
                    };
                },
                RecordType::NULL => {
                    match NULL::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::NULL(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS NULL Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS NULL Resource Record--retrying...");
                        }
                    };
                },
                RecordType::OPT => {
                    match OPT::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::OPT(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS OPT Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS OPT Resource Record--retrying...");
                        }
                    };
                },
                RecordType::PTR => {
                    match PTR::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::PTR(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS PTR Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS PTR Resource Record--retrying...");
                        }
                    };
                },
                RecordType::SOA => {
                    match SOA::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::SOA(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS SOA Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS SOA Resource Record--retrying...");
                        }
                    };
                },
                RecordType::SRV => {
                    match SRV::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::SRV(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS SRV Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS SRV Resource Record--retrying...");
                        }
                    };
                },
                RecordType::SSHFP => {
                    match SSHFP::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::SSHFP(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS SSHFP Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS SSHFP Resource Record--retrying...");
                        }
                    };
                },
                RecordType::SVCB => {
                    match SVCB::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::SVCB(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS SVCB Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS SVCB Resource Record--retrying...");
                        }
                    };
                },
                RecordType::TLSA => {
                    match TLSA::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::TLSA(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS TLSA Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
                            log::error!("Entropic failed to generate DNS TLSA Resource Record--retrying...");
                        }
                    };
                },
                RecordType::TXT => {
                    match TXT::from_entropy::<_, DefaultEntropyScheme>(data.iter().chain(iter::repeat(&0u8).take(65536))) {
                        Ok(rdata) => match Record::from_rdata(q.name.clone(), 127, RData::TXT(rdata)).to_bytes() {
                            Ok(b) => break b,
                            Err(e) => {
                                log::error!("Entropic created DNS TXT Resource Record that failed to convert to bytes--retrying...");
                            }
                        } 
                        Err(e) => {
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
}
