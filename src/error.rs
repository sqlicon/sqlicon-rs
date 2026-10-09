use std::fmt;
use thiserror::Error;

/// Status codes corresponding to C libsqli's `sqli_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Status {
    Ok = 0,
    Err = 1,
    Eof = 2,
    Timeout = 3,
    AuthFail = 4,
    ProtoError = 5,
    IoError = 6,
    AllocFail = 7,
    InvalidState = 8,
    InvalidArgument = 9,
    OutOfRange = 10,
    Inexact = 11,
    BufferTooSmall = 12,
    NullValue = 13,
    LimitExceeded = 14,
    MetadataUnavailable = 15,
    TypeMismatch = 16,
    Canceled = 17,
    Unsupported = 18,
    Unknown = 255,
}

impl Status {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Ok => "SQLI_OK",
            Self::Err => "SQLI_ERR",
            Self::Eof => "SQLI_EOF",
            Self::Timeout => "SQLI_TIMEOUT",
            Self::AuthFail => "SQLI_AUTH_FAIL",
            Self::ProtoError => "SQLI_PROTO_ERROR",
            Self::IoError => "SQLI_IO_ERROR",
            Self::AllocFail => "SQLI_ALLOC_FAIL",
            Self::InvalidState => "SQLI_INVALID_STATE",
            Self::InvalidArgument => "SQLI_INVALID_ARGUMENT",
            Self::OutOfRange => "SQLI_OUT_OF_RANGE",
            Self::Inexact => "SQLI_INEXACT",
            Self::BufferTooSmall => "SQLI_BUFFER_TOO_SMALL",
            Self::NullValue => "SQLI_NULL_VALUE",
            Self::LimitExceeded => "SQLI_LIMIT_EXCEEDED",
            Self::MetadataUnavailable => "SQLI_METADATA_UNAVAILABLE",
            Self::TypeMismatch => "SQLI_TYPE_MISMATCH",
            Self::Canceled => "SQLI_CANCELED",
            Self::Unsupported => "SQLI_UNSUPPORTED",
            Self::Unknown => "SQLI_UNKNOWN_STATUS",
        }
    }

    pub const fn description(&self) -> &'static str {
        match self {
            Self::Ok => "Success",
            Self::Err => "Operation failed",
            Self::Eof => "End of result",
            Self::Timeout => "Operation timed out",
            Self::AuthFail => "Authentication failed",
            Self::ProtoError => "Invalid protocol data",
            Self::IoError => "Input/output failure",
            Self::AllocFail => "Memory allocation failed",
            Self::InvalidState => "Operation is invalid in the current state",
            Self::InvalidArgument => "Invalid argument",
            Self::OutOfRange => "Value or index is outside the supported range",
            Self::Inexact => "Conversion would discard nonzero digits",
            Self::BufferTooSmall => "Output buffer is too small",
            Self::NullValue => "Value is SQL NULL",
            Self::LimitExceeded => "Resource limit exceeded",
            Self::MetadataUnavailable => "Requested metadata is unavailable",
            Self::TypeMismatch => "Column type does not support this conversion",
            Self::Canceled => "Execution canceled; inspect cancellation outcome",
            Self::Unsupported => "Operation or transport is not supported",
            Self::Unknown => "Unknown library status",
        }
    }

    pub const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Ok,
            1 => Self::Err,
            2 => Self::Eof,
            3 => Self::Timeout,
            4 => Self::AuthFail,
            5 => Self::ProtoError,
            6 => Self::IoError,
            7 => Self::AllocFail,
            8 => Self::InvalidState,
            9 => Self::InvalidArgument,
            10 => Self::OutOfRange,
            11 => Self::Inexact,
            12 => Self::BufferTooSmall,
            13 => Self::NullValue,
            14 => Self::LimitExceeded,
            15 => Self::MetadataUnavailable,
            16 => Self::TypeMismatch,
            17 => Self::Canceled,
            18 => Self::Unsupported,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name(), self.description())
    }
}

/// Rich diagnostic information returned by Informix database server errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerErrorInfo {
    pub sqlcode: i32,
    pub isamcode: i32,
    pub sqlstate: String,
    pub opcode: Option<u16>,
    pub message: String,
}

/// Idiomatic Rust error type for sqlicon.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Buffer too small: needed {needed}, available {available}")]
    BufferTooSmall { needed: usize, available: usize },

    #[error("Server error (SQLCODE {sqlcode}, ISAM {isamcode}): {message}")]
    Server {
        sqlcode: i32,
        isamcode: i32,
        sqlstate: String,
        message: String,
    },

    #[error("Authentication failed: {0}")]
    Authentication(String),

    #[error("Connection invalid state: {0}")]
    InvalidState(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Value out of range: {0}")]
    OutOfRange(String),

    #[error("Conversion is inexact: {0}")]
    Inexact(String),

    #[error("Type mismatch: expected {expected}, found {found}")]
    TypeMismatch {
        expected: &'static str,
        found: &'static str,
    },

    #[error("SQL NULL value encountered")]
    NullValue,

    #[error("Operation canceled")]
    Canceled,

    #[error("Unsupported operation: {0}")]
    Unsupported(String),
}

impl Error {
    pub fn status(&self) -> Status {
        match self {
            Self::Io(_) => Status::IoError,
            Self::Protocol(_) => Status::ProtoError,
            Self::BufferTooSmall { .. } => Status::BufferTooSmall,
            Self::Server { .. } => Status::Err,
            Self::Authentication(_) => Status::AuthFail,
            Self::InvalidState(_) => Status::InvalidState,
            Self::InvalidArgument(_) => Status::InvalidArgument,
            Self::OutOfRange(_) => Status::OutOfRange,
            Self::Inexact(_) => Status::Inexact,
            Self::TypeMismatch { .. } => Status::TypeMismatch,
            Self::NullValue => Status::NullValue,
            Self::Canceled => Status::Canceled,
            Self::Unsupported(_) => Status::Unsupported,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
