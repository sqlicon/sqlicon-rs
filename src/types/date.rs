//! Informix DATE wire format and calculations.
//! Informix wire DATE epoch is 1899-12-31 (day 0).
//! Plain 4-byte BE integer (days since epoch). NULL is 0x80000000.

use crate::error::{Error, Result};

pub const DATE_WIRE_SIZE: usize = 4;
pub const DATE_NULL_ENCODED: u32 = 0x8000_0000;

const FIRST_YEAR: i32 = 1;
const LAST_YEAR: i32 = 9999;
const EPOCH_FOLLOWING_YEAR: i32 = 1900;
const DAYS_PER_YEAR: i32 = 365;
const MONTHS_PER_YEAR: u32 = 12;

const MONTH_DAYS: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

#[inline]
fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[inline]
fn days_in_month(year: i32, month: u8) -> u8 {
    if month == 2 && is_leap_year(year) {
        29
    } else if (1..=12).contains(&month) {
        MONTH_DAYS[(month - 1) as usize]
    } else {
        0
    }
}

#[inline]
fn days_before_year(year: i32) -> i32 {
    let prev = year - 1;
    prev * DAYS_PER_YEAR + prev / 4 - prev / 100 + prev / 400
}

/// Proleptic Gregorian calendar date (years 1..9999).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub is_null: bool,
}

impl Date {
    pub const fn null() -> Self {
        Self {
            year: 0,
            month: 0,
            day: 0,
            is_null: true,
        }
    }

    pub fn new(year: i32, month: u8, day: u8) -> Result<Self> {
        let date = Self {
            year,
            month,
            day,
            is_null: false,
        };
        date.validate()?;
        Ok(date)
    }

    pub fn validate(&self) -> Result<()> {
        if self.is_null {
            return Ok(());
        }
        if self.year < FIRST_YEAR || self.year > LAST_YEAR {
            return Err(Error::OutOfRange(format!("Year {} outside 1..9999", self.year)));
        }
        if self.month < 1 || self.month > 12 {
            return Err(Error::OutOfRange(format!("Month {} outside 1..12", self.month)));
        }
        let max_days = days_in_month(self.year, self.month);
        if self.day < 1 || self.day > max_days {
            return Err(Error::OutOfRange(format!(
                "Day {} outside 1..{} for year {} month {}",
                self.day, max_days, self.year, self.month
            )));
        }
        Ok(())
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_null {
            write!(f, "NULL")
        } else {
            write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
        }
    }
}

/// Encodes an Informix Date into 4-byte BE wire format.
pub fn date_encode_wire(date: &Date, bytes: &mut [u8]) -> Result<usize> {
    date.validate()?;
    if bytes.len() < DATE_WIRE_SIZE {
        return Err(Error::BufferTooSmall {
            needed: DATE_WIRE_SIZE,
            available: bytes.len(),
        });
    }

    let encoded: u32 = if date.is_null {
        DATE_NULL_ENCODED
    } else {
        let mut ordinal = days_before_year(date.year) + (date.day as i32) - 1;
        for m in 1..date.month {
            ordinal += days_in_month(date.year, m) as i32;
        }
        let days = ordinal - (days_before_year(EPOCH_FOLLOWING_YEAR) - 1);
        days as u32
    };

    bytes[0] = (encoded >> 24) as u8;
    bytes[1] = (encoded >> 16) as u8;
    bytes[2] = (encoded >> 8) as u8;
    bytes[3] = encoded as u8;

    Ok(DATE_WIRE_SIZE)
}

/// Decodes an Informix 4-byte BE wire format into a `Date`.
pub fn date_decode_wire(bytes: &[u8]) -> Result<Date> {
    if bytes.len() != DATE_WIRE_SIZE {
        return Err(Error::Protocol(format!(
            "Invalid date wire size: expected {}, got {}",
            DATE_WIRE_SIZE,
            bytes.len()
        )));
    }

    let encoded = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if encoded == DATE_NULL_ENCODED {
        return Ok(Date::null());
    }

    let mut days = encoded as i64;
    if encoded > i32::MAX as u32 {
        days -= 4_294_967_296i64;
    }

    let ordinal = days + (days_before_year(EPOCH_FOLLOWING_YEAR) as i64) - 1;
    if ordinal < 0 || ordinal >= days_before_year(LAST_YEAR + 1) as i64 {
        return Err(Error::Protocol(format!("Date ordinal {ordinal} outside valid calendar range")));
    }

    let mut low = FIRST_YEAR;
    let mut high = LAST_YEAR;
    while low < high {
        let middle = low + (high - low + 1) / 2;
        if (days_before_year(middle) as i64) <= ordinal {
            low = middle;
        } else {
            high = middle - 1;
        }
    }

    let year = low;
    let mut day = (ordinal - (days_before_year(year) as i64)) as u32;
    let mut month = 1u8;
    while (month as u32) < MONTHS_PER_YEAR && day >= (days_in_month(year, month) as u32) {
        day -= days_in_month(year, month) as u32;
        month += 1;
    }

    Ok(Date {
        year,
        month,
        day: (day + 1) as u8,
        is_null: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_date_fixtures() {
        struct Case {
            bytes: [u8; 4],
            year: i32,
            month: u8,
            day: u8,
            is_null: bool,
        }

        let cases = [
            Case {
                bytes: [0, 0, 0, 0],
                year: 1899,
                month: 12,
                day: 31,
                is_null: false,
            },
            Case {
                bytes: [255, 255, 255, 255],
                year: 1899,
                month: 12,
                day: 30,
                is_null: false,
            },
            Case {
                bytes: [0, 0, 0x63, 0xe0],
                year: 1970,
                month: 1,
                day: 1,
                is_null: false,
            },
            Case {
                bytes: [0x80, 0, 0, 0],
                year: 0,
                month: 0,
                day: 0,
                is_null: true,
            },
            Case {
                bytes: [0xff, 0xf5, 0x6a, 0xa6],
                year: 1,
                month: 1,
                day: 1,
                is_null: false,
            },
            Case {
                bytes: [0, 0x2d, 0x24, 0x80],
                year: 9999,
                month: 12,
                day: 31,
                is_null: false,
            },
        ];

        for c in cases {
            let decoded = date_decode_wire(&c.bytes).unwrap();
            assert_eq!(decoded.year, c.year);
            assert_eq!(decoded.month, c.month);
            assert_eq!(decoded.day, c.day);
            assert_eq!(decoded.is_null, c.is_null);

            let mut encoded = [0u8; 4];
            let len = date_encode_wire(&decoded, &mut encoded).unwrap();
            assert_eq!(len, 4);
            assert_eq!(encoded, c.bytes);
        }
    }
}
