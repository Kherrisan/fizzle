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

        loop {
            if entropy.is_empty() {
                break
            }

            if entropy.len() < 12 {
                self.packets.push_back(Vec::from(entropy));
                break
            }

            let resp = &entropy[12..];
            let mut resp_len = resp.len();
            for i in 0..resp_len {
                if resp[i] == b'\0' {
                    resp_len = cmp::min(i + 4, resp.len());
                    break
                }
            }

            self.packets.push_back(Vec::from(&entropy[..12 + resp_len]));
            entropy = &resp[resp_len..];
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
