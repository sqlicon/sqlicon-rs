//! Parameter binding types for Informix Prepared Statements.

use crate::error::Result;
use crate::types::column_type::ColumnType;
use crate::types::date::{date_encode_wire, Date};
use crate::types::datetime::{datetime_encode_wire, interval_encode_wire, DateTime, Interval};
use crate::types::decimal::{decimal_encode_wire, Decimal};
use bytes::{BufMut, BytesMut};

/// Parameter values that can be bound to a PreparedStatement.
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterValue {
    Null,
    Int(i32),
    BigInt(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Date(Date),
    Decimal(Decimal, u16),   // value, qualifier
    DateTime(DateTime, u16), // value, qualifier
    Interval(Interval, u16), // value, qualifier
    /// Smart Large Object (BLOB / CLOB) locator parameter: (is_clob, locator_bytes)
    Sblob(bool, Vec<u8>),
}


impl ParameterValue {
    pub fn wire_type(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Int(_) => 2,     // SQLI_TYPE_INT
            Self::BigInt(_) => 52, // SQLI_TYPE_BIGINT
            Self::Float(_) => 3,   // SQLI_TYPE_FLOAT
            Self::String(_) => 13, // SQLI_TYPE_VARCHAR
            Self::Bytes(_) => 11,  // SQLI_TYPE_BYTE
            Self::Date(_) => 7,    // SQLI_TYPE_DATE
            Self::Decimal(_, _) => 5, // SQLI_TYPE_DECIMAL
            Self::DateTime(_, _) => 10, // SQLI_TYPE_DATETIME
            Self::Interval(_, _) => 14, // SQLI_TYPE_INTERVAL
            Self::Sblob(_, _) => 44,    // SQLI_TYPE_BLOB / SQLI_TYPE_CLOB wire type 44
        }
    }


    pub fn encode_into(&self, buf: &mut BytesMut) -> Result<()> {
        self.encode_into_with_locale(buf, None)
    }

    /// Returns `true` when binding this value against `server_type` requires `libsqli`'s
    /// legacy BYTE/TEXT wire override: the server-declared placeholder type is overridden
    /// onto the wire regardless of this value's own type, matching `sqli_prepare.c`'s
    /// `build_bind_msg` (`is_legacy_lob`). Native DATE/DECIMAL/DATETIME/INTERVAL values keep
    /// their own encoding even against a legacy-typed placeholder — this mirrors `libsqli`
    /// exactly, including the resulting quirk that binding e.g. an `Int` against a BYTE
    /// placeholder still overrides the wire type but sends an empty (zero-length) 56-byte
    /// descriptor, since only `Bytes`/`String` values carry data for that path.
    fn is_legacy_lob_override(&self, server_type: Option<ColumnType>) -> bool {
        matches!(server_type, Some(ColumnType::Byte) | Some(ColumnType::Text))
            && !matches!(
                self,
                Self::DateTime(_, _) | Self::Interval(_, _) | Self::Date(_) | Self::Decimal(_, _)
            )
    }

    /// Returns `true` if, after the main `SQ_BIND` message, this parameter's actual bytes
    /// must be streamed via a follow-up `SQ_BBIND`/chunk sequence (`libsqli`'s
    /// `sqli_stmt_param_needs_lob_streaming`): a `Bytes` or `String` value bound against a
    /// server-declared legacy BYTE/TEXT placeholder.
    pub fn needs_lob_streaming(&self, server_type: Option<ColumnType>) -> bool {
        self.is_legacy_lob_override(server_type) && matches!(self, Self::Bytes(_) | Self::String(_))
    }

    /// Returns the raw bytes to stream via `SQ_BBIND` chunks for a parameter for which
    /// [`needs_lob_streaming`](Self::needs_lob_streaming) returned `true`. A `String` bound
    /// against `SQLI_TYPE_TEXT` is locale-converted first (matching `sqli_conn_encode_client_to_db`
    /// in `sqli_prepare.c`); against `SQLI_TYPE_BYTE` it is sent as its raw UTF-8 bytes
    /// (matching the C library's plain `strlen`/pointer send with no locale conversion).
    pub fn lob_stream_bytes(&self, db_locale: Option<&str>, server_type: ColumnType) -> Result<Vec<u8>> {
        match self {
            Self::Bytes(b) => Ok(b.clone()),
            Self::String(s) => {
                if server_type == ColumnType::Text {
                    crate::types::charset::encode_string_to_db(s, db_locale)
                } else {
                    Ok(s.as_bytes().to_vec())
                }
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Encodes this parameter's `SQ_BIND` entry (type + null indicator + encoded length +
    /// data), applying `libsqli`'s legacy BYTE/TEXT 56-byte descriptor override when
    /// `server_type` calls for it. Falls back to [`encode_into_with_locale`](Self::encode_into_with_locale)
    /// otherwise.
    pub fn encode_bind_entry(
        &self,
        buf: &mut BytesMut,
        db_locale: Option<&str>,
        server_type: Option<ColumnType>,
    ) -> Result<()> {
        if !self.is_legacy_lob_override(server_type) {
            return self.encode_into_with_locale(buf, db_locale);
        }

        // Wire type is always the server-declared type here, matching `wire_type =
        // is_legacy_lob ? server_type : par->type` in build_bind_msg -- even for a value
        // that ultimately encodes an empty/zero-length descriptor below. `server_type` is
        // guaranteed `Some` here: `is_legacy_lob_override` only returns `true` for
        // `Some(ColumnType::Byte) | Some(ColumnType::Text)`, checked just above.
        buf.put_u16(server_type.expect("is_legacy_lob_override guarantees Some") as u16);

        match self {
            Self::Null => {
                buf.put_u16(0xFFFF); // null_indicator = -1
                buf.put_u16(0); // encoded_length = 0
            }
            Self::Bytes(b) => {
                buf.put_u16(0); // null_indicator = 0
                buf.put_u16(56); // encoded_length = 56
                let mut desc = [0u8; 56];
                desc[16..20].copy_from_slice(&(b.len() as u32).to_be_bytes());
                buf.put_slice(&desc);
            }
            Self::String(s) => {
                buf.put_u16(0);
                buf.put_u16(56);
                let total_len = if server_type == Some(ColumnType::Text) {
                    crate::types::charset::encode_string_to_db(s, db_locale)?.len() as u32
                } else {
                    s.len() as u32
                };
                let mut desc = [0u8; 56];
                desc[16..20].copy_from_slice(&total_len.to_be_bytes());
                buf.put_slice(&desc);
            }
            // Any other bound type against a legacy BYTE/TEXT placeholder: matches
            // `libsqli`'s fall-through (total_len stays 0, no streaming data follows).
            _ => {
                buf.put_u16(0);
                buf.put_u16(56);
                buf.put_slice(&[0u8; 56]);
            }
        }
        Ok(())
    }

    pub fn encode_into_with_locale(&self, buf: &mut BytesMut, db_locale: Option<&str>) -> Result<()> {
        match self {
            Self::Null => {
                buf.put_u16(0); // type 0
                buf.put_u16(0xFFFF); // null indicator = -1
                buf.put_u16(0); // encoded_length = 0
            }
            Self::Int(v) => {
                buf.put_u16(2); // type INT
                buf.put_u16(0); // null indicator = 0
                buf.put_u16(0); // encoded_length = 0
                buf.put_i32(*v);
            }
            Self::BigInt(v) => {
                buf.put_u16(52); // type BIGINT
                buf.put_u16(0);
                buf.put_u16(0);
                buf.put_i64(*v);
            }
            Self::Float(v) => {
                buf.put_u16(3); // type FLOAT
                buf.put_u16(0);
                buf.put_u16(0);
                buf.put_f64(*v);
            }
            Self::String(s) => {
                buf.put_u16(13); // type VARCHAR
                buf.put_u16(0);
                let encoded_bytes = crate::types::charset::encode_string_to_db(s, db_locale)?;
                let slen = encoded_bytes.len() as u16;
                buf.put_u16(slen);
                buf.put_u16(slen);
                buf.put_slice(&encoded_bytes);
                if (2 + slen as usize) % 2 != 0 {
                    buf.put_u8(0);
                }
            }
            Self::Bytes(b) => {
                buf.put_u16(11); // type BYTE
                buf.put_u16(0);
                let blen = b.len() as u16;
                buf.put_u16(blen);
                buf.put_slice(b);
                if blen % 2 != 0 {
                    buf.put_u8(0);
                }
            }
            Self::Date(d) => {
                buf.put_u16(7); // type DATE
                buf.put_u16(0);
                buf.put_u16(0);
                let mut dbuf = [0u8; 4];
                let _ = date_encode_wire(d, &mut dbuf);
                buf.put_slice(&dbuf);
            }
            Self::Decimal(dec, qualifier) => {
                buf.put_u16(5); // type DECIMAL
                buf.put_u16(0);
                buf.put_u16(*qualifier);
                let mut out = [0u8; 18];
                if let Ok(mut len) = decimal_encode_wire(dec, *qualifier, &mut out) {
                    while len > 1 && out[len - 1] == 0 {
                        len -= 1;
                    }
                    buf.put_u16(len as u16);
                    buf.put_slice(&out[..len]);
                    if len % 2 != 0 {
                        buf.put_u8(0);
                    }
                } else {
                    buf.put_u16(0);
                }
            }
            Self::DateTime(dt, qualifier) => {
                buf.put_u16(10); // type DATETIME
                buf.put_u16(0);
                buf.put_u16(*qualifier);
                let mut out = [0u8; 12];
                if let Ok(mut len) = datetime_encode_wire(dt, *qualifier, &mut out) {
                    while len > 1 && out[len - 1] == 0 {
                        len -= 1;
                    }
                    buf.put_u16(len as u16);
                    buf.put_slice(&out[..len]);
                    if len % 2 != 0 {
                        buf.put_u8(0);
                    }
                } else {
                    buf.put_u16(0);
                }
            }
            Self::Interval(it, qualifier) => {
                buf.put_u16(14); // type INTERVAL
                buf.put_u16(0);
                buf.put_u16(*qualifier);
                let mut out = [0u8; 12];
                if let Ok(mut len) = interval_encode_wire(it, *qualifier, &mut out) {
                    while len > 1 && out[len - 1] == 0 {
                        len -= 1;
                    }
                    buf.put_u16(len as u16);
                    buf.put_slice(&out[..len]);
                    if len % 2 != 0 {
                        buf.put_u8(0);
                    }
                } else {
                    buf.put_u16(0);
                }
            }
            Self::Sblob(is_clob, locator_bytes) => {
                buf.put_u16(44); // type 44
                buf.put_u16(0);  // owner length = 0
                let tname = if *is_clob { "clob" } else { "blob" };
                let tnlen = tname.len() as u16;
                buf.put_u16(tnlen);
                buf.put_slice(tname.as_bytes());
                if (2 + tnlen as usize) % 2 != 0 {
                    buf.put_u8(0);
                }
                buf.put_u16(0); // null indicator = 0
                buf.put_u16(0); // encoded_length = 0
                let blen = locator_bytes.len() as u32;
                buf.put_u32(blen);
                buf.put_slice(locator_bytes);
                if (4 + blen as usize) % 2 != 0 {
                    buf.put_u8(0);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod legacy_lob_bind_tests {
    use super::*;

    fn encode(p: &ParameterValue, server_type: Option<ColumnType>) -> Vec<u8> {
        let mut buf = BytesMut::new();
        p.encode_bind_entry(&mut buf, None, server_type).unwrap();
        buf.to_vec()
    }

    // Layout derived from sqli_prepare.c's build_bind_msg: type(2) + null_indicator(2) +
    // encoded_length(2) + [56-byte descriptor with total_len BE-u32 at offset 16 of the
    // descriptor, i.e. byte 6+16=22 of the whole entry].
    fn descriptor_total_len(entry: &[u8]) -> u32 {
        u32::from_be_bytes(entry[22..26].try_into().unwrap())
    }

    #[test]
    fn bytes_against_byte_placeholder_sends_56_byte_descriptor_with_total_len() {
        let p = ParameterValue::Bytes(vec![1, 2, 3, 4, 5]);
        let entry = encode(&p, Some(ColumnType::Byte));
        assert_eq!(entry.len(), 6 + 56);
        assert_eq!(u16::from_be_bytes([entry[0], entry[1]]), ColumnType::Byte as u16);
        assert_eq!(u16::from_be_bytes([entry[2], entry[3]]), 0); // null_indicator = 0
        assert_eq!(u16::from_be_bytes([entry[4], entry[5]]), 56); // encoded_length = 56
        assert_eq!(descriptor_total_len(&entry), 5);
        assert!(entry[6..6 + 56].iter().enumerate().all(|(i, &b)| i == 16 || i == 17 || i == 18 || i == 19 || b == 0));
    }

    #[test]
    fn string_against_text_placeholder_uses_locale_encoded_length() {
        let p = ParameterValue::String("hello".to_string());
        let entry = encode(&p, Some(ColumnType::Text));
        assert_eq!(u16::from_be_bytes([entry[0], entry[1]]), ColumnType::Text as u16);
        assert_eq!(descriptor_total_len(&entry), 5);
        assert!(p.needs_lob_streaming(Some(ColumnType::Text)));
        assert_eq!(p.lob_stream_bytes(None, ColumnType::Text).unwrap(), b"hello");
    }

    #[test]
    fn null_against_legacy_placeholder_overrides_wire_type_but_stays_null() {
        let entry = encode(&ParameterValue::Null, Some(ColumnType::Byte));
        assert_eq!(u16::from_be_bytes([entry[0], entry[1]]), ColumnType::Byte as u16);
        assert_eq!(u16::from_be_bytes([entry[2], entry[3]]), 0xFFFF); // null_indicator = -1
        assert_eq!(u16::from_be_bytes([entry[4], entry[5]]), 0); // encoded_length = 0
        assert_eq!(entry.len(), 6);
    }

    #[test]
    fn date_is_not_overridden_by_legacy_placeholder() {
        // DATE/DECIMAL/DATETIME/INTERVAL keep their own encoding even against a
        // server-declared BYTE/TEXT placeholder, matching build_bind_msg's exclusion list.
        let p = ParameterValue::Date(Date::new(2000, 1, 1).unwrap());
        assert!(!p.needs_lob_streaming(Some(ColumnType::Byte)));
        let entry = encode(&p, Some(ColumnType::Byte));
        // Falls through to the normal DATE encoding (type=7), not a 56-byte descriptor.
        assert_eq!(u16::from_be_bytes([entry[0], entry[1]]), 7);
        assert_eq!(entry.len(), 6 + 4);
    }

    #[test]
    fn non_stream_value_against_legacy_placeholder_sends_empty_descriptor() {
        // Matches libsqli's own quirk: is_legacy_lob_override doesn't exclude Int/BigInt/
        // Float, so binding e.g. an Int against a BYTE placeholder still overrides the wire
        // type but sends a zero-length descriptor (no data to stream).
        let p = ParameterValue::Float(1.5);
        assert!(!p.needs_lob_streaming(Some(ColumnType::Byte)));
        let entry = encode(&p, Some(ColumnType::Byte));
        assert_eq!(u16::from_be_bytes([entry[0], entry[1]]), ColumnType::Byte as u16);
        assert_eq!(descriptor_total_len(&entry), 0);
    }

    #[test]
    fn no_server_type_falls_back_to_default_encoding() {
        let p = ParameterValue::Bytes(vec![9, 9]);
        let entry = encode(&p, None);
        assert_eq!(u16::from_be_bytes([entry[0], entry[1]]), 11); // SQLI_TYPE_BYTE default
        assert_ne!(entry.len(), 6 + 56); // not the 56-byte descriptor path
    }
}

