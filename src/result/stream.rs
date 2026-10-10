//! Streaming query cursor implementation for Informix.
//!
//! Provides [`RowStream`] which fetches rows asynchronously in chunks from the Informix
//! server as requested, enabling processing of large result sets with minimal memory usage.

use std::collections::VecDeque;

use crate::connection::client::Connection;
use crate::error::Result;
use crate::result::column::ColumnInfo;
use crate::result::row::Row;

/// An asynchronous stream of [`Row`] items yielded from a live cursor.
pub struct RowStream<'a> {
    conn: Option<&'a mut Connection>,
    stmt_id: u16,
    columns: Vec<ColumnInfo>,
    buffered_rows: VecDeque<Row>,
    done: bool,
}

impl<'a> RowStream<'a> {
    pub(crate) fn new(
        conn: &'a mut Connection,
        stmt_id: u16,
        columns: Vec<ColumnInfo>,
    ) -> Self {
        Self {
            conn: Some(conn),
            stmt_id,
            columns,
            buffered_rows: VecDeque::new(),
            done: false,
        }
    }

    /// Returns the column descriptors for rows yielded by this stream.
    pub fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    /// Fetches the next [`Row`] from the stream.
    ///
    /// Returns:
    /// - `Ok(Some(row))` if a row is available.
    /// - `Ok(None)` when the cursor has reached the end of the result set.
    /// - `Err(e)` if a network or SQL error occurred.
    pub async fn next(&mut self) -> Result<Option<Row>> {
        if let Some(row) = self.buffered_rows.pop_front() {
            return Ok(Some(row));
        }

        if self.done {
            return Ok(None);
        }

        let conn = match self.conn.as_deref_mut() {
            Some(c) => c,
            None => return Ok(None),
        };

        let turn = conn.fetch_turn(self.stmt_id, &self.columns).await?;

        if turn.saw_done || turn.rows.is_empty() {
            self.done = true;
            // Statement cursor cleanup
            let _ = conn.close_statement(self.stmt_id).await;
        }

        for row in turn.rows {
            self.buffered_rows.push_back(row);
        }

        Ok(self.buffered_rows.pop_front())
    }

    /// Closes the underlying cursor and deallocates statement resources on the server.
    pub async fn close(&mut self) -> Result<()> {
        if !self.done {
            self.done = true;
            if let Some(conn) = self.conn.take() {
                conn.close_statement(self.stmt_id).await?;
            }
        }
        Ok(())
    }
}
