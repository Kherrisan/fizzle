#[allow(unused)]
#[allow(non_camel_case_types)]
mod s1ap;

use bitvec::prelude::*;

use std::cmp;

use asnfuzzgen_codecs::aper::AperCodec;
use asnfuzzgen_codecs::PerCodecData;
use fizzle_plugin::{Plugin, PluginError, PluginModule};

pub struct S1apFuzzClient {
    s1setup_bytes: Vec<u8>,
    s1setup_idx: usize,
    message_bytes: Vec<u8>,
    message_idx: usize,
}

#[allow(non_camel_case_types)]
impl PluginModule for S1apFuzzClient {
    fn fuzz_round_start(&mut self, entropy: &[u8]) {
        self.s1setup_idx = 0;
        self.message_idx = 0;
        self.message_bytes.clear();
        self.message_bytes.extend(entropy);
    }

    fn read(
        &mut self,
        buf: &[u8],
        _ctx: &fizzle_plugin::Context,
    ) -> Result<usize, fizzle_plugin::PluginError> {
        Ok(buf.len())
    }

    fn write(
        &mut self,
        buf: &mut [std::mem::MaybeUninit<u8>],
        _ctx: &fizzle_plugin::Context,
    ) -> Result<usize, fizzle_plugin::PluginError> {
        // TODO: only supports a single communication channel (use plugin contexts to fix)

        let s1setup_rem = &self.s1setup_bytes[self.s1setup_idx..];
        let message_rem = &self.message_bytes[self.message_idx..];

        if !s1setup_rem.is_empty() {
            let write_len = cmp::min(buf.len(), s1setup_rem.len());

            for (dst, src) in buf.iter_mut().zip(s1setup_rem.iter()) {
                dst.write(*src);
            }

            self.s1setup_idx += write_len;
            Ok(write_len)
        } else if !message_rem.is_empty() {
            let write_len = cmp::min(buf.len(), message_rem.len());

            for (dst, src) in buf.iter_mut().zip(message_rem.iter()) {
                dst.write(*src);
            }

            self.message_idx += write_len;
            Ok(write_len)
        } else {
            return Err(PluginError::NotReady);
        }
    }

    fn can_read(&self, _ctx: &fizzle_plugin::Context) -> bool {
        true
    }

    fn can_write(&self, _ctx: &fizzle_plugin::Context) -> bool {
        self.s1setup_idx < self.s1setup_bytes.len() || self.message_idx < self.message_bytes.len()
    }
}

impl Plugin for S1apFuzzClient {
    fn new(
        _config: std::collections::HashMap<fizzle_plugin::IoEndpointVariant, toml::Table>,
    ) -> Self {
        let pdu = s1ap::S1AP_PDU::InitiatingMessage(s1ap::InitiatingMessage {
            procedure_code: s1ap::ProcedureCode(17),
            criticality: s1ap::Criticality(s1ap::Criticality::REJECT),
            value: s1ap::InitiatingMessageValue::Id_S1Setup(s1ap::S1SetupRequest {
                protocol_i_es: s1ap::S1SetupRequestProtocolIEs(vec![
                    s1ap::S1SetupRequestProtocolIEs_Entry {
                        id: s1ap::ProtocolIE_ID(59),
                        criticality: s1ap::Criticality(s1ap::Criticality::REJECT),
                        value: s1ap::S1SetupRequestProtocolIEs_EntryValue::Id_Global_ENB_ID(
                            s1ap::Global_ENB_ID {
                                plm_nidentity: s1ap::TBCD_STRING(vec![0x99, 0xf9, 0x07]),
                                enb_id: s1ap::ENB_ID::MacroENB_ID(s1ap::ENB_ID_macroENB_ID(
                                    BitVec::<u8, Msb0>::from_bitslice(
                                        bitvec::bits!(u8, Msb0; 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1),
                                    ),
                                )),
                                ie_extensions: None,
                            },
                        ),
                    },
                    s1ap::S1SetupRequestProtocolIEs_Entry {
                        id: s1ap::ProtocolIE_ID(60),
                        criticality: s1ap::Criticality(s1ap::Criticality::REJECT),
                        value: s1ap::S1SetupRequestProtocolIEs_EntryValue::Id_eNBname(
                            s1ap::ENBname("fuzz_enb".to_string()),
                        ),
                    },
                    s1ap::S1SetupRequestProtocolIEs_Entry {
                        id: s1ap::ProtocolIE_ID(64),
                        criticality: s1ap::Criticality(s1ap::Criticality::REJECT),
                        value: s1ap::S1SetupRequestProtocolIEs_EntryValue::Id_SupportedTAs(
                            s1ap::SupportedTAs(vec![s1ap::SupportedTAs_Item {
                                tac: s1ap::TAC(vec![0x00, 0x01]),
                                broadcast_plm_ns: s1ap::BPLMNs(vec![s1ap::TBCD_STRING(vec![
                                    0x99, 0xf9, 0x07,
                                ])]),
                                ie_extensions: None,
                            }]),
                        ),
                    },
                    s1ap::S1SetupRequestProtocolIEs_Entry {
                        id: s1ap::ProtocolIE_ID(137),
                        criticality: s1ap::Criticality(s1ap::Criticality::REJECT),
                        value: s1ap::S1SetupRequestProtocolIEs_EntryValue::Id_DefaultPagingDRX(
                            s1ap::PagingDRX(s1ap::PagingDRX::V128),
                        ),
                    },
                ]),
            }),
        });

        let mut pdu_data = PerCodecData::new_aper();
        pdu.aper_encode(&mut pdu_data).unwrap();

        let setup_bytes = pdu_data.into_bytes();
        Self {
            s1setup_bytes: setup_bytes,
            s1setup_idx: 0,
            message_bytes: Vec::new(),
            message_idx: 0,
        }
    }
}
