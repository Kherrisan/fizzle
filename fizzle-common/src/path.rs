use std::ffi::CStr;

use crate::storage::Buffer;

/// The path for a named semaphore.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SemPath {
    buf: Buffer<252>,
}

impl SemPath {
    pub fn from_cstr(path: &CStr) -> Result<Self, PathError> {
        Self::from_raw_bytes(path.to_bytes_with_nul())
    }

    /// Note that this **should** include a null terminating character.
    pub fn from_raw_bytes(path: &[u8]) -> Result<Self, PathError> {
        if path.len() > 252 {
            return Err(PathError);
        }

        let Some(b'/') = path.first() else {
            return Err(PathError);
        };

        let Some(b'\0') = path.last() else {
            return Err(PathError);
        };

        for &b in path.iter().skip(1).take(path.len() - 2) {
            if b == b'/' || b == b'\0' {
                return Err(PathError);
            }
        }

        let mut buf = Buffer::new();
        buf.append(path);

        Ok(Self { buf })
    }

    pub fn as_cstr(&self) -> &CStr {
        unsafe { CStr::from_bytes_with_nul_unchecked(self.buf.data()) }
    }
}

#[derive(Debug, Clone)]
pub struct PathError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FilePath {
    buf: Buffer<256>,
    trailing_slash: bool,
}

impl Default for FilePath {
    fn default() -> Self {
        let mut buf = Buffer::new();
        buf.append(b"/");

        Self {
            buf,
            trailing_slash: true,
        }
    }
}

impl FilePath {
    fn segment(path: &[u8]) -> &[u8] {
        for (idx, &c) in path.iter().enumerate() {
            if c == b'/' {
                return &path[..idx];
            }
        }
        path
    }

    // Gets the reverse segment of the given path
    fn last_segment(path: &[u8]) -> &[u8] {
        let mut first_slash_seen = false;
        for (idx, &c) in path.iter().enumerate().rev() {
            if c == b'/' {
                if first_slash_seen {
                    return &path[idx + 1..];
                } else {
                    first_slash_seen = true;
                }
            }
        }
        path
    }

    pub fn from_cstr(path: &CStr) -> Result<Self, PathError> {
        Self::from_raw_bytes(path.to_bytes())
    }

    /// Note that this should not include any null terminating character.
    pub fn from_raw_bytes(path: &[u8]) -> Result<Self, PathError> {
        if path.len() > 255 || path.len() == 0 {
            return Err(PathError);
        }

        let mut buf = Buffer::new();
        buf.try_append(path).map_err(|_| PathError)?;
        buf.try_append(b"\0").map_err(|_| PathError)?;

        let mut read_idx = 0usize;
        let mut write_idx = 0usize;
        let data = buf.data_mut();

        // Special case: path is absolute
        if let Some(b'/') = path.get(read_idx) {
            read_idx += 1;
            write_idx += 1;
        }

        while read_idx < path.len() {
            let segment = Self::segment(&path[read_idx..]);
            let segment_len = segment.len();
            match segment {
                b"" | b"." => (), // Do nothing
                b".." => {
                    // Traverse back one segment
                    match Self::last_segment(&data[..write_idx]) {
                        b"" | b"../" => {
                            data[write_idx..write_idx + 3].copy_from_slice(b"../");
                            write_idx += 3;
                        }
                        b"/" => return Err(PathError),
                        segment => write_idx -= segment.len(),
                    }
                }
                _ => {
                    // Copy current segment to write portion
                    data[write_idx..write_idx + segment_len]
                        .copy_from_slice(&path[read_idx..read_idx + segment_len]);
                    write_idx += segment_len;

                    // copy '/' if exists
                    if read_idx + segment_len == path.len() - 1 {
                        data[write_idx] = b'/';
                        write_idx += 1;
                    }
                }
            }

            read_idx += segment_len + 1;
        }

        if write_idx == 0 || (write_idx == 1 && data[0] == b'.') {
            data[..2].copy_from_slice(b"./");
            write_idx = 2;
        }

        let trailing_slash = data[write_idx - 1] == b'/';

        buf.shrink(write_idx).map_err(|_| PathError)?;
        buf.try_append(b"\0").map_err(|_| PathError)?;

        Ok(FilePath {
            buf,
            trailing_slash,
        })
    }

    pub fn concat(mut self, other: &FilePath) -> Result<Self, PathError> {
        let data = &other.buf.data()[..other.buf.data().len() - 1]; // remove null character
        let mut read_idx = 0;

        self.buf.shrink(self.buf.len() - 1).unwrap(); // Remove null character

        while read_idx < other.buf.len() {
            let segment = Self::segment(&data[read_idx..]);
            let segment_len = segment.len();

            match segment {
                b"" | b"." => (), // Do nothing (shouldn't happen unless `other` has an absolute at the start)
                b".." => {
                    // Traverse back one segment
                    match Self::last_segment(self.buf.data()) {
                        b"" | b"../" => self.buf.try_append(b"../").map_err(|_| PathError)?,
                        b"/" => return Err(PathError),
                        segment => self.buf.shrink(segment.len()).unwrap(),
                    }
                }
                _ => {
                    self.buf.try_append(segment).map_err(|_| PathError)?;
                    // copy '/' if exists
                    if segment_len < data.len() - read_idx {
                        self.buf.try_append(b"/").map_err(|_| PathError)?;
                    }
                }
            }

            read_idx += segment_len + 1;
        }

        // Re-add null character
        self.buf.try_append(b"\0").map_err(|_| PathError)?;

        self.trailing_slash = other.trailing_slash;
        Ok(self)
    }

    pub fn is_absolute(&self) -> bool {
        self.buf.data().first() == Some(&b'/')
    }

    pub fn has_trailing_slash(&self) -> bool {
        self.trailing_slash
    }
}
