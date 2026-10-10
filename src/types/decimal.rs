//! Exact Informix DECIMAL, NUMERIC, and MONEY wire codecs and arithmetic.

use crate::error::{Error, Result};
use crate::types::base100::radix_complement;
use std::fmt;
use std::str::FromStr;

pub const DECIMAL_LIMB_BASE: u32 = 1_000_000_000;
pub const DECIMAL_LIMB_DIGITS: usize = 9;
pub const DECIMAL_MAX_DIGITS: usize = 1_000_000;
/// Mirrors libsqli's `SQLI_DECIMAL_MAX_TEXT` (`SQLI_DECIMAL_MAX_DIGITS + 32`).
pub const DECIMAL_MAX_TEXT: usize = DECIMAL_MAX_DIGITS + 32;
pub const DECIMAL_WIRE_CAPACITY: usize = 18;

const MAX_PRECISION: u8 = 32;
const FLOATING_SCALE: u8 = 255;
const EXPONENT_BIAS: i32 = 64;
const POSITIVE_FLAG: u8 = 128;
const EXPONENT_MIN: i64 = -64;
const EXPONENT_MAX: i64 = 63;
const PAIR_DIGITS: usize = 2;
const PAIR_BASE: u8 = 100;
const MAX_COEFFICIENT_GROUPS: usize = 16;
const COEFFICIENT_LIMBS: usize = 4;

const POWERS_OF_TEN: [u32; 9] = [
    1, 10, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecimalType {
    pub precision: u8,
    pub scale: u8,
    pub width: usize,
}

pub fn decode_type(descriptor: u16) -> Result<DecimalType> {
    let precision = (descriptor >> 8) as u8;
    let scale = (descriptor & 0xFF) as u8;
    if precision == 0 || precision > MAX_PRECISION || (scale != FLOATING_SCALE && scale > precision) {
        return Err(Error::Protocol(format!("Invalid decimal descriptor: 0x{descriptor:04x}")));
    }
    let width = ((precision as usize) + ((scale as usize) & 1) + 3) / 2;
    Ok(DecimalType {
        precision,
        scale,
        width,
    })
}

pub fn decimal_wire_size(descriptor: u16) -> Result<usize> {
    decode_type(descriptor).map(|t| t.width)
}

/// Exact Informix decimal value representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decimal {
    /// Base-10^9 coefficient limbs, least significant first.
    pub limbs: Vec<u32>,
    /// Scale (number of fractional digits).
    pub scale: i32,
    pub negative: bool,
    pub is_null: bool,
}

impl Decimal {
    pub const fn null() -> Self {
        Self {
            limbs: Vec::new(),
            scale: 0,
            negative: false,
            is_null: true,
        }
    }

    pub fn zero(scale: i32) -> Self {
        Self {
            limbs: Vec::new(),
            scale,
            negative: false,
            is_null: false,
        }
    }

    pub fn from_i64(val: i64, scale: i32) -> Self {
        let negative = val < 0;
        let mut unsigned_val = if negative {
            val.unsigned_abs()
        } else {
            val as u64
        };

        let mut limbs = Vec::new();
        while unsigned_val > 0 {
            limbs.push((unsigned_val % (DECIMAL_LIMB_BASE as u64)) as u32);
            unsigned_val /= DECIMAL_LIMB_BASE as u64;
        }

        let is_zero = Self::is_limbs_zero(&limbs);
        Self {
            limbs,
            scale,
            negative: negative && !is_zero,
            is_null: false,
        }
    }

    fn is_limbs_zero(limbs: &[u32]) -> bool {
        limbs.iter().all(|&l| l == 0)
    }

    pub fn trim(&mut self) {
        while let Some(&last) = self.limbs.last() {
            if last == 0 {
                self.limbs.pop();
            } else {
                break;
            }
        }
        if self.limbs.is_empty() {
            self.negative = false;
        }
    }

    pub fn digits(&self) -> usize {
        if self.limbs.is_empty() {
            return 1;
        }
        let mut digits = (self.limbs.len() - 1) * DECIMAL_LIMB_DIGITS;
        let mut top = *self.limbs.last().unwrap();
        while top > 0 {
            digits += 1;
            top /= 10;
        }
        digits
    }

    pub fn trailing_zeros(&self) -> usize {
        let mut zeros = 0;
        for &limb in &self.limbs {
            if limb == 0 {
                zeros += DECIMAL_LIMB_DIGITS;
                continue;
            }
            let mut l = limb;
            while l % 10 == 0 {
                zeros += 1;
                l /= 10;
            }
            break;
        }
        zeros
    }

    pub fn digit_at(&self, position: usize) -> u8 {
        let limb_idx = position / DECIMAL_LIMB_DIGITS;
        let power_idx = position % DECIMAL_LIMB_DIGITS;
        if limb_idx >= self.limbs.len() {
            0
        } else {
            ((self.limbs[limb_idx] / POWERS_OF_TEN[power_idx]) % 10) as u8
        }
    }
}

impl Default for Decimal {
    fn default() -> Self {
        Self::null()
    }
}

impl FromStr for Decimal {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        if text.is_empty() {
            return Err(Error::InvalidArgument("Empty decimal string".into()));
        }
        if text.len() > DECIMAL_MAX_TEXT {
            return Err(Error::OutOfRange(format!(
                "Decimal text length {} exceeds maximum of {DECIMAL_MAX_TEXT}",
                text.len()
            )));
        }
        let bytes = text.as_bytes();
        let mut pos = 0;
        let mut negative = false;
        if bytes[pos] == b'+' || bytes[pos] == b'-' {
            negative = bytes[pos] == b'-';
            pos += 1;
        }

        let start = pos;
        let mut digits = 0;
        let mut fractional = 0;
        let mut significant_digits = 0;
        let mut point = false;

        while pos < bytes.len() && bytes[pos] != b'e' && bytes[pos] != b'E' {
            let ch = bytes[pos];
            if ch == b'.' && !point {
                point = true;
            } else if ch.is_ascii_digit() {
                digits += 1;
                if ch != b'0' || significant_digits != 0 {
                    significant_digits += 1;
                }
                if point {
                    fractional += 1;
                }
            } else {
                return Err(Error::InvalidArgument(format!("Unexpected character: {}", ch as char)));
            }
            pos += 1;
        }

        if digits == 0 {
            return Err(Error::InvalidArgument("No digits in decimal string".into()));
        }
        if significant_digits > DECIMAL_MAX_DIGITS {
            return Err(Error::OutOfRange(format!(
                "Decimal has {significant_digits} significant digits, exceeds maximum of {DECIMAL_MAX_DIGITS}"
            )));
        }

        let mantissa_end = pos;
        let mut exponent: i64 = 0;
        if pos < bytes.len() {
            // Parse exponent after 'e' or 'E'
            pos += 1;
            let mut exp_negative = false;
            if pos < bytes.len() && (bytes[pos] == b'+' || bytes[pos] == b'-') {
                exp_negative = bytes[pos] == b'-';
                pos += 1;
            }
            if pos == bytes.len() {
                return Err(Error::InvalidArgument("Missing exponent digits".into()));
            }
            for &ch in &bytes[pos..] {
                if !ch.is_ascii_digit() {
                    return Err(Error::InvalidArgument("Non-digit in exponent".into()));
                }
                exponent = exponent
                    .checked_mul(10)
                    .and_then(|e| e.checked_add((ch - b'0') as i64))
                    .ok_or_else(|| Error::OutOfRange("Exponent overflow".into()))?;
            }
            if exp_negative {
                exponent = -exponent;
            }
        }

        let fractional_i64 = fractional as i64;
        if exponent < fractional_i64 - (i32::MAX as i64) || exponent > fractional_i64 - (i32::MIN as i64) {
            return Err(Error::OutOfRange("Decimal scale exceeds i32 range".into()));
        }
        let scale = (fractional_i64 - exponent) as i32;
        let count = (significant_digits + DECIMAL_LIMB_DIGITS - 1) / DECIMAL_LIMB_DIGITS;
        let mut limbs = vec![0u32; count];

        let mut offset = 0;
        for i in (start..mantissa_end).rev() {
            let ch = bytes[i];
            if ch != b'.' && offset < significant_digits {
                limbs[offset / DECIMAL_LIMB_DIGITS] +=
                    ((ch - b'0') as u32) * POWERS_OF_TEN[offset % DECIMAL_LIMB_DIGITS];
                offset += 1;
            }
        }

        let mut dec = Decimal {
            limbs,
            scale,
            negative,
            is_null: false,
        };
        dec.trim();
        Ok(dec)
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null {
            return write!(f, "NULL");
        }
        let digits = self.digits();
        let exponent = (digits as i64) - 1 - (self.scale as i64);
        let scientific = self.scale < 0 || exponent < -6;

        if self.negative {
            write!(f, "-")?;
        }

        let digit_at = |i: usize| -> u8 {
            if i >= digits {
                0
            } else {
                let offset = digits - i - 1;
                self.digit_at(offset)
            }
        };

        if scientific {
            write!(f, "{}", digit_at(0))?;
            if digits > 1 {
                write!(f, ".")?;
                for i in 1..digits {
                    write!(f, "{}", digit_at(i))?;
                }
            }
            write!(f, "E{:+}", exponent)?;
        } else if self.scale > 0 && (self.scale as usize) >= digits {
            write!(f, "0.")?;
            let zeros = (self.scale as usize) - digits;
            for _ in 0..zeros {
                write!(f, "0")?;
            }
            for i in 0..digits {
                write!(f, "{}", digit_at(i))?;
            }
        } else {
            for i in 0..digits {
                if self.scale != 0 && i == digits - (self.scale as usize) {
                    write!(f, ".")?;
                }
                write!(f, "{}", digit_at(i))?;
            }
        }

        Ok(())
    }
}

/// Encodes a `Decimal` into Informix wire format bytes.
pub fn decimal_encode_wire(
    value: &Decimal,
    descriptor: u16,
    bytes: &mut [u8],
) -> Result<usize> {
    let dtype = decode_type(descriptor)?;
    if bytes.len() < dtype.width {
        return Err(Error::BufferTooSmall {
            needed: dtype.width,
            available: bytes.len(),
        });
    }

    let mut encoded = [0u8; DECIMAL_WIRE_CAPACITY];
    if !value.is_null {
        if value.limbs.is_empty() {
            encoded[0] = POSITIVE_FLAG;
        } else {
            encode_coefficient(value, &dtype, &mut encoded)?;
        }
    }

    bytes[..dtype.width].copy_from_slice(&encoded[..dtype.width]);
    Ok(dtype.width)
}

fn encode_coefficient(
    value: &Decimal,
    dtype: &DecimalType,
    encoded: &mut [u8; DECIMAL_WIRE_CAPACITY],
) -> Result<()> {
    let digits = value.digits();
    let zeros = value.trailing_zeros();
    let significant = digits - zeros;

    if dtype.scale == FLOATING_SCALE {
        if significant > dtype.precision as usize {
            return Err(Error::OutOfRange("Significant digits exceed precision".into()));
        }
    } else {
        let reduction = (value.scale as i64) - (dtype.scale as i64);
        if reduction > (zeros as i64) {
            return Err(Error::Inexact("Conversion would discard nonzero fractional digits".into()));
        }
        if (digits as i64) - reduction > (dtype.precision as i64) {
            return Err(Error::OutOfRange("Integral digits exceed precision".into()));
        }
    }

    let integral_digits = (digits as i64) - (value.scale as i64);
    let exponent = if integral_digits >= 0 {
        (integral_digits + 1) / (PAIR_DIGITS as i64)
    } else {
        integral_digits / (PAIR_DIGITS as i64)
    };

    if !(EXPONENT_MIN..=EXPONENT_MAX).contains(&exponent) {
        return Err(Error::OutOfRange(format!("Exponent {exponent} outside [-64, 63]")));
    }

    let padding = (exponent * (PAIR_DIGITS as i64) - integral_digits) as usize;
    let groups = (padding + significant + 1) / PAIR_DIGITS;
    if groups > MAX_COEFFICIENT_GROUPS || groups > dtype.width - 1 {
        return Err(Error::OutOfRange("Coefficient groups exceed capacity".into()));
    }

    for i in 0..significant {
        let position = i + padding;
        let digit = value.digit_at(digits - 1 - i);
        let group_idx = 1 + position / PAIR_DIGITS;
        let add_val = if position % PAIR_DIGITS == 0 {
            digit * 10
        } else {
            digit
        };
        encoded[group_idx] += add_val;
    }

    encoded[0] = POSITIVE_FLAG | (((exponent as i32) + EXPONENT_BIAS) as u8);
    if value.negative {
        encoded[0] ^= 0xFF;
        radix_complement(&mut encoded[1..dtype.width]);
    }

    Ok(())
}

fn append_digit(limbs: &mut [u32; COEFFICIENT_LIMBS], digit: u32) {
    let mut carry = digit as u64;
    for limb in limbs.iter_mut() {
        let next = (*limb as u64) * 10 + carry;
        *limb = (next % (DECIMAL_LIMB_BASE as u64)) as u32;
        carry = next / (DECIMAL_LIMB_BASE as u64);
    }
}

/// Decodes an Informix wire format byte slice into a `Decimal`.
pub fn decimal_decode_wire(bytes: &[u8], descriptor: u16) -> Result<Decimal> {
    let dtype = decode_type(descriptor)?;
    if bytes.len() != dtype.width {
        return Err(Error::Protocol(format!(
            "Decimal payload width mismatch: expected {}, got {}",
            dtype.width,
            bytes.len()
        )));
    }

    if bytes.iter().all(|&b| b == 0) {
        return Ok(Decimal::null());
    }

    let count = dtype.width - 1;
    let mut groups = [0u8; DECIMAL_WIRE_CAPACITY];
    for i in 0..count {
        if bytes[i + 1] >= PAIR_BASE {
            return Err(Error::Protocol("Invalid base-100 digit in decimal".into()));
        }
        groups[i] = bytes[i + 1];
    }

    let negative = (bytes[0] & POSITIVE_FLAG) == 0;
    if negative {
        radix_complement(&mut groups[..count]);
    }

    let mut effective_count = count;
    while effective_count != 0 && groups[effective_count - 1] == 0 {
        effective_count -= 1;
    }

    if effective_count == 0 {
        if bytes[0] != POSITIVE_FLAG {
            return Err(Error::Protocol("Invalid zero marker in decimal".into()));
        }
        let scale = if dtype.scale == FLOATING_SCALE {
            0
        } else {
            dtype.scale as i32
        };
        return Ok(Decimal::zero(scale));
    }

    if effective_count > MAX_COEFFICIENT_GROUPS || groups[0] == 0 {
        return Err(Error::Protocol("Unnormalized decimal coefficient".into()));
    }

    let exponent = if negative {
        ((bytes[0] ^ 0x7F) as i32) - EXPONENT_BIAS
    } else {
        ((bytes[0] & 0x7F) as i32) - EXPONENT_BIAS
    };

    let drop_last_zero = groups[effective_count - 1] % 10 == 0;
    let first_digit = if groups[0] < 10 { 1 } else { 0 };
    let end_digit = effective_count * PAIR_DIGITS - if drop_last_zero { 1 } else { 0 };
    let digits = end_digit - first_digit;
    let mut scale = (effective_count * PAIR_DIGITS) as i32
        - (exponent * (PAIR_DIGITS as i32))
        - if drop_last_zero { 1 } else { 0 };

    if digits > dtype.precision as usize {
        return Err(Error::Protocol("Decoded digits exceed precision".into()));
    }

    let mut extra_zeros = 0;
    if dtype.scale != FLOATING_SCALE {
        extra_zeros = (dtype.scale as i32) - scale;
        if extra_zeros < 0 || (extra_zeros as usize) > (dtype.precision as usize) - digits {
            return Err(Error::Protocol("Scale mismatch in fixed decimal".into()));
        }
        scale = dtype.scale as i32;
    }

    let mut limbs = [0u32; COEFFICIENT_LIMBS];
    for i in first_digit..end_digit {
        let digit = if i % PAIR_DIGITS == 0 {
            (groups[i / PAIR_DIGITS] / 10) as u32
        } else {
            (groups[i / PAIR_DIGITS] % 10) as u32
        };
        append_digit(&mut limbs, digit);
    }
    for _ in 0..extra_zeros {
        append_digit(&mut limbs, 0);
    }

    let mut dec_limbs = limbs.to_vec();
    while let Some(&last) = dec_limbs.last() {
        if last == 0 {
            dec_limbs.pop();
        } else {
            break;
        }
    }

    Ok(Decimal {
        limbs: dec_limbs,
        scale,
        negative,
        is_null: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOATING32: u16 = 0x20FF;
    const FIXED8_4: u16 = 0x0804;

    #[test]
    fn test_fixed_fixtures() {
        struct TestCase {
            descriptor: u16,
            bytes: &'static [u8],
            text: &'static str,
        }

        let cases = [
            TestCase {
                descriptor: FIXED8_4,
                bytes: &[0xc2, 1, 23, 45, 0],
                text: "123.4500",
            },
            TestCase {
                descriptor: FIXED8_4,
                bytes: &[0x3d, 98, 76, 55, 0],
                text: "-123.4500",
            },
            TestCase {
                descriptor: FIXED8_4,
                bytes: &[0x80, 0, 0, 0, 0],
                text: "0.0000",
            },
            TestCase {
                descriptor: 0x0302,
                bytes: &[0xc1, 1, 23],
                text: "1.23",
            },
            TestCase {
                descriptor: 0x0303,
                bytes: &[0x40, 90, 0],
                text: "-0.001",
            },
            TestCase {
                descriptor: FLOATING32,
                bytes: &[
                    0xc2, 1, 23, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                ],
                text: "123.45",
            },
        ];

        for case in cases {
            // Decode test
            let dec = decimal_decode_wire(case.bytes, case.descriptor).unwrap();
            assert_eq!(dec.to_string(), case.text);

            // Encode roundtrip
            let parsed: Decimal = case.text.parse().unwrap();
            let mut buf = [0u8; 18];
            let len = decimal_encode_wire(&parsed, case.descriptor, &mut buf).unwrap();
            assert_eq!(len, case.bytes.len());
            assert_eq!(&buf[..len], case.bytes);
        }
    }

    #[test]
    fn test_all_descriptors_validity() {
        let mut accepted = 0;
        for descriptor in 0..=u16::MAX {
            let precision = (descriptor >> 8) as u8;
            let scale = (descriptor & 0xFF) as u8;
            let valid = precision >= 1 && precision <= 32 && (scale <= precision || scale == 255);
            match decimal_wire_size(descriptor) {
                Ok(width) => {
                    assert!(valid);
                    assert_eq!(width, ((precision as usize) + ((scale as usize) & 1) + 3) / 2);
                    accepted += 1;
                }
                Err(_) => {
                    assert!(!valid);
                }
            }
        }
        assert_eq!(accepted, 592);
    }
}
