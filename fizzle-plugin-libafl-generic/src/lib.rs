use std::{cmp, collections::VecDeque};

use fizzle_plugin::{Plugin, PluginError, PluginModule};

pub struct LibaflFuzzClient {
    packets: VecDeque<Vec<u8>>,
    packet_idx: usize,
}

#[allow(non_camel_case_types)]
impl PluginModule for LibaflFuzzClient {
    fn fuzz_round_start(&mut self, entropy: &[u8]) {
        self.packets.clear();
        let count = u32::from_be_bytes(entropy[..4].try_into().unwrap()) as usize;

        let mut idx = 4;
        for _ in 0..count {
            let len = u32::from_be_bytes(entropy[..4].try_into().unwrap()) as usize;
            idx += 4;
            self.packets.push_back(Vec::from(&entropy[idx..idx + len]));
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

        let Some(packet) = self.packets.front() else {
            return Err(PluginError::NotReady)
        };

        let message_rem = &packet[self.packet_idx..];

        let write_len = cmp::min(buf.len(), message_rem.len());

        for (dst, src) in buf.iter_mut().zip(message_rem.iter()) {
            dst.write(*src);
        }

        self.packet_idx += write_len;

        if self.packet_idx == packet.len() {
            self.packet_idx = 0;
            self.packets.pop_front();
        }
        
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
            packet_idx: 0,
        }
    }
}
