//! Informix Result Row extraction and typed column accessors.

use crate::error::{Error, Result};
use crate::result::column::ColumnInfo;
use crate::types::column_type::ColumnType;
use crate::types::date::{date_decode_wire, Date};
use crate::types::datetime::{datetime_decode_wire, DateTime};
use crate::types::decimal::{decimal_decode_wire, decimal_wire_size, Decimal};
use crate::types::temporal_wire_size;

/// A single row in a query result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub(crate) tuple_data: Vec<u8>,
    pub(crate) columns: Vec<ColumnInfo>,
    pub(crate) col_offsets: Vec<usize>,
    pub(crate) db_locale: Option<String>,
}

impl Row {
    pub fn new(tuple_data: Vec<u8>, columns: Vec<ColumnInfo>) -> Self {
        Self::with_locale(tuple_data, columns, None)
    }

    pub fn with_locale(
        tuple_data: Vec<u8>,
        columns: Vec<ColumnInfo>,
        db_locale: Option<String>,
    ) -> Self {
        let col_offsets = Self::calculate_col_offsets(&tuple_data, &columns);
        Self {
            tuple_data,
            columns,
            col_offsets,
            db_locale,
        }
    }

    pub fn set_db_locale(&mut self, locale: Option<String>) {
        self.db_locale = locale;
    }

    fn calculate_col_offsets(tuple_data: &[u8], columns: &[ColumnInfo]) -> Vec<usize> {
        let mut offsets = Vec::with_capacity(columns.len());
        let mut offset = 0;

        for col in columns {
            offsets.push(offset);
            if offset >= tuple_data.len() {
                continue;
            }

            let span = match col.col_type {
                ColumnType::VarChar | ColumnType::NVarChar => {
                    let len = tuple_data[offset] as usize;
                    1 + len
                }
                ColumnType::Bool | ColumnType::DBoolean | ColumnType::LVarChar | ColumnType::Blob | ColumnType::Clob => {
                    if offset + 5 <= tuple_data.len() {
                        let len = u32::from_be_bytes([
                            tuple_data[offset + 1],
                            tuple_data[offset + 2],
                            tuple_data[offset + 3],
                            tuple_data[offset + 4],
                        ]) as usize;
                        5 + len
                    } else if offset + 2 <= tuple_data.len() {
                        let len = u16::from_be_bytes([tuple_data[offset], tuple_data[offset + 1]]) as usize;
                        2 + len
                    } else {
                        1
                    }
                }
                ColumnType::Char | ColumnType::NChar => {
                    let width = if col.encoded_length > 0 { col.encoded_length as usize } else { 1 };
                    if offset + width <= tuple_data.len() {
                        width
                    } else {
                        let len = tuple_data[offset] as usize;
                        1 + len
                    }
                }
                ColumnType::SmallInt => 2,
                ColumnType::Int | ColumnType::Serial | ColumnType::Date => 4,
                ColumnType::Float | ColumnType::BigInt | ColumnType::BigSerial => 8,
                ColumnType::Int8 | ColumnType::Serial8 => 10,
                ColumnType::Decimal | ColumnType::Money => {
                    decimal_wire_size(col.encoded_length as u16).unwrap_or(16)
                }
                ColumnType::DateTime => {
                    temporal_wire_size(col.encoded_length as u16, false).unwrap_or(8)
                }
                ColumnType::Interval => {
                    temporal_wire_size(col.encoded_length as u16, true).unwrap_or(8)
                }
                ColumnType::Byte | ColumnType::Text => {
                    if col.encoded_length > 0 { col.encoded_length as usize } else { 56 }
                }
                _ => {
                    let width = col.encoded_length as usize;
                    if width > 0 { width } else { 4 }
                }
            };

            offset += span;
        }

        offsets
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn column(&self, index: usize) -> Option<&ColumnInfo> {
        self.columns.get(index)
    }

    /// Locates the raw slice of a column in the row's tuple buffer.
    pub fn get_raw_column(&self, index: usize) -> Result<Option<&[u8]>> {
        let col = self.columns.get(index).ok_or_else(|| {
            Error::OutOfRange(format!("Column index {index} out of range (count: {})", self.columns.len()))
        })?;

        let base = self.col_offsets.get(index).copied().unwrap_or(col.col_start_pos as usize);
        let buf = &self.tuple_data;
        if base >= buf.len() {
            return Err(Error::Protocol(format!(
                "Column '{}' start pos {} exceeds tuple length {}",
                col.name, base, buf.len()
            )));
        }

        match col.col_type {
            ColumnType::Bool | ColumnType::DBoolean | ColumnType::LVarChar | ColumnType::Blob | ColumnType::Clob => {
                // 5-byte prefix [null_marker: u8][len: u32 BE]
                if base + 5 <= buf.len() {
                    let null_marker = buf[base];
                    let len = u32::from_be_bytes([buf[base + 1], buf[base + 2], buf[base + 3], buf[base + 4]]) as usize;
                    if null_marker != 0 && len == 0 {
                        return Ok(None);
                    }
                    if null_marker == 0 && base + 5 + len <= buf.len() {
                        return Ok(Some(&buf[base + 5..base + 5 + len]));
                    }
                }
                // Fallback to 2-byte prefix [len: u16 BE]
                if base + 2 <= buf.len() {
                    let len = u16::from_be_bytes([buf[base], buf[base + 1]]) as usize;
                    if base + 2 + len <= buf.len() {
                        return Ok(Some(&buf[base + 2..base + 2 + len]));
                    }
                }
                Err(Error::Protocol("Invalid variable column framing".into()))
            }
            ColumnType::VarChar | ColumnType::NVarChar => {
                // 1-byte length prefix
                let len = buf[base] as usize;
                if base + 1 + len <= buf.len() {
                    let payload = &buf[base + 1..base + 1 + len];
                    // Informix VARCHAR NULL sentinel: [len=1][payload=0x00]
                    if len == 1 && payload[0] == 0x00 {
                        Ok(None)
                    } else {
                        Ok(Some(payload))
                    }
                } else {
                    Err(Error::Protocol("VarChar length exceeds tuple bounds".into()))
                }
            }
            ColumnType::Decimal | ColumnType::Money => {
                let width = decimal_wire_size(col.encoded_length as u16)?;
                if base + width <= buf.len() {
                    let slice = &buf[base..base + width];
                    if slice.iter().all(|&b| b == 0) {
                        Ok(None)
                    } else {
                        Ok(Some(slice))
                    }
                } else {
                    Err(Error::Protocol("Decimal column exceeds tuple bounds".into()))
                }
            }
            ColumnType::DateTime => {
                let width = temporal_wire_size(col.encoded_length as u16, false)?;
                if base + width <= buf.len() {
                    let slice = &buf[base..base + width];
                    if slice.iter().all(|&b| b == 0) {
                        Ok(None)
                    } else {
                        Ok(Some(slice))
                    }
                } else {
                    Err(Error::Protocol("DateTime column exceeds tuple bounds".into()))
                }
            }
            ColumnType::Interval => {
                let width = temporal_wire_size(col.encoded_length as u16, true)?;
                if base + width <= buf.len() {
                    let slice = &buf[base..base + width];
                    if slice.iter().all(|&b| b == 0) {
                        Ok(None)
                    } else {
                        Ok(Some(slice))
                    }
                } else {
                    Err(Error::Protocol("Interval column exceeds tuple bounds".into()))
                }
            }
            ColumnType::Char | ColumnType::NChar => {
                let width = col.encoded_length as usize;
                if base + width <= buf.len() {
                    let slice = &buf[base..base + width];
                    // Informix CHAR NULL sentinel: first byte 0x00 and remaining bytes space padding
                    if width > 0 && slice[0] == 0x00 && slice[1..].iter().all(|&b| b == b' ') {
                        Ok(None)
                    } else {
                        Ok(Some(slice))
                    }
                } else {
                    Err(Error::Protocol("Char column exceeds tuple bounds".into()))
                }
            }
            ColumnType::SmallInt => {
                if base + 2 <= buf.len() {
                    let val = i16::from_be_bytes([buf[base], buf[base + 1]]);
                    if val == i16::MIN {
                        Ok(None)
                    } else {
                        Ok(Some(&buf[base..base + 2]))
                    }
                } else {
                    Err(Error::Protocol("SmallInt column exceeds tuple bounds".into()))
                }
            }
            ColumnType::Int | ColumnType::Serial => {
                if base + 4 <= buf.len() {
                    let val = i32::from_be_bytes([buf[base], buf[base + 1], buf[base + 2], buf[base + 3]]);
                    if val == i32::MIN {
                        Ok(None)
                    } else {
                        Ok(Some(&buf[base..base + 4]))
                    }
                } else {
                    Err(Error::Protocol("Int column exceeds tuple bounds".into()))
                }
            }
            ColumnType::Date => {
                if base + 4 <= buf.len() {
                    let val = u32::from_be_bytes([buf[base], buf[base + 1], buf[base + 2], buf[base + 3]]);
                    if val == crate::types::date::DATE_NULL_ENCODED {
                        Ok(None)
                    } else {
                        Ok(Some(&buf[base..base + 4]))
                    }
                } else {
                    Err(Error::Protocol("Date column exceeds tuple bounds".into()))
                }
            }
            ColumnType::BigInt | ColumnType::BigSerial => {
                if base + 8 <= buf.len() {
                    let val = i64::from_be_bytes([
                        buf[base], buf[base + 1], buf[base + 2], buf[base + 3],
                        buf[base + 4], buf[base + 5], buf[base + 6], buf[base + 7],
                    ]);
                    if val == i64::MIN {
                        Ok(None)
                    } else {
                        Ok(Some(&buf[base..base + 8]))
                    }
                } else {
                    Err(Error::Protocol("BigInt column exceeds tuple bounds".into()))
                }
            }
            ColumnType::Int8 | ColumnType::Serial8 => {
                if base + 10 <= buf.len() {
                    // INT8 NULL sentinel: first 2 bytes are 0
                    if buf[base] == 0 && buf[base + 1] == 0 {
                        Ok(None)
                    } else {
                        Ok(Some(&buf[base..base + 10]))
                    }
                } else {
                    Err(Error::Protocol("Int8 column exceeds tuple bounds".into()))
                }
            }
            ColumnType::Float => {
                if base + 8 <= buf.len() {
                    // IEEE 754 null sentinel: all 8 bytes 0xFF
                    if buf[base..base + 8].iter().all(|&b| b == 0xFF) {
                        Ok(None)
                    } else {
                        Ok(Some(&buf[base..base + 8]))
                    }
                } else {
                    Err(Error::Protocol("Float column exceeds tuple bounds".into()))
                }
            }
            ColumnType::SmallFloat => {
                if base + 4 <= buf.len() {
                    // IEEE 754 null sentinel: all 4 bytes 0xFF
                    if buf[base..base + 4].iter().all(|&b| b == 0xFF) {
                        Ok(None)
                    } else {
                        Ok(Some(&buf[base..base + 4]))
                    }
                } else {
                    Err(Error::Protocol("SmallFloat column exceeds tuple bounds".into()))
                }
            }
            ColumnType::Byte | ColumnType::Text => {
                // Byte / Text locator descriptor: 56 bytes. Null if bit 0 of byte 39 is set
                let len = if col.encoded_length > 0 { col.encoded_length as usize } else { 56 };
                if base + len <= buf.len() {
                    if len >= 40 && (buf[base + 39] & 0x01) != 0 {
                        Ok(None)
                    } else {
                        Ok(Some(&buf[base..base + len]))
                    }
                } else {
                    Err(Error::Protocol("Byte/Text column exceeds tuple bounds".into()))
                }
            }
            _ => {
                // Generic fallback: check width
                let width = col.encoded_length as usize;
                if width > 0 && base + width <= buf.len() {
                    Ok(Some(&buf[base..base + width]))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Returns the column value as a borrowed `&str` if valid UTF-8, or None if NULL.
    ///
    /// For non-UTF-8 database charsets (e.g. CP1252), prefer [`get_string`](Self::get_string)
    /// which performs full conversion into an owned UTF-8 `String`.
    pub fn get_str(&self, index: usize) -> Result<Option<&str>> {
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let s = std::str::from_utf8(raw)
            .map_err(|e| Error::Protocol(format!("Invalid UTF-8 in column {index}: {e}")))?;
        Ok(Some(s.trim_end()))
    }

    /// Returns the column value decoded into a Rust UTF-8 `String`, automatically converting
    /// from the server's character set (CP1252, ISO-8859-*, etc.) according to `DB_LOCALE`.
    pub fn get_string(&self, index: usize) -> Result<Option<String>> {
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };

        let decoded = crate::types::charset::decode_wire_string(raw, self.db_locale.as_deref());
        Ok(Some(decoded.trim_end().to_string()))
    }

    pub fn get_i16(&self, index: usize) -> Result<Option<i16>> {
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        if raw.len() != 2 {
            return Err(Error::TypeMismatch { expected: "i16", found: "non-2-byte" });
        }
        Ok(Some(i16::from_be_bytes([raw[0], raw[1]])))
    }

    pub fn get_i32(&self, index: usize) -> Result<Option<i32>> {
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        if raw.len() != 4 {
            return Err(Error::TypeMismatch { expected: "i32", found: "non-4-byte" });
        }
        Ok(Some(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])))
    }

    pub fn get_i64(&self, index: usize) -> Result<Option<i64>> {
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        if raw.len() == 8 {
            Ok(Some(i64::from_be_bytes([
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
            ])))
        } else if raw.len() == 4 {
            Ok(Some(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as i64))
        } else if raw.len() == 10 {
            // Informix INT8 / SERIAL8: [sign: 2 bytes BE][lo32: 4 bytes BE][hi32: 4 bytes BE]
            let sign = i16::from_be_bytes([raw[0], raw[1]]);
            if sign == 0 {
                return Ok(Some(0));
            }
            let lo = u32::from_be_bytes([raw[2], raw[3], raw[4], raw[5]]) as u64;
            let hi = u32::from_be_bytes([raw[6], raw[7], raw[8], raw[9]]) as u64;
            let mag = (hi << 32) | lo;
            if sign < 0 {
                if mag >= (i64::MAX as u64 + 1) {
                    Ok(Some(i64::MIN))
                } else {
                    Ok(Some(-(mag as i64)))
                }
            } else {
                Ok(Some(if mag > i64::MAX as u64 { i64::MAX } else { mag as i64 }))
            }
        } else {
            Err(Error::TypeMismatch { expected: "i64", found: "unexpected length" })
        }
    }

    pub fn get_f64(&self, index: usize) -> Result<Option<f64>> {
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        if raw.len() == 8 {
            Ok(Some(f64::from_be_bytes([
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
            ])))
        } else if raw.len() == 4 {
            let f = f32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
            Ok(Some(f as f64))
        } else {
            Err(Error::TypeMismatch { expected: "f64", found: "unexpected float length" })
        }
    }

    pub fn get_f32(&self, index: usize) -> Result<Option<f32>> {
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        if raw.len() == 4 {
            Ok(Some(f32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])))
        } else if raw.len() == 8 {
            let d = f64::from_be_bytes([
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
            ]);
            Ok(Some(d as f32))
        } else {
            Err(Error::TypeMismatch { expected: "f32", found: "unexpected float length" })
        }
    }

    pub fn get_bool(&self, index: usize) -> Result<Option<bool>> {
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        if raw.is_empty() {
            return Ok(None);
        }
        match raw[0] {
            0 | b'0' | b'f' | b'F' | b'n' | b'N' => Ok(Some(false)),
            1 | 0xFF | b'1' | b't' | b'T' | b'y' | b'Y' => Ok(Some(true)),
            _ => Ok(Some(raw[0] != 0)),
        }
    }

    pub fn get_interval(&self, index: usize) -> Result<Option<crate::types::datetime::Interval>> {
        let col = self.columns.get(index).ok_or_else(|| {
            Error::OutOfRange(format!("Column {index} out of range"))
        })?;
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let iv = crate::types::datetime::interval_decode_wire(raw, col.encoded_length as u16)?;
        if iv.is_null {
            Ok(None)
        } else {
            Ok(Some(iv))
        }
    }

    pub fn get_decimal(&self, index: usize) -> Result<Option<Decimal>> {
        let col = self.columns.get(index).ok_or_else(|| {
            Error::OutOfRange(format!("Column {index} out of range"))
        })?;
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let dec = decimal_decode_wire(raw, col.encoded_length as u16)?;
        if dec.is_null {
            Ok(None)
        } else {
            Ok(Some(dec))
        }
    }

    pub fn get_date(&self, index: usize) -> Result<Option<Date>> {
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let date = date_decode_wire(raw)?;
        if date.is_null {
            Ok(None)
        } else {
            Ok(Some(date))
        }
    }

    pub fn get_datetime(&self, index: usize) -> Result<Option<DateTime>> {
        let col = self.columns.get(index).ok_or_else(|| {
            Error::OutOfRange(format!("Column {index} out of range"))
        })?;
        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let dt = datetime_decode_wire(raw, col.encoded_length as u16)?;
        if dt.is_null {
            Ok(None)
        } else {
            Ok(Some(dt))
        }
    }

    /// Returns a string representation suitable for table display (or "NULL" if null).
    pub fn get_display_value(&self, index: usize) -> Result<String> {
        let col = self.columns.get(index).ok_or_else(|| {
            Error::OutOfRange(format!("Column {index} out of range"))
        })?;

        let raw = match self.get_raw_column(index)? {
            Some(r) => r,
            None => return Ok("NULL".to_string()),
        };

        match col.col_type {
            ColumnType::Char | ColumnType::NChar | ColumnType::VarChar | ColumnType::NVarChar | ColumnType::LVarChar => {
                let decoded = crate::types::charset::decode_wire_string(raw, self.db_locale.as_deref());
                Ok(decoded.trim_end().to_string())
            }
            ColumnType::SmallInt => {
                if raw.len() == 2 {
                    Ok(i16::from_be_bytes([raw[0], raw[1]]).to_string())
                } else {
                    Ok("NULL".to_string())
                }
            }
            ColumnType::Int | ColumnType::Serial => {
                if raw.len() == 4 {
                    Ok(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]).to_string())
                } else {
                    Ok("NULL".to_string())
                }
            }
            ColumnType::BigInt | ColumnType::BigSerial => {
                if raw.len() == 8 {
                    Ok(i64::from_be_bytes([raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7]]).to_string())
                } else if raw.len() == 4 {
                    Ok((i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as i64).to_string())
                } else {
                    Ok("NULL".to_string())
                }
            }
            ColumnType::Int8 | ColumnType::Serial8 => {
                match self.get_i64(index)? {
                    Some(v) => Ok(v.to_string()),
                    None => Ok("NULL".to_string()),
                }
            }
            ColumnType::Interval => {
                match crate::types::datetime::interval_decode_wire(raw, col.encoded_length as u16) {
                    Ok(iv) if !iv.is_null => Ok(iv.to_string()),
                    _ => Ok("NULL".to_string()),
                }
            }
            ColumnType::Float => {
                if raw.len() == 8 {
                    Ok(f64::from_be_bytes([raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7]]).to_string())
                } else {
                    Ok("NULL".to_string())
                }
            }
            ColumnType::SmallFloat => {
                if raw.len() == 4 {
                    Ok(f32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]).to_string())
                } else {
                    Ok("NULL".to_string())
                }
            }
            ColumnType::Bool | ColumnType::DBoolean => {
                if raw.is_empty() {
                    Ok("NULL".to_string())
                } else {
                    Ok(if raw[0] != 0 { "t".to_string() } else { "f".to_string() })
                }
            }
            ColumnType::Date => {
                match date_decode_wire(raw) {
                    Ok(d) if !d.is_null => Ok(d.to_string()),
                    _ => Ok("NULL".to_string()),
                }
            }
            ColumnType::DateTime => {
                match datetime_decode_wire(raw, col.encoded_length as u16) {
                    Ok(dt) if !dt.is_null => Ok(dt.to_string()),
                    _ => Ok("NULL".to_string()),
                }
            }
            ColumnType::Decimal | ColumnType::Money => {
                match decimal_decode_wire(raw, col.encoded_length as u16) {
                    Ok(dec) if !dec.is_null => Ok(dec.to_string()),
                    _ => Ok("NULL".to_string()),
                }
            }
            ColumnType::Blob | ColumnType::Clob => {
                // Smart LOB locator: already ASCII text string in wire representation
                if let Ok(s) = std::str::from_utf8(raw) {
                    Ok(s.trim_end().to_string())
                } else {
                    let mut hex = String::with_capacity(raw.len() * 2);
                    for &b in raw {
                        use std::fmt::Write;
                        let _ = write!(hex, "{:02x}", b);
                    }
                    Ok(hex)
                }
            }
            ColumnType::Byte => {
                let mut hex = String::with_capacity(raw.len() * 2);
                for &b in raw {
                    use std::fmt::Write;
                    let _ = write!(hex, "{:02x}", b);
                }
                Ok(hex)
            }
            _ => {
                Ok(String::from_utf8_lossy(raw).trim_end().to_string())
            }
        }
    }

    /// Returns true if the column value is SQL NULL.
    pub fn is_null(&self, index: usize) -> bool {
        match self.get_raw_column(index) {
            Ok(Some(_)) => false,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_row_get_typed_values() {
        let cols = vec![
            ColumnInfo::new("id", ColumnType::Int, 0, 4, 0, 1),
            ColumnInfo::new("name", ColumnType::VarChar, 0, 32, 4, 2),
            ColumnInfo::new("active", ColumnType::Bool, 0, 1, 10, 3),
        ];

        // Tuple layout:
        // offset 0..4: id = 42 (0x0000002A)
        // offset 4..10: varchar with len=5: [5, 'a', 'l', 'i', 'c', 'e']
        // offset 10..16: boolean with 5-byte header: [0, 0, 0, 0, 1, 1] (present, len=1, true)
        let mut tuple = Vec::new();
        tuple.extend_from_slice(&42i32.to_be_bytes()); // offset 0..4
        tuple.push(5); // len of varchar
        tuple.extend_from_slice(b"alice"); // offset 5..10
        tuple.extend_from_slice(&[0, 0, 0, 0, 1, 1]); // offset 10..16

        let row = Row::new(tuple, cols);
        assert_eq!(row.get_i32(0).unwrap(), Some(42));
        assert_eq!(row.get_str(1).unwrap(), Some("alice"));
        assert_eq!(row.get_bool(2).unwrap(), Some(true));
    }

    #[test]
    fn test_row_null_values() {
        let cols = vec![
            ColumnInfo::new("id", ColumnType::Int, 0, 4, 0, 1),
            ColumnInfo::new("status", ColumnType::SmallInt, 0, 2, 4, 2),
        ];

        let mut tuple = Vec::new();
        tuple.extend_from_slice(&i32::MIN.to_be_bytes()); // NULL int
        tuple.extend_from_slice(&i16::MIN.to_be_bytes()); // NULL smallint

        let row = Row::new(tuple, cols);
        assert_eq!(row.get_i32(0).unwrap(), None);
        assert_eq!(row.get_i16(1).unwrap(), None);
    }
}
