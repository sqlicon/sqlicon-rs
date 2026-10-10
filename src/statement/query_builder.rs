//! `sqlx`-style fluent query builder: `query(sql).bind(a).bind(b).fetch_all(&mut conn)`.
//!
//! This is purely a convenience facade over [`Connection::prepare`]/
//! [`execute_stmt`](Connection::execute_stmt)/[`query_stmt`](Connection::query_stmt) -- it
//! prepares `sql` fresh on every terminal call (`execute`/`fetch_*`), the same as calling
//! those methods directly would. Reach for [`Connection::prepare`] yourself instead when a
//! statement is run many times and the parse cost should be paid only once.
//!
//! Unlike `sqlx::query`, this builder does not borrow a connection pool at construction
//! time -- the connection is supplied only to the terminal call, mirroring `sqlx`'s own
//! `.fetch_all(&pool)` shape. [`Connection::query`](crate::connection::client::Connection::query)
//! (no parameters, returns [`QueryResult`] directly) is a separate, older entry point
//! unrelated to this builder; use whichever fits the call site.

use crate::connection::client::Connection;
use crate::error::{Error, Result};
use crate::result::{QueryResult, Row};
use crate::statement::param::ParameterValue;
use crate::result::ExecuteResult;

/// Starts a fluent, parameter-binding query against `sql`. See the module docs.
pub fn query(sql: &str) -> Query<'_> {
    Query { sql, params: Vec::new() }
}

/// A SQL statement with positionally-bound parameters, not yet run against a connection.
/// Built via [`query`].
pub struct Query<'q> {
    sql: &'q str,
    params: Vec<ParameterValue>,
}

impl<'q> Query<'q> {
    /// Binds the next `?` placeholder, in declaration order. Accepts any value with a
    /// [`ParameterValue`] `From` impl (`i32`, `i64`, `f64`, `&str`, `String`, `Vec<u8>`,
    /// `Date`, `Option<T>`, ...), or a [`ParameterValue`] itself.
    pub fn bind(mut self, value: impl Into<ParameterValue>) -> Self {
        self.params.push(value.into());
        self
    }

    /// Prepares and runs a non-row-returning statement (INSERT/UPDATE/DELETE/DDL).
    pub async fn execute(self, conn: &mut Connection) -> Result<ExecuteResult> {
        let stmt = conn.prepare(self.sql).await?;
        conn.execute_stmt(&stmt, &self.params).await
    }

    /// Prepares and runs a row-returning statement, fetching every row.
    pub async fn fetch_all(self, conn: &mut Connection) -> Result<QueryResult> {
        let stmt = conn.prepare(self.sql).await?;
        conn.query_stmt(&stmt, &self.params).await
    }

    /// Fetches exactly one row, or [`Error::Eof`] if the statement returned none.
    pub async fn fetch_one(self, conn: &mut Connection) -> Result<Row> {
        let mut result = self.fetch_all(conn).await?;
        if result.rows.is_empty() {
            return Err(Error::Eof);
        }
        Ok(result.rows.remove(0))
    }

    /// Fetches at most one row: `Ok(None)` if the statement returned none, rather than
    /// [`Error::Eof`].
    pub async fn fetch_optional(self, conn: &mut Connection) -> Result<Option<Row>> {
        let mut result = self.fetch_all(conn).await?;
        if result.rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some(result.rows.remove(0)))
        }
    }
}
