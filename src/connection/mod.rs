//! Connection management and handshake protocol.

pub mod capabilities;
pub mod client;
pub mod handshake;
pub mod params;
pub mod pool;

pub use capabilities::ServerCapabilities;
pub use client::{
    BatchItemResult, BatchResult, CancelHandle, Connection, ConnectionState, IsolationLevel,
};
pub use handshake::*;
pub use params::ConnectParams;
pub use pool::{ConnectionPool, PoolOptions, PooledConnection};
