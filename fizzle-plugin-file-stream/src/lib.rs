use std::{cmp, collections::HashMap, env, fs};

use fizzle_plugin::{IoEndpointVariant, Plugin, PluginError, PluginModule};

const FIZZLE_PAYLOAD_FILE_ENV: &str = "FIZZLE_PAYLOAD_FILE";

pub struct FileBackedFuzzClient {
    payload: Vec<u8>,
    cursor: usize,
}

impl PluginModule for FileBackedFuzzClient {
    fn fuzz_round_start(&mut self, _entropy: &[u8]) {
        self.cursor = 0;
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
        let remaining = &self.payload[self.cursor..];
        if remaining.is_empty() {
            return Err(PluginError::NotReady);
        }

        let write_len = cmp::min(buf.len(), remaining.len());
        for (dst, src) in buf.iter_mut().zip(remaining.iter()) {
            dst.write(*src);
        }

        self.cursor += write_len;
        Ok(write_len)
    }

    fn can_read(&self, _ctx: &fizzle_plugin::Context) -> bool {
        true
    }

    fn can_write(&self, _ctx: &fizzle_plugin::Context) -> bool {
        self.cursor < self.payload.len()
    }
}

impl Plugin for FileBackedFuzzClient {
    fn new(_config: HashMap<IoEndpointVariant, toml::Table>) -> Self {
        let payload_path = env::var(FIZZLE_PAYLOAD_FILE_ENV)
            .unwrap_or_else(|_| panic!("{FIZZLE_PAYLOAD_FILE_ENV} must point to a payload file"));
        let payload = fs::read(&payload_path).unwrap_or_else(|err| {
            panic!("failed to read {FIZZLE_PAYLOAD_FILE_ENV}={payload_path}: {err}")
        });

        Self { payload, cursor: 0 }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env;
    use std::fs;
    use std::mem::MaybeUninit;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};
    use std::time::{SystemTime, UNIX_EPOCH};

    use fizzle_plugin::{Context, IoEndpointVariant, Plugin, PluginModule, StreamId};

    use super::FileBackedFuzzClient;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct EnvPayload {
        _guard: MutexGuard<'static, ()>,
        path: PathBuf,
    }

    impl EnvPayload {
        fn new(contents: &[u8]) -> Self {
            let guard = env_lock();
            let path = temp_payload(contents);
            unsafe {
                env::set_var("FIZZLE_PAYLOAD_FILE", &path);
            }

            Self {
                _guard: guard,
                path,
            }
        }
    }

    impl Drop for EnvPayload {
        fn drop(&mut self) {
            unsafe {
                env::remove_var("FIZZLE_PAYLOAD_FILE");
            }
            fs::remove_file(&self.path).unwrap();
        }
    }

    fn temp_payload(contents: &[u8]) -> PathBuf {
        let mut path = env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("fizzle-file-stream-test-{nanos}.raw"));
        fs::write(&path, contents).unwrap();
        path
    }

    fn context() -> Context {
        Context {
            endpoint: IoEndpointVariant::TcpClient("127.0.0.1:6379".parse().unwrap()),
            stream_id: StreamId::from(0),
        }
    }

    fn initialized_bytes(buf: &[MaybeUninit<u8>], len: usize) -> Vec<u8> {
        buf[..len]
            .iter()
            .map(|byte| unsafe { byte.assume_init() })
            .collect()
    }

    #[test]
    fn reads_payload_file_from_environment_and_streams_it_once() {
        let _payload = EnvPayload::new(b"hello");

        let mut plugin = FileBackedFuzzClient::new(HashMap::new());
        let ctx = context();
        let mut out = [MaybeUninit::uninit(); 8];

        assert!(plugin.can_write(&ctx));
        let written = plugin.write(&mut out, &ctx).unwrap();

        assert_eq!(written, 5);
        assert_eq!(initialized_bytes(&out, written), b"hello");
        assert!(!plugin.can_write(&ctx));
    }

    #[test]
    fn splits_payload_across_small_recv_buffers() {
        let _payload = EnvPayload::new(b"abcdef");

        let mut plugin = FileBackedFuzzClient::new(HashMap::new());
        let ctx = context();
        let mut first = [MaybeUninit::uninit(); 2];
        let mut second = [MaybeUninit::uninit(); 4];

        let first_len = plugin.write(&mut first, &ctx).unwrap();
        let second_len = plugin.write(&mut second, &ctx).unwrap();

        assert_eq!(initialized_bytes(&first, first_len), b"ab");
        assert_eq!(initialized_bytes(&second, second_len), b"cdef");
        assert!(!plugin.can_write(&ctx));
    }

    #[test]
    fn fuzz_round_start_rewinds_payload_cursor() {
        let _payload = EnvPayload::new(b"xyz");

        let mut plugin = FileBackedFuzzClient::new(HashMap::new());
        let ctx = context();
        let mut out = [MaybeUninit::uninit(); 3];

        assert_eq!(plugin.write(&mut out, &ctx).unwrap(), 3);
        assert!(!plugin.can_write(&ctx));

        plugin.fuzz_round_start(&[]);

        assert!(plugin.can_write(&ctx));
        assert_eq!(plugin.write(&mut out, &ctx).unwrap(), 3);
        assert_eq!(initialized_bytes(&out, 3), b"xyz");
    }

    #[test]
    #[should_panic(expected = "FIZZLE_PAYLOAD_FILE must point to a payload file")]
    fn panics_when_payload_environment_variable_is_missing() {
        let _guard = env_lock();
        unsafe {
            env::remove_var("FIZZLE_PAYLOAD_FILE");
        }

        let _plugin = FileBackedFuzzClient::new(HashMap::new());
    }
}
