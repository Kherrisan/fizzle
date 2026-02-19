use std::ffi::{CStr, VaList};
use std::{mem, usize};

use crate::errno::Errno;
use crate::handlers::filestream::{FilePtr, StreamReadEvent, StreamUngetEvent};
use crate::external::{STDERR, STDIN, STDOUT, vsscanf};
use crate::scheduler::Scheduler;
#[cfg(feature = "sigsan")]
use crate::state::in_sighandler;

enum MatchFailure {
    Truncated {
        /// The number of parameters successfully matched.
        matched: usize,
        /// The number of bytes of input consumed by the scan operation.
        input_consumed: usize,
        /// The number of bytes of the formatter consumed by the scan operation.
        format_consumed: usize,
        /// The minimum number of additional characters that would be needed for a successful match.
        min_remainder: usize,
    },
    /// An invalid character was reached for the given formatter.
    /// 
    /// The value returned indicates the number of items successfully matched and assigned.
    BadInput(usize),
}

/// Returns the minimum number of bytes that the given scan operation would consume.
/// 
/// If an invalid character sequence is found within the C-string, this will
/// return None.
fn scan_incremental(format: &[u8], input: &[u8], params: &[*mut libc::c_void]) -> Result<usize, MatchFailure> {
    let mut format_idx = 0;
    let mut input_idx = 0;
    let mut params_idx = 0;
    let mut needed = 1;
    let mut truncated = false;

    while input_idx < input.len() && format_idx < format.len() {
        if unsafe { libc::isspace(format[format_idx] as i32) != 0 } {
            if let Some(consumed) = match_whitespace(&input[input_idx..]) {
                input_idx += consumed;
                format_idx += 1;
            } else {
                truncated = true;
                break;
                // This will break out of while loop and result in a `Truncated` error
            }
        } else if format[format_idx] == b'%' {
            if format_idx + 1 == format.len() {
                return Err(MatchFailure::BadInput(format_idx))
            }

            match match_param(&format[format_idx + 1..], &input[input_idx..], params, params_idx) {
                Ok(ParamSuccess { input_consumed, format_consumed, assigned }) => {
                    log::debug!("match_param() -> Success {{ input_consumed={}, format_consumed={}, assigned={} }}", input_consumed, format_consumed, assigned);
                    input_idx += input_consumed;
                    format_idx += 1 + format_consumed;

                    if assigned {
                        params_idx += 1;
                    }
                }
                Err(Some(needed_for_param)) => {
                    log::debug!("match_param() -> Err(Some(needed_for_param={}))", needed_for_param);
                    needed = needed_for_param;
                    truncated = true;
                    break
                }
                Err(None) => {
                    log::debug!("match_param() -> Err(None)");
                    return Err(MatchFailure::BadInput(params_idx))
                }
            }

        } else {
            if input[input_idx] != format[format_idx] {
                return Err(MatchFailure::BadInput(params_idx))
            }
            input_idx += 1;
            format_idx += 1;
        }
    }

    if format_idx < format.len() {
        truncated = true;
    }

    if !truncated {
        // assert_eq!(params_idx, params.len());
        log::debug!("scan_incremental() -> Ok({})", input_idx);
        Ok(input_idx)

    } else {
        // Try to determine the number of remaining tokens needed.
        let mut tmp_fmt_idx = format_idx;

        // Discard the immediate consumed token
        match format[tmp_fmt_idx] {
            b'%' => {
                tmp_fmt_idx += 1;
                if tmp_fmt_idx == format.len() {
                    return Err(MatchFailure::BadInput(params_idx))
                }

                let Some(param) = param_ty(&format[tmp_fmt_idx..]) else {
                    return Err(MatchFailure::BadInput(params_idx))
                };
                tmp_fmt_idx += param.consumed;
            }
            _ => ()
        }

        while tmp_fmt_idx < format.len() {
            match format[tmp_fmt_idx] {
                b'%' => {
                    tmp_fmt_idx += 1;
                    if tmp_fmt_idx == format.len() {
                        return Err(MatchFailure::BadInput(params_idx))
                    }

                    let Some(param) = param_ty(&format[tmp_fmt_idx..]) else {
                        return Err(MatchFailure::BadInput(params_idx))
                    };
                    tmp_fmt_idx += param.consumed;
                    needed += param.min_length();
                }
                c if unsafe { libc::isspace(c as i32) } != 0 => tmp_fmt_idx += 1,
                _ => {
                    tmp_fmt_idx += 1;
                    needed += 1;
                }
            }
        }

        Err(MatchFailure::Truncated {
            matched: params_idx,
            input_consumed: input_idx,
            format_consumed: format_idx,
            min_remainder: needed, // TODO: estimate remaining amount needed
        })
    }
}

fn match_whitespace(input: &[u8]) -> Option<usize> {
    for (idx, c) in input.iter().enumerate() {
        if unsafe { libc::isspace(*c as i32) == 0 } {
            // return when the first non-whitespace character is reached
            return Some(idx)
        }
    }

    None
}

pub struct ParamSuccess {
    input_consumed: usize,
    format_consumed: usize,
    assigned: bool,
}


pub struct ParamInfo<'a> {
    ty: ParamType<'a>,
    ptr_ty: Option<PointerType>,
    max_field_width: Option<usize>,
    param_idx: Option<usize>,
    masked: bool,
    do_alloc: bool,
    consumed: usize,
    thousands_separators: bool,
}

impl ParamInfo<'_> {
    pub fn min_length(&self) -> usize {
        match &self.ty {
            ParamType::Percent => 1,
            ParamType::SignedDecimal => 1,
            ParamType::SignedInt => 1,
            ParamType::UnsignedOctInt => 1,
            ParamType::UnsignedDecimalInt => 1,
            ParamType::UnsignedHexInt => 1,
            ParamType::Float => 1,
            ParamType::Sequence => 1,
            ParamType::CSequence => self.max_field_width.unwrap_or(1), // TODO: likely incorrect
            ParamType::Charset(_) => 1,
            ParamType::NotCharset(_) => 1,
            ParamType::Pointer => todo!(),
            ParamType::Consumed => 0,
        }
    }
}

#[derive(Clone, Copy)]
pub enum ParamType<'a> {
    Percent,
    /// An optionally-signed decimal integer.
    SignedDecimal,
    /// An optionally-signed integer. Read in base-16 if starts with `0x`/`0X`, base 8 if starts with `0`, or base 10 otherwise.
    SignedInt,
    /// An unsigned octal integer.
    UnsignedOctInt,
    /// An unsigned decimal integer.
    UnsignedDecimalInt,
    /// An unsigned hexadecimal integer. Optionally begins with `0x` or `0X`.
    UnsignedHexInt,
    /// An optionally-signed floating-point number.
    Float,
    /// A non-empty sequence of non-whitespace characters.
    Sequence,
    /// A sequence of characters exactly equal to the `maximum field width`.
    CSequence,
    /// A non-empty sequence of characters from the specified set.
    Charset(&'a [u8]),
    /// A non-empty sequence of characters _not_ from the specified set.
    NotCharset(&'a [u8]),
    /// A pointer value (as printed by %p in printf).
    Pointer,
    /// The number of characters consumed thus far from the input.
    Consumed,
}

pub enum PointerType {
    Short,
    Char,
    IntMax,
    Long,
    LongLong,
    Ptrdiff,
    SizeType,
}

impl PointerType {
    pub fn width(&self) -> usize {
        match self {
            PointerType::Short => mem::size_of::<libc::c_short>(),
            PointerType::Char => mem::size_of::<libc::c_char>(),
            PointerType::IntMax => mem::size_of::<libc::intmax_t>(),
            PointerType::Long => mem::size_of::<libc::c_long>(),
            PointerType::LongLong => mem::size_of::<libc::c_longlong>(),
            PointerType::Ptrdiff => mem::size_of::<libc::ptrdiff_t>(),
            PointerType::SizeType => mem::size_of::<libc::size_t>(),
        }
    }
}

fn param_ty(format: &[u8]) -> Option<ParamInfo<'_>> {
    let mut idx = 0;
    let mut masked = false;
    let mut thousands_separators = false;
    let mut do_alloc = false;
    let mut param_idx = None;
    let mut max_field_width = None;
    let mut ptr_ty = None;

    'prefix_end: {
        // Step 1: parse `%n$` structure, if applicable
        while let Some(&c) = format.get(idx) {
            match c {
                b'0'..=b'9' => param_idx = Some(match param_idx {
                    None => (c - b'0') as usize,
                    Some(i) => (i * 10) + ((c - b'0') as usize),
                }),
                b'$' if param_idx.is_none() => return None, // no number between `%` and `$`
                b'$' => {
                    idx += 1;
                    break
                }
                _ if param_idx.is_none() => break,
                _ => {
                    // This was actually the maximum field width
                    max_field_width = param_idx.take();
                    break 'prefix_end
                }
            }

            idx += 1;
        }

        // Step 2: parse `*` assignment-suppression character and `'` quote character in any order.
        while let Some(&c) = format.get(idx) {
            match c {
                b'*' if masked => return None,
                b'*' => masked = true,
                b'\'' if thousands_separators => return None,
                b'\'' => thousands_separators = true,
                _ => break
            }
            
            idx += 1;
        }

        // Step 3: optionally parse `m` character
        if let Some(b'm') = format.get(idx) {
            idx += 1;
            do_alloc = true;
        }

        // Step 4: optional maximum field width
        while let Some(&c) = format.get(idx) {
            match c {
                b'0'..=b'9' => max_field_width = Some(match max_field_width {
                    None => (c - b'0') as usize,
                    Some(i) => (i * 10) + ((c - b'0') as usize),
                }),
                _ => break,
            }

            idx += 1;
        }
    }

    // Step 5: optional type modifier
    if let Some(c) = format.get(idx) {
        'ty_mod: {
            match c {
                b'h' => ptr_ty = Some(if let Some(b'h') = format.get(idx + 1) {
                    idx += 1;
                    PointerType::Char
                } else {
                    PointerType::Short
                }),
                b'j' => ptr_ty = Some(PointerType::IntMax),
                b'l' => ptr_ty = Some(if let Some(b'l') = format.get(idx + 1) {
                    idx += 1;
                    PointerType::LongLong
                } else {
                    PointerType::Long
                }),
                b'L' | b'q' => ptr_ty = Some(PointerType::LongLong), // TODO: slight differences between this and `ll`...
                b't' => ptr_ty = Some(PointerType::Ptrdiff),
                b'z' => ptr_ty = Some(PointerType::SizeType),
                _ => break 'ty_mod,
            }

            idx += 1;
        }
    }

    // Step 6: conversion specifier
    let Some(c) = format.get(idx) else {
        return None
    };

    idx += 1;

    let param_ty = match c {
        b'%' => ParamType::Percent,
        b'd' => ParamType::SignedDecimal,
        b'i' => ParamType::SignedInt,
        b'o' => ParamType::UnsignedOctInt,
        b'u' => ParamType::UnsignedDecimalInt,
        b'x' | b'X' => ParamType::UnsignedHexInt,
        b'f' | b'e' | b'g' | b'E' | b'a' => ParamType::Float,
        b's' => ParamType::Sequence,
        b'c' => ParamType::CSequence,
        b'[' => {
            let mut negation = false;
            let mut start_idx = idx;

            if matches!(format.get(idx), Some(b'^')) {
                negation = true;
                start_idx += 1;
                idx += 1;
            }

            if matches!(format.get(idx), Some(b']')) {
                idx += 1;
            }

            let mut charset = None;

            while let Some(&c) = format.get(idx) {
                match c {
                    // Make sure there's no backslash immediately preceding the closing bracket that would escape it
                    b']' if matches!(format.get(idx - 1), Some(b'\\')) && !matches!(format.get(idx - 2), Some(b'\\')) => (),
                    b']' if negation => {
                        charset = Some(ParamType::NotCharset(&format[start_idx..idx]));
                        idx += 1;
                        break
                    },
                    b']' => {
                        charset = Some(ParamType::Charset(&format[start_idx..idx - 1]));
                        idx += 1;
                        break
                    },
                    _ => (),   
                }

                idx += 1;
            }

            charset?
        }
        b'p' => ParamType::Pointer,
        b'n' => ParamType::Consumed,
        _ => return None
    };

    Some(ParamInfo {
        ty: param_ty,
        ptr_ty,
        max_field_width,
        param_idx,
        masked,
        do_alloc,
        consumed: idx,
        thousands_separators,
    })
}

/// Matches to a single `%` parameter.
/// 
/// If there isn't enough data to match the parameter, this returns Err(true).
/// If processing would return the error, this returns Err(false).
fn match_param(format: &[u8], input: &[u8], _params: &[*mut libc::c_void], _params_idx: usize) -> Result<ParamSuccess, Option<usize>> {
    log::debug!("match_param(format={:?}, input={:?}) -> ...", format, input);

    let param_info = param_ty(format).ok_or(None)?;

    let mut input_idx = 0;

    let max_width = param_info.max_field_width.unwrap_or(usize::MAX);
    if max_width == 0 {
        return Err(None)
    }

    match param_info.ty {
        ParamType::Percent => {
            let Some(&b'%') = input.get(0) else {
                return Err(None)
            };
            input_idx += 1;
        }
        ParamType::SignedDecimal => {
            if matches!(input.get(0), Some(b'-')) {
                input_idx += 1;
                if max_width == 1 {
                    return Err(None)
                }
            }

            // TODO: are leading 0s allowed?
            if !matches!(input.get(input_idx), Some(b'0'..=b'9')) {
                return Err(None)
            }

            input_idx += 1;
            let mut param_success = false;

            while input_idx < input.len() && input_idx < max_width {
                let c = input[input_idx];
                input_idx += 1;
                if c < b'0' || c > b'9' {
                    param_success = true;
                    break;
                }
            }

            if !param_success && input_idx < max_width {
                return Err(Some(param_info.min_length()))
            }
        }
        ParamType::SignedInt => {
            match (input.get(0), input.get(1)) {
                (Some(b'-'), _) => {
                    let mut param_success = false;
                    input_idx += 1;

                    while input_idx < input.len() && input_idx < max_width {
                        let c = input[input_idx];
                        if c < b'0' || c > b'9' {
                            param_success = true;
                            break;
                        }
                        input_idx += 1;
                    }

                    if !param_success && input_idx < max_width {
                        return Err(Some(param_info.min_length()))
                    }
                }
                (Some(b'0'), _) if max_width == 1 => {
                    input_idx += 1;
                }
                (Some(b'0'), Some(b'x' | b'X')) if max_width == 2 => return Err(None),
                (Some(b'0'), Some(b'x' | b'X')) => {
                    let mut param_success = false;
                    input_idx += 2;

                    while input_idx < input.len() && input_idx < max_width {
                        let c = input[input_idx];
                        match c {
                            b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => (),
                            _ => {
                                param_success = true;
                                break;
                            }
                        }
                        input_idx += 1;
                    }

                    if !param_success && input_idx < max_width {
                        return Err(Some(param_info.min_length()))
                    }
                }
                (Some(b'0'), _) => {
                    let mut param_success = false;
                    input_idx += 1;

                    while input_idx < input.len() && input_idx < max_width {
                        let c = input[input_idx];
                        match c {
                            b'0'..=b'7' => input_idx += 1,
                            b'8'..=b'9' => return Err(None), // Non-octal characters
                            _ => {
                                param_success = true;
                                break;
                            }
                        }
                    }

                    if !param_success && input_idx < max_width {
                        return Err(Some(param_info.min_length()))
                    }
                }
                (Some(b'1'..=b'9'), _) => {
                    let mut param_success = false;
                    input_idx += 1;

                    while input_idx < input.len() && input_idx < max_width {
                        let c = input[input_idx];
                        match c {
                            b'0'..=b'9' => input_idx += 1,
                            _ => {
                                param_success = true;
                                break;
                            }
                        }
                    }

                    if !param_success && input_idx < max_width {
                        return Err(Some(param_info.min_length()))
                    }
                }
                (Some(_), _) => return Err(None),
                (None, _) => return Err(Some(param_info.min_length())),
            }

            if matches!(input.get(input_idx), Some(b'-')) {
                input_idx += 1;
            }
        }
        ParamType::UnsignedOctInt => {
            // TODO: are leading 0s allowed?
            if !matches!(input.get(input_idx), Some(b'0'..=b'7')) {
                return Err(None)
            }

            input_idx += 1;
            let mut param_success = false;

            while input_idx < input.len() && input_idx < max_width {
                let c = input[input_idx];
                if c < b'0' || c > b'7' {
                    param_success = true;
                    break;
                }

                input_idx += 1;
            }

            if !param_success && input_idx < max_width {
                return Err(Some(param_info.min_length()))
            }
        }
        ParamType::UnsignedDecimalInt => {
            // TODO: are leading 0s allowed?
            if !matches!(input.get(input_idx), Some(b'0'..=b'9')) {
                return Err(Some(param_info.min_length()))
            }

            input_idx += 1;
            let mut param_success = false;

            while input_idx < input.len() && input_idx < max_width {
                let c = input[input_idx];
                if c < b'0' || c > b'9' {
                    param_success = true;
                    break;
                }

                input_idx += 1;
            }

            if !param_success && input_idx < max_width {
                return Err(Some(param_info.min_length()))
            }
        }
        ParamType::UnsignedHexInt => {
            match (input.get(0), input.get(1)) {
                (Some(b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F'), _) if max_width == 1 => {
                    input_idx += 1;
                }
                (Some(b'0'), Some(b'x' | b'X')) if max_width == 2 => return Err(None),
                (Some(b'0'), Some(b'x' | b'X')) => {
                    let mut param_success = false;
                    input_idx += 2;

                    while input_idx < input.len() && input_idx < max_width {
                        let c = input[input_idx];
                        match c {
                            b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => (),
                            _ => {
                                param_success = true;
                                break;
                            }
                        }
                        input_idx += 1;
                    }

                    if !param_success && input_idx < max_width {
                        return Err(Some(param_info.min_length()))
                    }
                }
                (Some(b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F'), _) => {
                    let mut param_success = false;
                    input_idx += 1;

                    while input_idx < input.len() && input_idx < max_width {
                        let c = input[input_idx];
                        match c {
                            b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => (),
                            _ => {
                                param_success = true;
                                break;
                            }
                        }
                        input_idx += 1;
                    }

                    if !param_success && input_idx < max_width {
                        return Err(Some(param_info.min_length()))
                    }
                }
                _ => return Err(None),
            }
        }
        ParamType::Float => {
            if matches!(input.get(0), Some(b'-')) {
                input_idx += 1;
                if max_width == 1 {
                    return Err(None)
                }
            }

            // TODO: are leading 0s allowed?
            if !matches!(input.get(input_idx), Some(b'0'..=b'9')) {
                return Err(None)
            }

            input_idx += 1;
            let mut param_success = false;

            while input_idx < input.len() && input_idx < max_width {
                let c = input[input_idx];
                if (c < b'0' || c > b'9') && c != b'.' { // TODO: what about multiple dots? ending exponent?
                    param_success = true;
                    break;
                }

                input_idx += 1;
            }

            if !param_success && input_idx < max_width {
                return Err(Some(param_info.min_length()))
            }
        }
        ParamType::Sequence => {
            let mut param_success = false;

            while input_idx < input.len() && input_idx < max_width {
                let c = input[input_idx];
                if unsafe { libc::isspace(c as libc::c_int) } != 0 {
                    param_success = true;
                    break
                }

                input_idx += 1;
            }

            if !param_success && input_idx < max_width {
                return Err(Some(param_info.min_length()))
            }
        }
        ParamType::CSequence => {
            if input.len() < max_width {
                return Err(Some(param_info.min_length() - input.len()))
            }
            input_idx += max_width;
        }
        ParamType::Charset(items) => {
            let mut param_success = false;

            while input_idx < input.len() && input_idx < max_width {
                let mut matched_charset = false;
                let c = input[input_idx];

                let mut i = 0;
                // TODO: need to implement hyphen parsing here too...
                while i < items.len() {
                    /*
                    if items[i] == b'\\' && (i + 1 < items.len()) {
                        i += 1;
                        match (items[i], c) {
                            (b'a', 0x07) | (b'b', 0x08) | (b'e', 0x1b) | (b'f', 0x0c) | (b'n', 0x0a) | (b'r', 0x0d) | (b't', b'\t') | (b'v', 0x0b) | (b'\\', b'\\') | (b'\'', b'\'') | (b'\"', b'\"') | (b'?', b'?') => {
                                matched_charset = true;
                            }
                            _ => (),
                        }
                    } else 
                    */
                    if items[i] == b'-' && (i + 1 < items.len()) {
                        todo!("implement item ranges for Charset")
                    } else if c == items[i] {
                        matched_charset = true;
                        break
                    }

                    i += 1;
                }

                if !matched_charset {
                    // We've reached past the last character matching this charset
                    param_success = true;
                    break
                }

                input_idx += 1;
            }

            if !param_success && input_idx < max_width {
                return Err(Some(param_info.min_length()))
            }
        }
        ParamType::NotCharset(items) => {
            let mut matched_charset = false;

            while input_idx < input.len() && input_idx < max_width {
                let c = input[input_idx];

                let mut i = 0;
                // TODO: need to implement hyphen parsing here too...
                while i < items.len() {
                    /*
                    if items[i] == b'\\' && i+1 < items.len() {
                        i += 1;
                        match (items[i], c) {
                            (b'a', 0x07) | (b'b', 0x08) | (b'e', 0x1b) | (b'f', 0x0c) | (b'n', 0x0a) | (b'r', 0x0d) | (b't', b'\t') | (b'v', 0x0b) | (b'\\', b'\\') | (b'\'', b'\'') | (b'\"', b'\"') | (b'?', b'?') => {
                                matched_charset = true;
                            }
                            _ => (),
                        }
                    } else 
                    */
                    if items[i] == b'-' && (i + 1 < items.len()) {
                        todo!("implement item ranges for NotCharset")
                    } else if c == items[i] {
                        matched_charset = true;
                        break
                    }

                    i += 1;
                }

                if matched_charset {
                    break
                }

                input_idx += 1;
            }

            if !matched_charset && input_idx < max_width {
                return Err(Some(param_info.min_length()))
            }
        }
        ParamType::Pointer => todo!(),
        ParamType::Consumed => () // consumes 0 bytes
    }

    Ok(ParamSuccess {
        input_consumed: input_idx,
        format_consumed: param_info.consumed,
        assigned: !(param_info.masked || matches!(param_info.ty, ParamType::Percent)),
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn scanf(
    format: *const libc::c_char,
    va_args: ...
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function scanf() called within signal handler")
        }
    }
    
    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("scanf() unimplemented for Fizzle internal use");
    };

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("scanf(format={:?}) -> ...", format_cstr);

    let stream_ptr = FilePtr::from_raw(STDIN).unwrap();

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("scanf(format={:?}, ...) -> EOF ({})", format_cstr, Errno::get_errno());
    } else {
        crate::strace!("scanf(format={:?}, ...) -> {}", format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc99_scanf(
    format: *const libc::c_char,
    va_args: ...,
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function __isoc99_scanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc99_scanf() unimplemented for Fizzle internal use");
    };

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("__isoc99_scanf(format={:?}) -> ...", format_cstr);

    let stream_ptr = FilePtr::from_raw(STDIN).unwrap();

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("__isoc99_scanf(format={:?}, ...) -> EOF ({})", format_cstr, Errno::get_errno());
    } else {
        crate::strace!("__isoc99_scanf(format={:?}, ...) -> {}", format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_scanf(
    format: *const libc::c_char,
    va_args: ...,
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function __isoc23_scanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc23_scanf() unimplemented for Fizzle internal use");
    };


    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("__isoc23_scanf(format={:?}) -> ...", format_cstr);

    let stream_ptr = FilePtr::from_raw(STDIN).unwrap();

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("__isoc23_scanf(format={:?}, ...) -> EOF ({})", format_cstr, Errno::get_errno());
    } else {
        crate::strace!("__isoc23_scanf(format={:?}, ...) -> {}", format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    va_args: ...,
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function fscanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("fscanf() unimplemented for Fizzle internal use");
    };

    let stream_ptr = FilePtr::from_raw(stream).unwrap();

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("fscanf(stream={:?}, format={:?}) -> ...", stream, format_cstr);

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("fscanf(stream={:?}, format={:?}, ...) -> EOF ({})", stream, format_cstr, Errno::get_errno());
    } else {
        crate::strace!("fscanf(stream={:?}, format={:?}, ...) -> {}", stream, format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc99_fscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    va_args: ...,
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function __isoc99_fscanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc99_fscanf() unimplemented for Fizzle internal use");
    };

    let stream_ptr = FilePtr::from_raw(stream).unwrap();

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("__isoc99_fscanf(stream={:?}, format={:?}) -> ...", stream, format_cstr);

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                debug_assert!(buf_consumed <= buf.len());
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {
                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("__isoc99_fscanf(stream={:?}, format={:?}, ...) -> EOF ({})", stream, format_cstr, Errno::get_errno());
    } else {
        crate::strace!("__isoc99_fscanf(stream={:?}, format={:?}, ...) -> {}", stream, format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_fscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    va_args: ...,
) -> libc::c_int {
    
    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function __isoc23_fscanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc23_fscanf() unimplemented for Fizzle internal use");
    };

    let stream_ptr = FilePtr::from_raw(stream).unwrap();

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("__isoc23_fscanf(stream={:?}, format={:?}) -> ...", stream, format_cstr);

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("__isoc23_fscanf(stream={:?}, format={:?}, ...) -> EOF ({})", stream, format_cstr, Errno::get_errno());
    } else {
        crate::strace!("__isoc23_fscanf(stream={:?}, format={:?}, ...) -> {}", stream, format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vscanf(
    format: *const libc::c_char,
    va_args: VaList
) -> libc::c_int {
    
    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function vscanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vscanf() unimplemented for Fizzle internal use");
    };

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("vscanf(format={:?}) -> ...", format_cstr);

    let stream_ptr = FilePtr::from_raw(STDIN).unwrap();

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("vscanf(format={:?}, ...) -> EOF ({})", format_cstr, Errno::get_errno());
    } else {
        crate::strace!("vscanf(format={:?}, ...) -> {}", format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc99_vscanf(
    format: *const libc::c_char,
    va_args: VaList,
) -> libc::c_int {
    
    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function __isoc99_vscanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc99_vscanf() unimplemented for Fizzle internal use");
    };

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("__isoc99_vscanf(format={:?}) -> ...", format_cstr);

    let stream_ptr = FilePtr::from_raw(STDIN).unwrap();

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("__isoc99_vscanf(format={:?}, ...) -> EOF ({})", format_cstr, Errno::get_errno());
    } else {
        crate::strace!("__isoc99_vscanf(format={:?}, ...) -> {}", format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_vscanf(
    format: *const libc::c_char,
    va_args: VaList,
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function __isoc23_vscanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc23_vscanf() unimplemented for Fizzle internal use");
    };


    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("__isoc23_vscanf(format={:?}) -> ...", format_cstr);

    let stream_ptr = FilePtr::from_raw(STDIN).unwrap();

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("__isoc23_vscanf(format={:?}, ...) -> EOF ({})", format_cstr, Errno::get_errno());
    } else {
        crate::strace!("__isoc23_vscanf(format={:?}, ...) -> {}", format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn vfscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    va_args: VaList,
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function vfscanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("vfscanf() unimplemented for Fizzle internal use");
    };

    let stream_ptr = FilePtr::from_raw(stream).unwrap();

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("vfscanf(stream={:?}, format={:?}) -> ...", stream, format_cstr);

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("vfscanf(stream={:?}, format={:?}, ...) -> EOF ({})", stream, format_cstr, Errno::get_errno());
    } else {
        crate::strace!("vfscanf(stream={:?}, format={:?}, ...) -> {}", stream, format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc99_vfscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    va_args: VaList,
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function __isoc99_vfscanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc99_vfscanf() unimplemented for Fizzle internal use");
    };

    let stream_ptr = FilePtr::from_raw(stream).unwrap();

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("__isoc99_vfscanf(stream={:?}, format={:?}) -> ...", stream, format_cstr);

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("__isoc99_vfscanf(stream={:?}, format={:?}, ...) -> EOF ({})", stream, format_cstr, Errno::get_errno());
    } else {
        crate::strace!("__isoc99_vfscanf(stream={:?}, format={:?}, ...) -> {}", stream, format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_vfscanf(
    stream: *mut libc::FILE,
    format: *const libc::c_char,
    va_args: VaList,
) -> libc::c_int {

    #[cfg(feature = "sigsan")] {
        if in_sighandler() {
            panic!("async-signal-unsafe function __isoc23_vfscanf() called within signal handler")
        }
    }

    let Some(mut ctx) = crate::hooks::pre_hook() else {
        panic!("__isoc23_vfscanf() unimplemented for Fizzle internal use");
    };

    let stream_ptr = FilePtr::from_raw(stream).unwrap();

    let format_cstr = CStr::from_ptr(format);
    let mut format_bytes = format_cstr.to_bytes();
    crate::strace!("__isoc23_vfscanf(stream={:?}, format={:?}) -> ...", stream, format_cstr);

    let mut buf = Vec::new();
    let mut buf_consumed = 0;
    let mut total_matched = 0;

    let res = loop {
        match scan_incremental(format_bytes, &buf[buf_consumed..], &mut []) {
            Ok(consumed) => {
                buf_consumed += consumed;
                // We have all the bytes we need--now actually scan into va_args

                let res = vsscanf(buf.as_ptr().cast(), format, va_args);
                if res == libc::EOF {
                    panic!("libc and internal vsscanf() implementations in disagreement");
                }

                if buf_consumed < buf.len() {
                    // Need to pushback a character
                    assert_eq!(buf_consumed + 1, buf.len());

                    match Scheduler::handle_event(&mut ctx, StreamUngetEvent::new(stream_ptr, buf[buf_consumed], false)) {
                        Ok(()) => (),
                        Err(()) => unreachable!(),
                    }
                }

                break res
            }
            Err(MatchFailure::Truncated { matched, input_consumed, format_consumed, min_remainder }) => {
                total_matched += matched;
                format_bytes = &format_bytes[format_consumed..];
                buf_consumed += input_consumed;
                let prev_end = buf.len();
                buf.extend(std::iter::repeat(0).take(min_remainder));

                let read = match Scheduler::handle_event(&mut ctx, StreamReadEvent::new(stream_ptr, &mut buf[prev_end..prev_end + min_remainder], 1,  false, false)) {
                    Ok(read) => read,
                    Err(read) => {
                        let e = Errno::get_errno();
                        if e != Errno::SUCCESS || (read == 0 && total_matched == 0) {
                            break libc::EOF
                        }

                        if read == 0 {
                            break total_matched as libc::c_int
                        }

                        read
                    }
                };

                // If the read didn't receive everything
                for _ in read..min_remainder {
                    buf.pop();
                }

                // Now time to retry through the loop
            }
            Err(MatchFailure::BadInput(written)) => if written == 0 {

                Errno::EILSEQ.set_errno();
                break libc::EOF
            } else {
                break written as libc::c_int
            }
        }
    };

    if res == libc::EOF {
        crate::strace!("__isoc23_vfscanf(stream={:?}, format={:?}, ...) -> EOF ({})", stream, format_cstr, Errno::get_errno());
    } else {
        crate::strace!("__isoc23_vfscanf(stream={:?}, format={:?}, ...) -> {}", stream, format_cstr, res);
    }

    crate::hooks::post_hook();
    res
}
