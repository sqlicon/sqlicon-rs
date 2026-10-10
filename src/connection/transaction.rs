//! RAII-style scoping for [`Connection::begin`]/`commit`/`rollback`.

use crate::connection::client::Connection;
use crate::error::Result;

/// A scope for the transaction started by [`Connection::transaction`].
///
/// Must be finished explicitly with [`commit`](Self::commit) or [`rollback`](Self::rollback)
/// -- marked `#[must_use]` so the compiler flags a `Transaction` that's dropped without
/// either. This is *not* a full auto-rollback guard: because `Connection` is borrowed here
/// (not owned, unlike a connection checked out of [`crate::ConnectionPool`]), `Drop` cannot
/// send an async `ROLLBACK` over the wire without either blocking the calling thread or
/// spawning a `'static` task it can't own data for. Blocking in `Drop` is exactly what this
/// crate's own coding guideline (`doc/coding_rust.md`) forbids, so an unfinished
/// `Transaction` is only logged via `tracing::warn!` on drop -- the transaction itself
/// remains genuinely open on the server until the connection is reused, closed, or the
/// caller finishes it. [`Connection::begin`]/[`commit`](Connection::commit)/
/// [`rollback`](Connection::rollback) remain available directly for callers who don't want
/// this wrapper.
#[must_use = "a Transaction must be finished with `commit()` or `rollback()`, or it leaks \
              open on the server (only logged via `tracing::warn!`, not auto-rolled-back)"]
pub struct Transaction<'a> {
    conn: &'a mut Connection,
    finished: bool,
}

impl<'a> Transaction<'a> {
    pub(crate) fn new(conn: &'a mut Connection) -> Self {
        Self { conn, finished: false }
    }

    /// Commits the transaction.
    pub async fn commit(mut self) -> Result<()> {
        self.conn.commit().await?;
        self.finished = true;
        Ok(())
    }

    /// Rolls back the transaction.
    pub async fn rollback(mut self) -> Result<()> {
        self.conn.rollback().await?;
        self.finished = true;
        Ok(())
    }

    /// Borrows the underlying connection to run statements within the transaction.
    pub fn connection(&mut self) -> &mut Connection {
        self.conn
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            tracing::warn!(
                "Transaction dropped without an explicit commit() or rollback(); \
                 it remains open on the server"
            );
        }
    }
}
