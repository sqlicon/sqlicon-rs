//! Informix Query Result Set container.

use crate::result::column::ColumnInfo;
use crate::result::row::Row;

/// Complete result set of an Informix query execution, containing schema metadata,
/// row tuples, and server status indicators.
///
/// Implements [`IntoIterator`] for convenient traversal over rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult {
    /// Column descriptors defining names, types, flags, and field offsets.
    pub columns: Vec<ColumnInfo>,
    /// Decoded row tuples returned by the server.
    pub rows: Vec<Row>,
    /// Number of rows affected (for DML) or retrieved (for queries).
    pub rows_affected: i64,
    /// Auto-generated `SERIAL`, `SERIAL8`, or `BIGSERIAL` key value if applicable.
    pub generated_serial: Option<i64>,
    /// Informix SQL warning bitmask (e.g. truncated character data).
    pub warnings: u16,
}

impl QueryResult {
    /// Creates a new `QueryResult` with the provided metadata and rows.
    pub fn new(
        columns: Vec<ColumnInfo>,
        rows: Vec<Row>,
        rows_affected: i64,
        generated_serial: Option<i64>,
        warnings: u16,
    ) -> Self {
        Self {
            columns,
            rows,
            rows_affected,
            generated_serial,
            warnings,
        }
    }

    /// Creates an empty `QueryResult` representing a statement execution with no output tuples.
    pub fn empty(rows_affected: i64) -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected,
            generated_serial: None,
            warnings: 0,
        }
    }

    /// Returns the number of rows contained in this result set.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Returns `true` if the result set contains zero rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Returns an iterator over references to the rows.
    pub fn iter(&self) -> std::slice::Iter<'_, Row> {
        self.rows.iter()
    }
}

impl IntoIterator for QueryResult {
    type Item = Row;
    type IntoIter = std::vec::IntoIter<Row>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.into_iter()
    }
}

impl<'a> IntoIterator for &'a QueryResult {
    type Item = &'a Row;
    type IntoIter = std::slice::Iter<'a, Row>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}
