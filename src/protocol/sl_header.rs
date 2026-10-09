//! Session Layer (SL) header encode and decode.

use crate::error::{Error, Result};
use crate::protocol::constants::SL_HEADER_SIZE;
use bytes::{Buf, BufMut};

/// Session Layer (SL) header structure.
/// Layout:
/// - bytes 0..2: pdu_size (BE u16)
/// - byte 2: sl_type (u8)
/// - byte 3: sl_attr (u8)
/// - bytes 4..6: sl_opts (BE u16)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlHeader {
    pub pdu_size: u16,
    pub sl_type: u8,
    pub sl_attr: u8,
    pub sl_opts: u16,
}

impl SlHeader {
    pub const SIZE: usize = SL_HEADER_SIZE;

    pub const fn new(pdu_size: u16, sl_type: u8, sl_attr: u8, sl_opts: u16) -> Self {
        Self {
            pdu_size,
            sl_type,
            sl_attr,
            sl_opts,
        }
    }

    /// Encodes the SL header into a byte slice. Returns number of bytes written (6).
    pub fn encode_to_slice(&self, buf: &mut [u8]) -> Result<usize> {
        if buf.len() < Self::SIZE {
            return Err(Error::BufferTooSmall {
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        buf[0] = ((self.pdu_size >> 8) & 0xFF) as u8;
        buf[1] = (self.pdu_size & 0xFF) as u8;
        buf[2] = self.sl_type;
        buf[3] = self.sl_attr;
        buf[4] = ((self.sl_opts >> 8) & 0xFF) as u8;
        buf[5] = (self.sl_opts & 0xFF) as u8;
        Ok(Self::SIZE)
    }

    /// Encodes the SL header using `BufMut`.
    pub fn encode<B: BufMut>(&self, buf: &mut B) {
        buf.put_u16(self.pdu_size);
        buf.put_u8(self.sl_type);
        buf.put_u8(self.sl_attr);
        buf.put_u16(self.sl_opts);
    }

    /// Decodes the SL header from a byte slice.
    pub fn decode_from_slice(buf: &[u8]) -> Result<Self> {
        if buf.len() < Self::SIZE {
            return Err(Error::BufferTooSmall {
                needed: Self::SIZE,
                available: buf.len(),
            });
        }
        let pdu_size = u16::from_be_bytes([buf[0], buf[1]]);
        let sl_type = buf[2];
        let sl_attr = buf[3];
        let sl_opts = u16::from_be_bytes([buf[4], buf[5]]);
        Ok(Self {
            pdu_size,
            sl_type,
            sl_attr,
            sl_opts,
        })
    }

    /// Decodes the SL header from `Buf`.
    pub fn decode<B: Buf>(buf: &mut B) -> Result<Self> {
        if buf.remaining() < Self::SIZE {
            return Err(Error::BufferTooSmall {
                needed: Self::SIZE,
                available: buf.remaining(),
            });
        }
        let pdu_size = buf.get_u16();
        let sl_type = buf.get_u8();
        let sl_attr = buf.get_u8();
        let sl_opts = buf.get_u16();
        Ok(Self {
            pdu_size,
            sl_type,
            sl_attr,
            sl_opts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sl_header_encode_decode() {
        let header = SlHeader::new(128, 1, 60, 0x1234);
        let mut buf = [0u8; 6];
        assert_eq!(header.encode_to_slice(&mut buf).unwrap(), 6);

        assert_eq!(buf[0], 0x00);
        assert_eq!(buf[1], 0x80);
        assert_eq!(buf[2], 1);
        assert_eq!(buf[3], 60);
        assert_eq!(buf[4], 0x12);
        assert_eq!(buf[5], 0x34);

        let decoded = SlHeader::decode_from_slice(&buf).unwrap();
        assert_eq!(decoded, header);
    }

    #[test]
    fn test_sl_header_buffer_too_small() {
        let mut buf = [0u8; 5];
        let header = SlHeader::new(10, 1, 2, 3);
        assert!(header.encode_to_slice(&mut buf).is_err());
        assert!(SlHeader::decode_from_slice(&buf).is_err());
    }
}
