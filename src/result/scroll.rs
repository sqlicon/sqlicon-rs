//! Scrollable, positionable cursor fetch (`FETCH FIRST`/`LAST`/`ABSOLUTE`/`RELATIVE`).
//!
//! Mirrors `libsqli`'s scroll-insensitive cursor support (`sqli_result_first`/`last`/
//! `absolute`/`relative`/`previous`/`next` in `sqli_result.c`), which is opt-in per cursor:
//! `SQ_SCROLL` (opcode 24, no payload) is sent once at `SQ_OPEN` time
//! (see [`Connection::query_scrollable`](crate::connection::client::Connection::query_scrollable)),
//! and positioned fetches use `SQ_SFETCH` (opcode 23) instead of `SQ_NFETCH`. Only two
//! direction codes are used for real navigation on the wire -- `SFETCH_LAST` and
//! `SFETCH_ABSOLUTE` (`sqli_result.c:12-16`) -- `FIRST`/`PREVIOUS`/`NEXT`/`RELATIVE` are all
//! synthesized client-side as `ABSOLUTE(n)` calls, exactly as `libsqli` does.
//!
//! Unlike `libsqli`'s own `sqli_query`/`sqli_execute` convenience loop (which always fetches
//! a cursor to full exhaustion regardless of scrollability), [`ScrollCursor`] fetches
//! incrementally like [`RowStream`](crate::result::RowStream): [`ScrollCursor::absolute`]
//! is the one primitive every other move reduces to -- a target inside the currently
//! buffered batch repositions for free, otherwise one `SQ_SFETCH` round trip replaces the
//! batch. This deliberately skips C's sequential/continuation direction code `1` (reserved
//! there for eagerly filling a local buffer); every move here is either free or an
//! independently-correct `ABSOLUTE`/`LAST` round trip.

use crate::connection::client::Connection;
use crate::error::Result;
use crate::result::column::ColumnInfo;
use crate::result::row::Row;

/// `SQ_SFETCH` direction code: position at the last row (`sqli_result.c`'s
/// `SQLI_SFETCH_LAST`).
pub(crate) const SFETCH_LAST: u16 = 4;
/// `SQ_SFETCH` direction code: position at an absolute 1-based row number
/// (`sqli_result.c`'s `SQLI_SFETCH_ABSOLUTE`).
pub(crate) const SFETCH_ABSOLUTE: u16 = 6;

/// A positionable cursor over a result set opened with [`Connection::query_scrollable`]/
/// [`Connection::query_stmt_scrollable`](crate::connection::client::Connection::query_stmt_scrollable).
pub struct ScrollCursor<'a> {
    conn: Option<&'a mut Connection>,
    stmt_id: u16,
    columns: Vec<ColumnInfo>,
    /// Currently buffered batch, as returned by the last fetch.
    rows: Vec<Row>,
    /// 1-based absolute position of `rows[0]`; `0` if `rows` is empty/stale.
    rows_start: i64,
    /// 1-based absolute position of the current row; `0` if not positioned on a row
    /// (before-first or after-last -- see `at_after_last` to tell them apart).
    position: i64,
    at_after_last: bool,
}

impl<'a> ScrollCursor<'a> {
    pub(crate) fn new(conn: &'a mut Connection, stmt_id: u16, columns: Vec<ColumnInfo>) -> Self {
        Self {
            conn: Some(conn),
            stmt_id,
            columns,
            rows: Vec::new(),
            rows_start: 0,
            position: 0,
            at_after_last: false,
        }
    }

    /// Returns the column descriptors for rows yielded by this cursor.
    pub fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    /// Returns the 1-based absolute position of the current row, or `None` if positioned
    /// before the first row or after the last.
    pub fn position(&self) -> Option<i64> {
        (self.position > 0).then_some(self.position)
    }

    fn current(&self) -> Option<Row> {
        if self.position <= 0 || self.rows_start <= 0 {
            return None;
        }
        let idx = (self.position - self.rows_start) as usize;
        self.rows.get(idx).cloned()
    }

    fn borrow_conn(&mut self) -> Result<&mut Connection> {
        self.conn
            .as_deref_mut()
            .ok_or_else(|| crate::error::Error::InvalidState("scroll cursor is closed".into()))
    }

    /// Moves to row 1.
    pub async fn first(&mut self) -> Result<Option<Row>> {
        self.absolute(1).await
    }

    /// Moves to the last row.
    ///
    /// `SQ_SFETCH(LAST)` reliably returns the last row's *content*, but (observed against a
    /// live server) its `DONE` packet does not always carry a usable row count for the
    /// client to learn that row's absolute position from -- the same `rows_affected` field
    /// `libsqli`'s `sqli_result_last` depends on. When that happens, the absolute position
    /// is instead discovered with a bounded exponential-then-binary search over
    /// [`absolute`](Self::absolute) probes, which only relies on the already-verified
    /// `ABSOLUTE` direction.
    pub async fn last(&mut self) -> Result<Option<Row>> {
        let stmt_id = self.stmt_id;
        let columns = self.columns.clone();
        let conn = self.borrow_conn()?;
        let turn = conn.sfetch_turn(stmt_id, &columns, SFETCH_LAST, 0).await?;
        if turn.rows.is_empty() {
            self.rows.clear();
            self.rows_start = 0;
            self.position = 0;
            self.at_after_last = true;
            return Ok(None);
        }
        let total = if turn.rows_affected > 0 {
            turn.rows_affected
        } else {
            self.discover_total_by_search().await?
        };
        let batch_len = turn.rows.len() as i64;
        self.rows_start = (total - batch_len + 1).max(1);
        self.position = total;
        self.at_after_last = false;
        self.rows = turn.rows;
        Ok(self.current())
    }

    /// Finds the largest `n` for which `ABSOLUTE(n)` still returns a row, via exponential
    /// growth followed by a binary search -- `O(log total)` round trips, none of which touch
    /// this cursor's buffered state (each is a throwaway probe).
    async fn discover_total_by_search(&mut self) -> Result<i64> {
        let mut lo: i64 = 1; // the LAST fetch already proved row 1 isn't past the end
        let mut hi: i64 = 2;
        loop {
            if self.probe_absolute(hi).await? {
                lo = hi;
                if hi > i32::MAX as i64 / 2 {
                    hi = hi.saturating_add(1);
                    break;
                }
                hi *= 2;
            } else {
                break;
            }
        }
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if self.probe_absolute(mid).await? {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }

    /// Issues a throwaway `ABSOLUTE(n)` fetch without touching this cursor's buffered state.
    async fn probe_absolute(&mut self, n: i64) -> Result<bool> {
        let stmt_id = self.stmt_id;
        let columns = self.columns.clone();
        let conn = self.borrow_conn()?;
        let turn = conn
            .sfetch_turn(stmt_id, &columns, SFETCH_ABSOLUTE, n as i32)
            .await?;
        Ok(!turn.rows.is_empty())
    }

    /// Moves to the 1-based absolute row `row_1based`. Returns `Ok(None)` if `row_1based`
    /// is out of range (including `<= 0`), leaving the cursor positioned after the last row.
    pub async fn absolute(&mut self, row_1based: i64) -> Result<Option<Row>> {
        if row_1based <= 0 {
            self.position = 0;
            self.at_after_last = false;
            return Ok(None);
        }
        if self.rows_start > 0
            && row_1based >= self.rows_start
            && row_1based < self.rows_start + self.rows.len() as i64
        {
            self.position = row_1based;
            self.at_after_last = false;
            return Ok(self.current());
        }
        let stmt_id = self.stmt_id;
        let columns = self.columns.clone();
        let conn = self.borrow_conn()?;
        let turn = conn
            .sfetch_turn(stmt_id, &columns, SFETCH_ABSOLUTE, row_1based as i32)
            .await?;
        self.apply_absolute_refetch(turn, row_1based)
    }

    /// Moves `offset` rows from the current position (negative moves backward).
    pub async fn relative(&mut self, offset: i64) -> Result<Option<Row>> {
        if self.position <= 0 {
            return Ok(None);
        }
        self.absolute(self.position + offset).await
    }

    /// Moves one row backward.
    pub async fn previous(&mut self) -> Result<Option<Row>> {
        if self.at_after_last {
            return self.last().await;
        }
        if self.position <= 0 {
            return Ok(None);
        }
        self.absolute(self.position - 1).await
    }

    /// Moves one row forward. A no-op returning `Ok(None)` once already positioned after the
    /// last row (matching `libsqli`'s `sqli_result_next_internal`, which never wraps around).
    pub async fn next(&mut self) -> Result<Option<Row>> {
        if self.at_after_last {
            return Ok(None);
        }
        let target = if self.position > 0 { self.position + 1 } else { 1 };
        self.absolute(target).await
    }

    /// Applies the result of an `SQ_SFETCH(ABSOLUTE, n)` round trip, replacing the buffered
    /// batch. The batch is trusted to start exactly at `n`, matching `libsqli` setting
    /// `absolute_row_num = index` after an `ABSOLUTE` refetch.
    fn apply_absolute_refetch(&mut self, turn: crate::connection::client::TurnResult, n: i64) -> Result<Option<Row>> {
        if turn.rows.is_empty() {
            self.rows.clear();
            self.rows_start = 0;
            self.position = 0;
            self.at_after_last = true;
            return Ok(None);
        }
        self.rows_start = n;
        self.position = n;
        self.at_after_last = false;
        self.rows = turn.rows;
        Ok(self.current())
    }

    /// Closes the underlying cursor and deallocates statement resources on the server.
    pub async fn close(&mut self) -> Result<()> {
        if let Some(conn) = self.conn.take() {
            conn.close_statement(self.stmt_id).await?;
        }
        Ok(())
    }
}
