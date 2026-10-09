//! # sqlicon
//!
//! An asynchronous Informix SQLI client driver written in Rust.

pub mod connection;
pub mod error;
pub mod profile;
pub mod protocol;
pub mod result;
pub mod statement;
pub mod transport;
pub mod types;

pub use connection::{
    ConnectParams, Connection, ConnectionState, IsolationLevel, ServerCapabilities,
};
pub use error::{lookup_error_message, Error, Result, Status};
pub use profile::{Profile, ProfileStore};
pub use protocol::{Opcode, SlHeader};
pub use result::{ColumnInfo, QueryResult, Row};
pub use statement::{ParameterValue, PreparedStatement};
pub use transport::SqliStream;
pub use types::{ColumnType, Date, DateTime, Decimal, Interval};
