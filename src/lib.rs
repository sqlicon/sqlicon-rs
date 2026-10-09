//! # sqlicon
//!
//! An asynchronous Informix SQLI client driver written in Rust.

pub mod connection;
pub mod error;
pub mod protocol;
pub mod result;
pub mod statement;
pub mod transport;
pub mod types;

pub use connection::{ConnectParams, Connection, ConnectionState, ServerCapabilities};
pub use error::{Error, Result, Status};
pub use protocol::{Opcode, SlHeader};
pub use result::{ColumnInfo, QueryResult, Row};
pub use statement::PreparedStatement;
pub use transport::SqliStream;
pub use types::{ColumnType, Date, DateTime, Decimal, Interval};
