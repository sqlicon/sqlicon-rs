//! Parameter binding types for Informix Prepared Statements.

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


    pub fn encode_into(&self, buf: &mut BytesMut) {
        self.encode_into_with_locale(buf, None);
    }

    pub fn encode_into_with_locale(&self, buf: &mut BytesMut, db_locale: Option<&str>) {
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
                let encoded_bytes = crate::types::charset::encode_string_to_db(s, db_locale);
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
    }
}

