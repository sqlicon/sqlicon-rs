//! Connection management and handshake protocol.

pub mod cancel;
pub mod capabilities;
pub mod client;
pub mod handshake;
pub mod params;
pub mod pool;
pub mod sblob;
pub mod sqlhosts;

pub use cancel::{CancelDisposition, CancelOperation, CancelOutcome, CancelRequester, CancelSnapshot};
pub use capabilities::ServerCapabilities;
pub use client::{BatchItemResult, BatchResult, Connection, ConnectionState, IsolationLevel};
pub use handshake::*;
pub use params::ConnectParams;
pub use pool::{ConnectionPool, PoolOptions, PooledConnection};
pub use sblob::{SblobHandle, SblobOpenMode, SblobOptions, SblobType, IFX_LO_SPEC_SIZE, SBLOB_LOCATOR_MAX};
pub use sqlhosts::{find_sqlhosts_entry, parse_sqlhosts, SqlhostsEntry};
