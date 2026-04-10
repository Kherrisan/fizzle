#[allow(unused)]
#[allow(non_camel_case_types)]
mod ngap;

use bitvec::prelude::*;

use std::cmp;

use asnfuzzgen_codecs::aper::AperCodec;
use asnfuzzgen_codecs::PerCodecData;
use fizzle_plugin::{Plugin, PluginError, PluginModule};

use crate::ngap::{AMFName, Criticality, NGAP_PDU, ProcedureCode, ProtocolIE_ID, SuccessfulOutcome, SuccessfulOutcomeValue};
use crate::ngap::{NGSetupResponse, NGSetupResponseProtocolIEs, NGSetupResponseProtocolIEs_Entry, NGSetupResponseProtocolIEs_EntryValue};

pub enum NgapState {
    PreNgapSetup,
    PostNgapSetup,
}

pub struct NgapServer {
    request_bytes: Vec<u8>,
    response_bytes: Vec<u8>,
    response_idx: usize,
    ngap_state: NgapState,
}

#[allow(non_camel_case_types)]
impl PluginModule for NgapServer {
    fn fuzz_round_start(&mut self, _entropy: &[u8]) {
        self.request_bytes.clear();
        self.response_bytes.clear();
        self.response_idx = 0;
        self.ngap_state = NgapState::PreNgapSetup;
    }

    fn read(
        &mut self,
        buf: &[u8],
        _ctx: &fizzle_plugin::Context,
    ) -> Result<usize, fizzle_plugin::PluginError> {
        self.request_bytes.extend_from_slice(buf);

        let mut codec_data = PerCodecData::from_slice_aper(&self.request_bytes);
        match ngap::NGAP_PDU::aper_decode(&mut codec_data) {
            Ok(ngap_pdu) => match self.ngap_state {
                NgapState::PreNgapSetup => {
                    let NGAP_PDU::InitiatingMessage(ngap::InitiatingMessage {
                        procedure_code: _,
                        criticality: _,
                        value: ngap::InitiatingMessageValue::Id_NGSetup(ngsetup),
                    }) = ngap_pdu else {
                        todo!("Handle missing setup message")
                    };

                    // TODO: continue here, pull out fields from NGSetupRequest, create NGSetupResponse and encode to `response_bytes` buffer.
                    
                    // Let's see if sending the same NGSetupResponse every time
                    // will cause any problems.
                    let mut ng_setup_response_protocol_i_es = NGSetupResponseProtocolIEs(Vec::new());

                    // TODO Add the NGSetupResponseProtocolIEs_Entry structs to NGSetupResponseProtocolIEs
                    // TODO Check if it compiles first, I guess
                    ng_setup_response_protocol_i_es.0.push(
                        NGSetupResponseProtocolIEs_Entry{
                            id: ProtocolIE_ID(1),
                            criticality: ngap::Criticality(Criticality::REJECT),
                            value: NGSetupResponseProtocolIEs_EntryValue::Id_AMFName(AMFName("Fizzle".to_owned())),
                        }
                    );
                        
                    let mut response_ngap_pdu = NGAP_PDU::SuccessfulOutcome(SuccessfulOutcome {
                        procedure_code: ProcedureCode(21),  // ngsetup is response code 21
                        criticality: Criticality {0: Criticality::REJECT},
                        value: SuccessfulOutcomeValue::Id_NGSetup(NGSetupResponse {
                            protocol_i_es: ng_setup_response_protocol_i_es
                        }),
                    });


                },
                NgapState::PostNgapSetup => todo!(),
            },
            Err(e) => {
                todo!("Handle missing error handling")
            },
        }



        Ok(buf.len())
    }

    // TODO: designate write() as unsafe
    fn write(
        &mut self,
        buf: &mut [std::mem::MaybeUninit<u8>],
        _ctx: &fizzle_plugin::Context,
    ) -> Result<usize, fizzle_plugin::PluginError> {
        let message_rem = &self.response_bytes[self.response_idx..];
        let write_len = cmp::min(buf.len(), message_rem.len());

        for (dst, src) in buf.iter_mut().zip(message_rem.iter()) {
            dst.write(*src);
        }

        self.response_idx += write_len;
        Ok(write_len)
    }

    fn can_read(&self, _ctx: &fizzle_plugin::Context) -> bool {
        true
    }

    fn can_write(&self, _ctx: &fizzle_plugin::Context) -> bool {
        self.response_idx < self.response_bytes.len()
    }
}

impl Plugin for NgapServer {
    fn new(
        _config: std::collections::HashMap<fizzle_plugin::IoEndpointVariant, toml::Table>,
    ) -> Self {
        Self {
            request_bytes: Vec::new(),
            response_bytes: Vec::new(),
            response_idx: 0,
            ngap_state: NgapState::PreNgapSetup,
        }
    }
}
