//! Asynchronous connection pool for Informix connections.
//!
//! Provides [`ConnectionPool`] and [`PoolOptions`] for acquiring and recycling connections
//! safely across concurrent tasks with timeouts, health validation, and automatic reconnection.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

use crate::connection::client::Connection;
use crate::connection::params::ConnectParams;
use crate::error::{Error, Result};

/// Configuration options for [`ConnectionPool`].
#[derive(Debug, Clone)]
pub struct PoolOptions {
    /// Maximum number of active connections allowed concurrently. Default: `10`.
    pub max_connections: usize,
    /// Minimum number of idle connections to maintain. Default: `1`.
    pub min_idle: usize,
    /// Maximum duration a connection may remain idle before being recycled. Default: `300s`.
    pub idle_timeout: Duration,
    /// Maximum wait time when acquiring a connection from the pool. Default: `30s`.
    pub acquire_timeout: Duration,
    /// Whether to perform a health probe (`SELECT FIRST 1 1 FROM systables`) on checkout. Default: `true`.
    pub test_on_borrow: bool,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_idle: 1,
            idle_timeout: Duration::from_secs(300),
            acquire_timeout: Duration::from_secs(30),
            test_on_borrow: true,
        }
    }
}

impl PoolOptions {
    /// Sets the maximum connection limit.
    pub fn max_connections(mut self, limit: usize) -> Self {
        self.max_connections = limit.max(1);
        self
    }

    /// Sets the acquire timeout duration.
    pub fn acquire_timeout(mut self, timeout: Duration) -> Self {
        self.acquire_timeout = timeout;
        self
    }

    /// Sets the idle timeout duration.
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Configures whether connections are verified with a ping query on borrow.
    pub fn test_on_borrow(mut self, enabled: bool) -> Self {
        self.test_on_borrow = enabled;
        self
    }
}

struct IdleConnection {
    conn: Connection,
    last_used: Instant,
}

struct PoolInner {
    params: ConnectParams,
    options: PoolOptions,
    idle_queue: Mutex<VecDeque<IdleConnection>>,
    semaphore: Arc<Semaphore>,
}

/// An asynchronous, thread-safe connection pool for Informix.
#[derive(Clone)]
pub struct ConnectionPool {
    inner: Arc<PoolInner>,
}

impl ConnectionPool {
    /// Creates a new connection pool with default options for the given connection parameters.
    pub async fn new(params: ConnectParams) -> Result<Self> {
        Self::with_options(params, PoolOptions::default()).await
    }

    /// Creates a new connection pool with custom options.
    pub async fn with_options(params: ConnectParams, options: PoolOptions) -> Result<Self> {
        let max_connections = options.max_connections.max(1);
        let inner = Arc::new(PoolInner {
            params,
            options,
            idle_queue: Mutex::new(VecDeque::new()),
            semaphore: Arc::new(Semaphore::new(max_connections)),
        });

        let pool = Self { inner };

        // Warm up minimum idle connections if requested
        if pool.inner.options.min_idle > 0 {
            let warmup_count = pool.inner.options.min_idle.min(max_connections);
            for _ in 0..warmup_count {
                if let Ok(conn) = Connection::connect(pool.inner.params.clone()).await {
                    let mut q = pool.inner.idle_queue.lock().await;
                    q.push_back(IdleConnection {
                        conn,
                        last_used: Instant::now(),
                    });
                }
            }
        }

        Ok(pool)
    }

    /// Acquires a connection from the pool, waiting up to [`PoolOptions::acquire_timeout`].
    pub async fn acquire(&self) -> Result<PooledConnection> {
        tokio::time::timeout(self.inner.options.acquire_timeout, self.acquire_internal())
            .await
            .map_err(|_| Error::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "connection pool acquire timeout")))?
    }

    async fn acquire_internal(&self) -> Result<PooledConnection> {
        // Acquire permit from the semaphore
        let permit = self
            .inner
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Protocol("connection pool closed".into()))?;

        // Try to pop an existing idle connection
        loop {
            let item_opt = {
                let mut q = self.inner.idle_queue.lock().await;
                q.pop_front()
            };

            match item_opt {
                Some(mut idle) => {
                    // Check idle timeout
                    if idle.last_used.elapsed() > self.inner.options.idle_timeout {
                        continue; // Discard expired connection and loop for next
                    }

                    // Check health if test_on_borrow is enabled
                    if self.inner.options.test_on_borrow && idle.conn.ping().await.is_err() {
                        continue; // Discard broken connection and loop
                    }

                    return Ok(PooledConnection {
                        conn: Some(idle.conn),
                        pool: self.inner.clone(),
                        _permit: permit,
                    });
                }
                None => {
                    // No idle connection available: create a new one
                    let conn = Connection::connect(self.inner.params.clone()).await?;
                    return Ok(PooledConnection {
                        conn: Some(conn),
                        pool: self.inner.clone(),
                        _permit: permit,
                    });
                }
            }
        }
    }

    /// Returns the number of currently idle connections in the pool.
    pub async fn idle_count(&self) -> usize {
        let q = self.inner.idle_queue.lock().await;
        q.len()
    }

    /// Returns the maximum allowed connections.
    pub fn max_connections(&self) -> usize {
        self.inner.options.max_connections
    }
}

/// A RAII guard for a pooled connection. Returns the connection back to the pool on drop.
pub struct PooledConnection {
    conn: Option<Connection>,
    pool: Arc<PoolInner>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl std::ops::Deref for PooledConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.conn.as_ref().expect("pooled connection deref")
    }
}

impl std::ops::DerefMut for PooledConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.conn.as_mut().expect("pooled connection deref_mut")
    }
}

impl Drop for PooledConnection {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            if conn.is_alive() {
                let pool = self.pool.clone();
                // Spawn a detached cleanup task to return the connection to the idle queue
                tokio::spawn(async move {
                    let mut q = pool.idle_queue.lock().await;
                    q.push_back(IdleConnection {
                        conn,
                        last_used: Instant::now(),
                    });
                });
            }
        }
    }
}
