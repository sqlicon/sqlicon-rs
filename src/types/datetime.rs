//! Qualified Informix DATETIME and INTERVAL wire formats and codecs.

use crate::error::{Error, Result};
use crate::types::base100::radix_complement;

pub const TEMPORAL_WIRE_CAPACITY: usize = 12;

const LAST_YEAR: i32 = 9999;
const PAIR_BASE: u8 = 100;
const PAIR_DIGITS: usize = 2;
const EXPONENT_BIAS: i32 = 64;
const POSITIVE_FLAG: u8 = 128;
const ZERO_MARKER: u8 = 128;
const WIRE_SECOND: u32 = 10;
const WIRE_FRACTION_START: u32 = 12;
const MAX_LEADING_DIGITS: u32 = 9;
const YEAR_DIGITS: u32 = 4;
const ORDINARY_DIGITS: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum TemporalField {
    Unknown = 0,
    Year = 1,
    Month = 2,
    Day = 3,
    Hour = 4,
    Minute = 5,
    Second = 6,
    Fraction = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemporalRange {
    pub first: TemporalField,
    pub last: TemporalField,
    pub fractional_digits: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireRange {
    pub range: TemporalRange,
    pub leading_digits: u32,
    pub integral_pairs: usize,
    pub fraction_pairs: usize,
    pub width: usize,
    pub exponent: i32,
}

pub fn power10(mut digits: u32) -> u32 {
    let mut val = 1;
    while digits > 0 {
        val *= 10;
        digits -= 1;
    }
    val
}

pub fn decode_range(qualifier: u16, interval: bool) -> Result<WireRange> {
    let digits = (qualifier >> 8) as u32;
    let start = ((qualifier >> 4) & 0xF) as u32;
    let end = (qualifier & 0xF) as u32;

    if start > WIRE_FRACTION_START
        || (start & 1) != 0
        || (end <= WIRE_SECOND && ((end & 1) != 0 || end < start))
        || (start == WIRE_FRACTION_START && end <= WIRE_SECOND)
    {
        return Err(Error::Protocol(format!("Invalid temporal qualifier: 0x{qualifier:04x}")));
    }

    let first_field = match (start / 2) + 1 {
        1 => TemporalField::Year,
        2 => TemporalField::Month,
        3 => TemporalField::Day,
        4 => TemporalField::Hour,
        5 => TemporalField::Minute,
        6 => TemporalField::Second,
        7 => TemporalField::Fraction,
        _ => return Err(Error::Protocol("Invalid start field".into())),
    };

    let last_field = if end > WIRE_SECOND {
        TemporalField::Fraction
    } else {
        match (end / 2) + 1 {
            1 => TemporalField::Year,
            2 => TemporalField::Month,
            3 => TemporalField::Day,
            4 => TemporalField::Hour,
            5 => TemporalField::Minute,
            6 => TemporalField::Second,
            7 => TemporalField::Fraction,
            _ => return Err(Error::Protocol("Invalid end field".into())),
        }
    };

    let fractional_digits = if end > WIRE_SECOND {
        (end - WIRE_SECOND) as u8
    } else {
        0
    };

    if interval && first_field <= TemporalField::Month && last_field > TemporalField::Month {
        return Err(Error::Protocol("Interval cannot span YEAR/MONTH and DAY/time".into()));
    }

    let mut leading_digits = 0;
    let mut integral_pairs = 0;
    let mut exponent = 0;

    if start == WIRE_FRACTION_START {
        if digits != fractional_digits as u32 {
            return Err(Error::Protocol("Fractional digits mismatch".into()));
        }
    } else {
        let remaining = end - start;
        if digits <= remaining {
            return Err(Error::Protocol("Qualifier digits too small".into()));
        }
        leading_digits = digits - remaining;
        if interval {
            if leading_digits > MAX_LEADING_DIGITS {
                return Err(Error::Protocol("Interval leading digits exceed max".into()));
            }
        } else {
            let expected_lead = if start == 0 {
                YEAR_DIGITS
            } else {
                ORDINARY_DIGITS
            };
            if leading_digits != expected_lead {
                return Err(Error::Protocol("Datetime leading digits mismatch".into()));
            }
        }
        let integral_end = if end > WIRE_SECOND { WIRE_SECOND } else { end };
        integral_pairs = ((leading_digits + 1) / (PAIR_DIGITS as u32)
            + (integral_end - start) / (PAIR_DIGITS as u32)) as usize;
        exponent = (integral_pairs as i32) + (((WIRE_SECOND - integral_end) / (PAIR_DIGITS as u32)) as i32);
    }

    let fraction_pairs = (((fractional_digits as usize) + 1) / PAIR_DIGITS) as usize;
    let width = 1 + integral_pairs + fraction_pairs;

    if width > TEMPORAL_WIRE_CAPACITY {
        return Err(Error::Protocol("Temporal wire size exceeds capacity".into()));
    }

    Ok(WireRange {
        range: TemporalRange {
            first: first_field,
            last: last_field,
            fractional_digits,
        },
        leading_digits,
        integral_pairs,
        fraction_pairs,
        width,
        exponent,
    })
}

pub fn temporal_wire_size(qualifier: u16, interval: bool) -> Result<usize> {
    decode_range(qualifier, interval).map(|r| r.width)
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct WireFields {
    fields: [u64; 8],
    nanosecond: u32,
    negative: bool,
    is_null: bool,
}

fn unpack(bytes: &[u8], range: &WireRange) -> Result<WireFields> {
    if bytes.len() != range.width {
        return Err(Error::Protocol(format!(
            "Payload length mismatch: expected {}, got {}",
            range.width,
            bytes.len()
        )));
    }

    if bytes.iter().all(|&b| b == 0) {
        return Ok(WireFields {
            is_null: true,
            ..Default::default()
        });
    }

    let count = bytes.len() - 1;
    let mut coefficient = [0u8; TEMPORAL_WIRE_CAPACITY];
    for i in 0..count {
        if bytes[i + 1] >= PAIR_BASE {
            return Err(Error::Protocol("Invalid base-100 digit".into()));
        }
        coefficient[i] = bytes[i + 1];
    }

    let negative = (bytes[0] & POSITIVE_FLAG) == 0;
    if negative {
        radix_complement(&mut coefficient[..count]);
    }

    if coefficient[..count].iter().all(|&b| b == 0) {
        if bytes[0] == ZERO_MARKER {
            return Ok(WireFields {
                is_null: false,
                negative: false,
                ..Default::default()
            });
        } else {
            return Err(Error::Protocol("Invalid zero marker in temporal".into()));
        }
    }

    if coefficient[0] == 0 {
        return Err(Error::Protocol("Unnormalized temporal coefficient".into()));
    }

    let exponent = if negative {
        ((bytes[0] ^ 0x7F) as i32) - EXPONENT_BIAS
    } else {
        ((bytes[0] & 0x7F) as i32) - EXPONENT_BIAS
    };

    let offset = range.exponent - exponent;
    let mut aligned = [0u8; TEMPORAL_WIRE_CAPACITY];
    for i in 0..count {
        let slot = offset + (i as i32);
        if slot < 0 || slot >= (count as i32) {
            if coefficient[i] != 0 {
                return Err(Error::Protocol("Misaligned temporal coefficient".into()));
            }
        } else {
            aligned[slot as usize] = coefficient[i];
        }
    }

    let mut fields = [0u64; 8];
    let mut pos = 0;
    for field_num in (range.range.first as usize)..=(range.range.last as usize).min(TemporalField::Second as usize) {
        let width = if field_num == (range.range.first as usize) {
            ((range.leading_digits + 1) / (PAIR_DIGITS as u32)) as usize
        } else {
            1
        };
        let mut magnitude: u64 = 0;
        for _ in 0..width {
            magnitude = magnitude * (PAIR_BASE as u64) + (aligned[pos] as u64);
            pos += 1;
        }
        fields[field_num] = magnitude;
    }

    let mut fraction: u32 = 0;
    for _ in 0..range.fraction_pairs {
        fraction = fraction * (PAIR_BASE as u32) + (aligned[pos] as u32);
        pos += 1;
    }

    if (range.range.fractional_digits & 1) != 0 {
        if fraction % 10 != 0 {
            return Err(Error::Protocol("Nonzero discarded sub-digit in fraction".into()));
        }
        fraction /= 10;
    }

    let nanosecond = fraction * power10(9 - (range.range.fractional_digits as u32));
    if range.leading_digits != 0 && fields[range.range.first as usize] >= (power10(range.leading_digits) as u64) {
        return Err(Error::Protocol("Leading digits magnitude overflow".into()));
    }

    Ok(WireFields {
        fields,
        nanosecond,
        negative,
        is_null: false,
    })
}

fn pack(fields: &WireFields, range: &WireRange, bytes: &mut [u8]) -> Result<usize> {
    if bytes.len() < range.width {
        return Err(Error::BufferTooSmall {
            needed: range.width,
            available: bytes.len(),
        });
    }

    let mut temporary = [0u8; TEMPORAL_WIRE_CAPACITY];
    if !fields.is_null {
        if range.leading_digits != 0
            && fields.fields[range.range.first as usize] >= (power10(range.leading_digits) as u64)
        {
            return Err(Error::OutOfRange("Leading digits magnitude overflow".into()));
        }

        let factor = power10(9 - (range.range.fractional_digits as u32));
        if fields.nanosecond % factor != 0 {
            return Err(Error::Inexact("Conversion would lose fractional precision".into()));
        }

        let mut groups = [0u8; TEMPORAL_WIRE_CAPACITY];
        let mut count = 0;

        for field_num in (range.range.first as usize)..=(range.range.last as usize).min(TemporalField::Second as usize) {
            let width = if field_num == (range.range.first as usize) {
                ((range.leading_digits + 1) / (PAIR_DIGITS as u32)) as usize
            } else {
                1
            };
            let mut magnitude = fields.fields[field_num];
            for i in (0..width).rev() {
                groups[count + i] = (magnitude % (PAIR_BASE as u64)) as u8;
                magnitude /= PAIR_BASE as u64;
            }
            count += width;
        }

        let mut fraction = fields.nanosecond / factor;
        if (range.range.fractional_digits & 1) != 0 {
            fraction *= 10;
        }
        for i in (0..range.fraction_pairs).rev() {
            groups[count + i] = (fraction % (PAIR_BASE as u32)) as u8;
            fraction /= PAIR_BASE as u32;
        }
        count += range.fraction_pairs;

        let mut first = 0;
        while first < count && groups[first] == 0 {
            first += 1;
        }

        if first == count {
            temporary[0] = ZERO_MARKER;
        } else {
            let exponent = range.exponent - (first as i32);
            temporary[0] = POSITIVE_FLAG | (((exponent + EXPONENT_BIAS) as u8) & 0x7F);
            temporary[1..1 + (count - first)].copy_from_slice(&groups[first..count]);
            if fields.negative {
                temporary[0] ^= 0xFF;
                radix_complement(&mut temporary[1..range.width]);
            }
        }
    }

    bytes[..range.width].copy_from_slice(&temporary[..range.width]);
    Ok(range.width)
}

/// Qualified calendar datetime representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateTime {
    pub range: TemporalRange,
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub nanosecond: u32,
    pub is_null: bool,
}

impl DateTime {
    pub fn null(range: TemporalRange) -> Self {
        Self {
            range,
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            nanosecond: 0,
            is_null: true,
        }
    }
}

pub fn datetime_decode_wire(bytes: &[u8], qualifier: u16) -> Result<DateTime> {
    let range = decode_range(qualifier, false)?;
    let fields = unpack(bytes, &range)?;
    if fields.negative {
        return Err(Error::Protocol("Negative calendar datetime is not supported".into()));
    }
    if fields.fields[TemporalField::Year as usize] > LAST_YEAR as u64 {
        return Err(Error::Protocol("Year exceeds 9999".into()));
    }

    Ok(DateTime {
        range: range.range,
        year: fields.fields[TemporalField::Year as usize] as i32,
        month: fields.fields[TemporalField::Month as usize] as u8,
        day: fields.fields[TemporalField::Day as usize] as u8,
        hour: fields.fields[TemporalField::Hour as usize] as u8,
        minute: fields.fields[TemporalField::Minute as usize] as u8,
        second: fields.fields[TemporalField::Second as usize] as u8,
        nanosecond: fields.nanosecond,
        is_null: fields.is_null,
    })
}

pub fn datetime_encode_wire(value: &DateTime, qualifier: u16, bytes: &mut [u8]) -> Result<usize> {
    let range = decode_range(qualifier, false)?;
    let mut fields = [0u64; 8];
    fields[TemporalField::Year as usize] = value.year as u64;
    fields[TemporalField::Month as usize] = value.month as u64;
    fields[TemporalField::Day as usize] = value.day as u64;
    fields[TemporalField::Hour as usize] = value.hour as u64;
    fields[TemporalField::Minute as usize] = value.minute as u64;
    fields[TemporalField::Second as usize] = value.second as u64;

    let wf = WireFields {
        fields,
        nanosecond: value.nanosecond,
        negative: false,
        is_null: value.is_null,
    };
    pack(&wf, &range, bytes)
}

/// Qualified INTERVAL representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interval {
    pub range: TemporalRange,
    pub years: u64,
    pub months: u64,
    pub days: u64,
    pub hours: u64,
    pub minutes: u64,
    pub seconds: u64,
    pub nanosecond: u32,
    pub negative: bool,
    pub is_null: bool,
}

pub fn interval_decode_wire(bytes: &[u8], qualifier: u16) -> Result<Interval> {
    let range = decode_range(qualifier, true)?;
    let fields = unpack(bytes, &range)?;

    Ok(Interval {
        range: range.range,
        years: fields.fields[TemporalField::Year as usize],
        months: fields.fields[TemporalField::Month as usize],
        days: fields.fields[TemporalField::Day as usize],
        hours: fields.fields[TemporalField::Hour as usize],
        minutes: fields.fields[TemporalField::Minute as usize],
        seconds: fields.fields[TemporalField::Second as usize],
        nanosecond: fields.nanosecond,
        negative: fields.negative,
        is_null: fields.is_null,
    })
}

impl std::fmt::Display for DateTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_null {
            return write!(f, "NULL");
        }
        if self.hour == 0 && self.minute == 0 && self.second == 0 && self.nanosecond == 0 {
            write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
        } else if self.nanosecond > 0 {
            let millis = self.nanosecond / 1_000_000;
            write!(
                f,
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
                self.year, self.month, self.day, self.hour, self.minute, self.second, millis
            )
        } else {
            write!(
                f,
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                self.year, self.month, self.day, self.hour, self.minute, self.second
            )
        }
    }
}

impl std::fmt::Display for Interval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_null {
            return write!(f, "NULL");
        }
        let sign = if self.negative { "-" } else { "" };
        write!(
            f,
            "{}{}-{:02} {:02}:{:02}:{:02}",
            sign, self.days, self.hours, self.minutes, self.seconds, self.nanosecond / 1_000_000
        )
    }
}

pub fn interval_encode_wire(value: &Interval, qualifier: u16, bytes: &mut [u8]) -> Result<usize> {
    let range = decode_range(qualifier, true)?;
    let mut fields = [0u64; 8];
    fields[TemporalField::Year as usize] = value.years;
    fields[TemporalField::Month as usize] = value.months;
    fields[TemporalField::Day as usize] = value.days;
    fields[TemporalField::Hour as usize] = value.hours;
    fields[TemporalField::Minute as usize] = value.minutes;
    fields[TemporalField::Second as usize] = value.seconds;

    let wf = WireFields {
        fields,
        nanosecond: value.nanosecond,
        negative: value.negative,
        is_null: value.is_null,
    };
    pack(&wf, &range, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_DATETIME: u16 = 0x0E0A; // YEAR TO SECOND
    const YEAR_DATETIME: u16 = 0x0400; // YEAR TO YEAR
    const FRACTION_DATETIME: u16 = 0x05CF; // FRACTION(5) TO FRACTION(5)
    const DAY_INTERVAL: u16 = 0x0E4F;

    #[test]
    fn test_fraction_datetime_fixture() {
        let raw = [0xbe, 10, 0, 0];
        let dt = datetime_decode_wire(&raw, FRACTION_DATETIME).unwrap();
        assert_eq!(dt.nanosecond, 10_000);
        assert!(!dt.is_null);

        let mut encoded = [0u8; 4];
        let len = datetime_encode_wire(&dt, FRACTION_DATETIME, &mut encoded).unwrap();
        assert_eq!(len, 4);
        assert_eq!(encoded, raw);
    }

    #[test]
    fn test_datetime_fixtures() {
        // Test case 1: 0x0e0a
        let raw = [0xc7, 20, 26, 6, 20, 12, 34, 56];
        let dt = datetime_decode_wire(&raw, FULL_DATETIME).unwrap();
        assert_eq!(dt.year, 2026);
        assert_eq!(dt.month, 6);
        assert_eq!(dt.day, 20);
        assert_eq!(dt.hour, 12);
        assert_eq!(dt.minute, 34);
        assert_eq!(dt.second, 56);
        assert!(!dt.is_null);

        let mut encoded = [0u8; 8];
        let len = datetime_encode_wire(&dt, FULL_DATETIME, &mut encoded).unwrap();
        assert_eq!(len, 8);
        assert_eq!(encoded, raw);
    }

    #[test]
    fn test_year_datetime_fixture() {
        let raw = [0xc6, 1, 0];
        let dt = datetime_decode_wire(&raw, YEAR_DATETIME).unwrap();
        assert_eq!(dt.year, 1);
        assert!(!dt.is_null);

        let mut encoded = [0u8; 3];
        let len = datetime_encode_wire(&dt, YEAR_DATETIME, &mut encoded).unwrap();
        assert_eq!(len, 3);
        assert_eq!(encoded, raw);
    }

    #[test]
    fn test_interval_fixture() {
        let raw = [0xc4, 3, 3, 27, 1, 12, 34, 50, 0];
        let itv = interval_decode_wire(&raw, DAY_INTERVAL).unwrap();
        assert_eq!(itv.days, 3);
        assert_eq!(itv.hours, 3);
        assert_eq!(itv.minutes, 27);
        assert_eq!(itv.seconds, 1);
        assert_eq!(itv.nanosecond, 123450000);
        assert!(!itv.is_null);

        let mut encoded = [0u8; 9];
        let len = interval_encode_wire(&itv, DAY_INTERVAL, &mut encoded).unwrap();
        assert_eq!(len, 9);
        assert_eq!(encoded, raw);
    }
}
