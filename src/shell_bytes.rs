//! Helpers for representing shell byte streams inside Rust `String`s.
//!
//! Valid UTF-8 sequences are decoded normally. Bytes that can't be represented
//! as UTF-8 at their current position are mapped to the Unicode private-use
//! block so shell expansions can keep operating on a `String` while still
//! preserving the original byte stream for length calculations and binary-aware
//! commands like `od`.

const SHELL_BYTE_MARKER_BASE: u32 = 0xE000;

fn marker_for_byte(byte: u8) -> char {
    char::from_u32(SHELL_BYTE_MARKER_BASE + byte as u32).unwrap()
}

pub(crate) fn marker_byte(ch: char) -> Option<u8> {
    let code = ch as u32;
    if (SHELL_BYTE_MARKER_BASE..=SHELL_BYTE_MARKER_BASE + 0xFF).contains(&code) {
        Some((code - SHELL_BYTE_MARKER_BASE) as u8)
    } else {
        None
    }
}

pub(crate) fn contains_markers(s: &str) -> bool {
    s.chars().any(|ch| marker_byte(ch).is_some())
}

pub(crate) fn encode_shell_string(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.chars() {
        if let Some(byte) = marker_byte(ch) {
            out.push(byte);
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

pub(crate) fn decode_shell_bytes(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0usize;

    while i < bytes.len() {
        match std::str::from_utf8(&bytes[i..]) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(err) => {
                let valid = err.valid_up_to();
                if valid > 0 {
                    out.push_str(std::str::from_utf8(&bytes[i..i + valid]).unwrap());
                    i += valid;
                    continue;
                }

                match err.error_len() {
                    Some(invalid_len) => {
                        for &byte in &bytes[i..i + invalid_len] {
                            if byte.is_ascii() {
                                out.push(byte as char);
                            } else {
                                out.push(marker_for_byte(byte));
                            }
                        }
                        i += invalid_len;
                    }
                    None => {
                        for &byte in &bytes[i..] {
                            if byte.is_ascii() {
                                out.push(byte as char);
                            } else {
                                out.push(marker_for_byte(byte));
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    out
}
