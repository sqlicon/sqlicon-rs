//! # sqlicon-rs
//!
//! An asynchronous Informix SQLI protocol client library and interactive CLI written in Rust.
//!
//! This crate provides native, non-blocking communication with IBM Informix Dynamic Server engines
//! across Linux and Windows platforms without requiring the IBM Informix Client-SDK or CSDK runtime.
//!
//! ## Key Capabilities
//!
//! - **Native Wire Protocol**: Direct implementation of the Informix SQLI wire format (`onsoctcp`),
//!   session layer framing (`SL_HEADER`, `CONREQ`, `CONACC`), capabilities exchange (`SQ_PROTOCOLS`),
//!   and ASC encoding (`asc_encode`/`asc_decode`).
//! - **Complete Data Types**: First-class support for all Informix native and extended types:
//!   - Numeric: `SMALLINT`, `INTEGER`, `BIGINT`, `INT8`, `SERIAL`, `SERIAL8`, `BIGSERIAL`, `FLOAT`, `SMALLFLOAT`, `DECIMAL`, `MONEY`.
//!   - Temporal: `DATE`, `DATETIME` (all qualifiers `YEAR TO FRACTION(5)`), `INTERVAL`.
//!   - Text & Character: `CHAR`, `VARCHAR`, `LVARCHAR`, `NCHAR`, `NVARCHAR`, `BOOLEAN`.
//!   - Smart Large Objects: `BLOB` and `CLOB` with full random access seeking, chunked reading, and writing via `SQ_LODATA`.
//! - **Transactions & Savepoints**: ANSI and Informix transactions (`begin`, `commit`, `rollback`), named savepoints
//!   (`savepoint_set`, `savepoint_release`, `savepoint_rollback`), and isolation levels (`CommittedRead`, `RepeatableRead`,
//!   `Serializable`, `CursorStability`).
//! - **Batch Execution**: Statement batching (`SQ_BATCHSTART`/`SQ_BATCHEND`) and prepared parameter batching.
//! - **Out-of-band Query Cancellation**: Single-use `CancelOperation`/`CancelRequester` pair,
//!   scoped to one prepared `UPDATE`/`DELETE`/`INSERT` executed via
//!   [`Connection::execute_stmt_cancelable`](connection::client::Connection::execute_stmt_cancelable),
//!   using a TCP urgent out-of-band interrupt (`MSG_OOB`) to abort it on the server (`SQLCODE -213`).
//! - **Complete Error Catalog**: Full embedded 3,412-error Informix message catalog with dictionary expansion
//!   matching C11 `libsqli`.
//! - **Profile Management**: Profile store with AES-256-GCM encryption derived from machine-specific hardware IDs
//!   (`MachineGuid` on Windows, `/etc/machine-id` on Linux).
//!
//! ## Thread and Task Safety
//!
//! - A [`Connection`] instance represents a single TCP session and must be accessed exclusively (`&mut self`).
//! - [`CancelRequester`] is `Send + Sync + Clone` and can be moved to another task or thread to request cancellation of exactly one bound cancelable execute call.
//! - Prepared statements ([`PreparedStatement`]) and query results ([`QueryResult`]) are owned and can be freely moved across asynchronous tasks.
//!
//! ## Quick Start Example
//!
//! ```no_run
//! use sqlicon_rs::{ConnectParams, Connection};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Build connection parameters (or parse from URI)
//!     let params = ConnectParams::new("127.0.0.1", "9089", "ol_iep")
//!         .database("sqli_log_test")
//!         .credentials("admin", "admin")
//!         .locales("de_DE.1252", "de_DE.1252");
//!
//!     // Connect to Informix engine
//!     let mut conn = Connection::connect(params).await?;
//!
//!     // Execute query
//!     let res = conn.query("SELECT FIRST 5 tabid, tabname FROM systables").await?;
//!     for row in &res.rows {
//!         println!("tabid: {}, name: {}", row.get_display_value(0)?, row.get_display_value(1)?);
//!     }
//!
//!     Ok(())
//! }
//! ```

pub mod connection;
pub mod error;
pub mod profile;
pub mod protocol;
pub mod result;
pub mod statement;
pub mod transport;
pub mod types;

pub use connection::{
    BatchItemResult, BatchResult, CancelDisposition, CancelOperation, CancelOutcome,
    CancelRequester, CancelSnapshot, ConnectParams, Connection, ConnectionPool, ConnectionState,
    IsolationLevel, PoolOptions, PooledConnection, ServerCapabilities, SslMode, Transaction,
};
pub use error::{lookup_error_message, Error, Result, Status};
pub use profile::{Profile, ProfileStore};
pub use protocol::{Opcode, SlHeader};
pub use result::{ColumnInfo, ExecuteResult, QueryResult, Row, RowStream, ScrollCursor};
pub use statement::{CallResult, CallStatement, ParamMode, ParameterValue, PreparedStatement};
pub use transport::SqliStream;
pub use types::{ColumnType, Date, DateTime, Decimal, Interval};


