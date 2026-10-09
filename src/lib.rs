//! # sqlicon
//!
//! An asynchronous Informix SQLI client driver written in Rust.

pub mod error;
pub mod protocol;
pub mod types;

pub use error::{Error, Result, Status};
pub use protocol::{Opcode, SlHeader};
pub use types::{ColumnType, Date, DateTime, Decimal, Interval};
