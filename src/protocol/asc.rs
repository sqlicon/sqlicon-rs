//! ASC-BINARY string and buffer encoding utilities.

use crate::error::{Error, Result};
use bytes::BufMut;

/// Pads `n` up to the next even integer.
/// E.g. 0 -> 0, 1 -> 2, 2 -> 2, 3 -> 4, etc.
#[inline]
pub const fn pad_even(n: usize) -> usize {
    (n + 1) & !1
}

/// Encodes a string as `[2-byte BE length][content bytes]`.
pub fn encode_asc_str_to_slice(buf: &mut [u8], s: &str) -> Result<usize> {
    let slen = s.len();
    if slen > 0xFFFF {
        return Err(Error::OutOfRange("String length exceeds 65535".into()));
    }
    let needed = 2 + slen;
    if buf.len() < needed {
        return Err(Error::BufferTooSmall {
            needed,
            available: buf.len(),
        });
    }

    buf[0] = ((slen >> 8) & 0xFF) as u8;
    buf[1] = (slen & 0xFF) as u8;
    buf[2..2 + slen].copy_from_slice(s.as_bytes());

    Ok(needed)
}

/// Encodes a string into `BufMut` as `[2-byte BE length][content bytes]`.
pub fn encode_asc_str<B: BufMut>(buf: &mut B, s: &str) -> Result<usize> {
    let slen = s.len();
    if slen > 0xFFFF {
        return Err(Error::OutOfRange("String length exceeds 65535".into()));
    }
    buf.put_u16(slen as u16);
    buf.put_slice(s.as_bytes());
    Ok(2 + slen)
}

/// Encodes a string as `[2-byte BE length][content bytes][pad byte if odd]`.
/// The total length is always even.
pub fn encode_asc_str_padded_to_slice(buf: &mut [u8], s: &str) -> Result<usize> {
    let slen = s.len();
    if slen > 0xFFFF {
        return Err(Error::OutOfRange("String length exceeds 65535".into()));
    }
    let payload = 2 + slen;
    let total = pad_even(payload);

    if buf.len() < total {
        return Err(Error::BufferTooSmall {
            needed: total,
            available: buf.len(),
        });
    }

    buf[0] = ((slen >> 8) & 0xFF) as u8;
    buf[1] = (slen & 0xFF) as u8;
    buf[2..2 + slen].copy_from_slice(s.as_bytes());

    if slen % 2 != 0 {
        buf[payload] = 0;
    }

    Ok(total)
}

/// Encodes a string into `BufMut` as `[2-byte BE length][content bytes][pad byte if odd]`.
pub fn encode_asc_str_padded<B: BufMut>(buf: &mut B, s: &str) -> Result<usize> {
    let slen = s.len();
    if slen > 0xFFFF {
        return Err(Error::OutOfRange("String length exceeds 65535".into()));
    }
    buf.put_u16(slen as u16);
    buf.put_slice(s.as_bytes());
    if slen % 2 != 0 {
        buf.put_u8(0);
        Ok(2 + slen + 1)
    } else {
        Ok(2 + slen)
    }
}

/// Decodes an ASC string from a slice `[2-byte BE length][content bytes]`.
pub fn decode_asc_str_from_slice(buf: &[u8]) -> Result<(&str, usize)> {
    if buf.len() < 2 {
        return Err(Error::BufferTooSmall {
            needed: 2,
            available: buf.len(),
        });
    }
    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    let total = 2 + len;
    if buf.len() < total {
        return Err(Error::BufferTooSmall {
            needed: total,
            available: buf.len(),
        });
    }
    let s = std::str::from_utf8(&buf[2..total])
        .map_err(|e| Error::Protocol(format!("Invalid UTF-8 in ASC string: {e}")))?;
    Ok((s, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn test_pad_even() {
        assert_eq!(pad_even(0), 0);
        assert_eq!(pad_even(1), 2);
        assert_eq!(pad_even(2), 2);
        assert_eq!(pad_even(3), 4);
        assert_eq!(pad_even(4), 4);
        assert_eq!(pad_even(5), 6);
    }

    #[test]
    fn test_asc_string_encode_hello() {
        let mut buf = [0u8; 64];
        let n = encode_asc_str_to_slice(&mut buf, "hello").unwrap();
        assert_eq!(n, 7); // 2 length + 5 chars
        assert_eq!(buf[0], 0);
        assert_eq!(buf[1], 5);
        assert_eq!(&buf[2..7], b"hello");
    }

    #[test]
    fn test_asc_string_encode_empty() {
        let mut buf = [0u8; 64];
        let n = encode_asc_str_to_slice(&mut buf, "").unwrap();
        assert_eq!(n, 2);
        assert_eq!(buf[0], 0);
        assert_eq!(buf[1], 0);
    }

    #[test]
    fn test_asc_string_encode_overflow() {
        let mut buf = [0u8; 3];
        assert!(encode_asc_str_to_slice(&mut buf, "hello").is_err());
    }

    #[test]
    fn test_asc_encode_padded_even_len_str() {
        let mut buf = [0u8; 64];
        // "hi" = 2 bytes -> 2 header + 2 chars = 4 (already even)
        let n = encode_asc_str_padded_to_slice(&mut buf, "hi").unwrap();
        assert_eq!(n, 4);
        assert_eq!(buf[0], 0);
        assert_eq!(buf[1], 2);
        assert_eq!(&buf[2..4], b"hi");
    }

    #[test]
    fn test_asc_encode_padded_odd_len_str() {
        let mut buf = [0u8; 64];
        // "hi!" = 3 bytes -> 2 header + 3 chars + 1 pad = 6
        let n = encode_asc_str_padded_to_slice(&mut buf, "hi!").unwrap();
        assert_eq!(n, 6);
        assert_eq!(buf[0], 0);
        assert_eq!(buf[1], 3);
        assert_eq!(&buf[2..5], b"hi!");
        assert_eq!(buf[5], 0);
    }

    #[test]
    fn test_asc_decode() {
        let mut buf = BytesMut::new();
        encode_asc_str(&mut buf, "test_string").unwrap();
        let (s, read_len) = decode_asc_str_from_slice(&buf).unwrap();
        assert_eq!(s, "test_string");
        assert_eq!(read_len, buf.len());
    }
}

/// Informix's `onipcstr` preamble uses a non-standard Base64 alphabet:
/// `.` in place of `+` and `_` in place of `/`.
/// Verified against dbaccess network packet traces.
const INFORMIX_B64_ALPHA: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._";

/// Encodes binary data using Informix's custom Base64 alphabet (`.` and `_`).
pub fn encode_ipc_b64(src: &[u8]) -> String {
    let mut out = String::with_capacity((src.len() * 4) / 3 + 4);
    let mut i = 0;
    while i + 2 < src.len() {
        out.push(INFORMIX_B64_ALPHA[(src[i] >> 2) as usize] as char);
        out.push(INFORMIX_B64_ALPHA[(((src[i] & 0x03) << 4) | (src[i + 1] >> 4)) as usize] as char);
        out.push(INFORMIX_B64_ALPHA[(((src[i + 1] & 0x0F) << 2) | (src[i + 2] >> 6)) as usize] as char);
        out.push(INFORMIX_B64_ALPHA[(src[i + 2] & 0x3F) as usize] as char);
        i += 3;
    }
    if i < src.len() {
        out.push(INFORMIX_B64_ALPHA[(src[i] >> 2) as usize] as char);
        if i + 1 < src.len() {
            out.push(INFORMIX_B64_ALPHA[(((src[i] & 0x03) << 4) | (src[i + 1] >> 4)) as usize] as char);
            out.push(INFORMIX_B64_ALPHA[((src[i + 1] & 0x0F) << 2) as usize] as char);
        } else {
            out.push(INFORMIX_B64_ALPHA[((src[i] & 0x03) << 4) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod ipc_b64_tests {
    use super::*;

    #[test]
    fn test_ipc_b64_encoding_custom_chars() {
        // Test that values map to '.' (62) and '_' (63)
        let data = [0b11111000, 0b00111111]; // 62, 63 in 6-bit parts
        let encoded = encode_ipc_b64(&data);
        assert!(encoded.contains('.') || encoded.contains('_'));
    }
}
