use std::{cmp, collections::VecDeque};

use fizzle_plugin::{Plugin, PluginError, PluginModule};

pub struct LibaflFuzzClient {
    packets: VecDeque<Vec<u8>>,
}

#[allow(non_camel_case_types)]
impl PluginModule for LibaflFuzzClient {
    fn fuzz_round_start(&mut self, entropy: &[u8]) {
        self.packets.clear();
        
        let len = u32::from_be_bytes(entropy[..4].try_into().unwrap()) as usize;
        assert_eq!(entropy.len(), len + 4);
        let mut idx = 4;
        while idx < entropy.len() {
            let pktlen = u32::from_be_bytes(entropy[idx..idx + 4].try_into().unwrap()) as usize;
            idx += 4;

            let pkt = Vec::from(&entropy[idx..idx + pktlen]);
            idx += pktlen;

            if !pkt.is_empty() {
                self.packets.push_back(pkt);
            }
        }
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

        let Some(pkt) = self.packets.front() else {
            return Err(PluginError::NotReady)
        };

        let write_len = cmp::min(buf.len(), pkt.len());

        for (dst, src) in buf.iter_mut().zip(pkt.iter()) {
            dst.write(*src);
        }

        self.packets.pop_front();

        Ok(write_len)
    }

    fn can_read(&self, _ctx: &fizzle_plugin::Context) -> bool {
        true
    }

    fn can_write(&self, _ctx: &fizzle_plugin::Context) -> bool {
        !self.packets.is_empty()
    }
}

impl Plugin for LibaflFuzzClient {
    fn new(
        _config: std::collections::HashMap<fizzle_plugin::IoEndpointVariant, toml::Table>,
    ) -> Self {
        Self {
            packets: Default::default(),
        }
    }
}

