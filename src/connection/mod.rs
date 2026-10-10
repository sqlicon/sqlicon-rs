//! Connection management and handshake protocol.

pub mod cancel;
pub mod capabilities;
pub mod client;
pub mod handshake;
pub mod params;
pub mod pool;
pub mod sqlhosts;

pub use cancel::{CancelDisposition, CancelOperation, CancelOutcome, CancelRequester, CancelSnapshot};
pub use capabilities::ServerCapabilities;
pub use client::{BatchItemResult, BatchResult, Connection, ConnectionState, IsolationLevel};
pub use handshake::*;
pub use params::ConnectParams;
pub use pool::{ConnectionPool, PoolOptions, PooledConnection};
pub use sqlhosts::{find_sqlhosts_entry, parse_sqlhosts, SqlhostsEntry};
