use std::{cmp, collections::VecDeque};

use fizzle_plugin::{Plugin, PluginError, PluginModule};

pub struct DnsFuzzClient {
    packets: VecDeque<Vec<u8>>,
    packet_idx: usize,
}

#[allow(non_camel_case_types)]
impl PluginModule for DnsFuzzClient {
    fn fuzz_round_start(&mut self, mut entropy: &[u8]) {
        self.packet_idx = 0;
        self.packets.clear();

        // DNS-over-TCP style--2 bytes in network order indicate packet length.
        // Note that this means `fizzle-plugin-aflnet-generic` effectively does the exact same for TCP-based DNS fuzzing
        loop {
            if entropy.len() < 2 {
                self.packets.push_back(Vec::from(entropy));
                return
            }

            let data_len = u16::from_be_bytes(entropy[..2].try_into().unwrap()) as usize;
            let effective_len = cmp::min(data_len, entropy.len() - 2);
            self.packets.push_back(Vec::from(&entropy[2..2 + effective_len]));
            entropy = &entropy[2 + effective_len..];
            if entropy.is_empty() {
                return
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

impl Plugin for DnsFuzzClient {
    fn new(
        _config: std::collections::HashMap<fizzle_plugin::IoEndpointVariant, toml::Table>,
    ) -> Self {
        Self {
            packets: Default::default(),
            packet_idx: 0,
        }
    }
}
