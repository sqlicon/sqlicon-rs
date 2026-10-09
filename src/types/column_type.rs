//! Informix column types and type bitmask flags.

use crate::error::{Error, Result};
use std::fmt;

/// Type flags associated with Informix columns.
pub mod flags {
    pub const NOT_NULLABLE: u16 = 0x0100;
    pub const DISTINCT: u16 = 0x0800;
    pub const NAMED_ROW: u16 = 0x1000;
    pub const DBOOLEAN: u16 = 0x4000;
}

/// Semantic Informix SQL column types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum ColumnType {
    /// 8-bit ASCII or local character string (`CHAR(n)`).
    Char = 0,
    /// 16-bit signed integer (`SMALLINT`).
    SmallInt = 1,
    /// 32-bit signed integer (`INTEGER`).
    Int = 2,
    /// 64-bit IEEE 754 floating-point number (`FLOAT`).
    Float = 3,
    /// 32-bit IEEE 754 floating-point number (`SMALLFLOAT`).
    SmallFloat = 4,
    /// Arbitrary precision packed BCD decimal (`DECIMAL(p,s)`).
    Decimal = 5,
    /// Auto-incrementing 32-bit integer (`SERIAL`).
    Serial = 6,
    /// Julian date count (`DATE`).
    Date = 7,
    /// Currency decimal with default 2-digit scale (`MONEY(p,s)`).
    Money = 8,
    /// Untyped NULL placeholder.
    Null = 9,
    /// Calendar timestamp with arbitrary qualifier precision (`DATETIME`).
    DateTime = 10,
    /// Traditional binary large object (`BYTE`).
    Byte = 11,
    /// Traditional text large object (`TEXT`).
    Text = 12,
    /// Variable-length character string up to 255 bytes (`VARCHAR(m,r)`).
    VarChar = 13,
    /// Span of elapsed time (`INTERVAL`).
    Interval = 14,
    /// National character string (`NCHAR(n)`).
    NChar = 15,
    /// Variable-length national character string (`NVARCHAR(m,r)`).
    NVarChar = 16,
    /// 64-bit Informix INT8 integer.
    Int8 = 17,
    /// Auto-incrementing 64-bit INT8 integer (`SERIAL8`).
    Serial8 = 18,
    /// Large variable-length character string up to 32KB (`LVARCHAR`).
    LVarChar = 40,
    /// 1-byte boolean flag (`BOOLEAN`).
    Bool = 41,
    /// Distinct boolean UDT type.
    DBoolean = 45,
    /// Standard 64-bit signed integer (`BIGINT`).
    BigInt = 52,
    /// Auto-incrementing 64-bit integer (`BIGSERIAL`).
    BigSerial = 53,
    /// Smart Large Character Object locator (`CLOB`).
    Clob = 101,
    /// Smart Large Binary Object locator (`BLOB`).
    Blob = 102,
}

impl ColumnType {
    /// Returns the canonical SQL keyword for the data type.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Char => "CHAR",
            Self::SmallInt => "SMALLINT",
            Self::Int => "INTEGER",
            Self::Float => "FLOAT",
            Self::SmallFloat => "SMALLFLOAT",
            Self::Decimal => "DECIMAL",
            Self::Serial => "SERIAL",
            Self::Date => "DATE",
            Self::Money => "MONEY",
            Self::Null => "NULL",
            Self::DateTime => "DATETIME",
            Self::Byte => "BYTE",
            Self::Text => "TEXT",
            Self::VarChar => "VARCHAR",
            Self::Interval => "INTERVAL",
            Self::NChar => "NCHAR",
            Self::NVarChar => "NVARCHAR",
            Self::Int8 => "INT8",
            Self::Serial8 => "SERIAL8",
            Self::LVarChar => "LVARCHAR",
            Self::Bool => "BOOLEAN",
            Self::DBoolean => "DBOOLEAN",
            Self::BigInt => "BIGINT",
            Self::BigSerial => "BIGSERIAL",
            Self::Clob => "CLOB",
            Self::Blob => "BLOB",
        }
    }

    /// Extract raw type code without bit flags (low byte).
    pub const fn from_raw(raw: u16) -> Option<Self> {
        let base = raw & 0x00FF;
        match base {
            0 => Some(Self::Char),
            1 => Some(Self::SmallInt),
            2 => Some(Self::Int),
            3 => Some(Self::Float),
            4 => Some(Self::SmallFloat),
            5 => Some(Self::Decimal),
            6 => Some(Self::Serial),
            7 => Some(Self::Date),
            8 => Some(Self::Money),
            9 => Some(Self::Null),
            10 => Some(Self::DateTime),
            11 => Some(Self::Byte),
            12 => Some(Self::Text),
            13 => Some(Self::VarChar),
            14 => Some(Self::Interval),
            15 => Some(Self::NChar),
            16 => Some(Self::NVarChar),
            17 => Some(Self::Int8),
            18 => Some(Self::Serial8),
            40 => Some(Self::LVarChar),
            41 => Some(Self::Bool),
            45 => Some(Self::DBoolean),
            52 => Some(Self::BigInt),
            53 => Some(Self::BigSerial),
            101 => Some(Self::Clob),
            102 => Some(Self::Blob),
            _ => None,
        }
    }

    /// Returns `true` if the type represents a Large Object (BYTE, TEXT, BLOB, or CLOB).
    pub const fn is_lob(&self) -> bool {
        matches!(self, Self::Byte | Self::Text | Self::Blob | Self::Clob)
    }
}

impl TryFrom<u16> for ColumnType {
    type Error = Error;

    fn try_from(raw: u16) -> Result<Self> {
        Self::from_raw(raw).ok_or_else(|| {
            Error::Protocol(format!("Unknown Informix column type: {raw}"))
        })
    }
}

impl fmt::Display for ColumnType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}
