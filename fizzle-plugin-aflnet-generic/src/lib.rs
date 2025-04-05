use std::cmp;

use fizzle_plugin::{Plugin, PluginError, PluginModule};

pub struct SipFuzzClient {
    input: Vec<u8>,
    len: usize,
}

#[allow(non_camel_case_types)]
impl PluginModule for SipFuzzClient {
    fn fuzz_round_start(&mut self, entropy: &[u8]) {
        self.input.clear();
        self.len = 0;
        self.input.extend_from_slice(entropy);
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

        if self.len == self.input.len() {
            return Err(PluginError::NotReady)
        };

        let message_rem = &self.input[self.len..];

        let write_len = cmp::min(buf.len(), message_rem.len());

        for (dst, src) in buf.iter_mut().zip(message_rem.iter()) {
            dst.write(*src);
        }

        self.len += write_len;

        Ok(write_len)
    }

    fn can_read(&self, _ctx: &fizzle_plugin::Context) -> bool {
        true
    }

    fn can_write(&self, _ctx: &fizzle_plugin::Context) -> bool {
        self.len < self.input.len()
    }
}

impl Plugin for SipFuzzClient {
    fn new(
        _config: std::collections::HashMap<fizzle_plugin::IoEndpointVariant, toml::Table>,
    ) -> Self {
        Self {
            input: Default::default(),
            len: 0,
        }
    }
}
