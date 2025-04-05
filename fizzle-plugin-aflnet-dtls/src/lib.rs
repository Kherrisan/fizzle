use std::{cmp, collections::VecDeque};

use fizzle_plugin::{Plugin, PluginError, PluginModule};

pub struct DtlsFuzzClient {
    packets: VecDeque<Vec<u8>>,
    packet_idx: usize,
}

#[allow(non_camel_case_types)]
impl PluginModule for DtlsFuzzClient {
    fn fuzz_round_start(&mut self, mut entropy: &[u8]) {
        self.packet_idx = 0;
        self.packets.clear();

        while entropy.len() >= 3 {
            for i in 0..entropy.len() - 2 {
                if let 0x14..=0x18 = entropy[i] {
                    if entropy[i+1] == 0xFE && entropy[i+2] == 0xFD {
                        self.packets.push_back(Vec::from(&entropy[..i]));
                        entropy = &entropy[i..];
                    }
                }
            }

            if !entropy.is_empty() {
                self.packets.push_back(Vec::from(entropy));
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

impl Plugin for DtlsFuzzClient {
    fn new(
        _config: std::collections::HashMap<fizzle_plugin::IoEndpointVariant, toml::Table>,
    ) -> Self {
        Self {
            packets: Default::default(),
            packet_idx: 0,
        }
    }
}
