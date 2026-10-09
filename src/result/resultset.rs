//! Informix Query Result Set container.

use crate::result::column::ColumnInfo;
use crate::result::row::Row;

/// Result set containing columns, rows, and execution metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Row>,
    pub rows_affected: i64,
    pub generated_serial: Option<i64>,
    pub warnings: u16,
}

impl QueryResult {
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

    pub fn empty(rows_affected: i64) -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected,
            generated_serial: None,
            warnings: 0,
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

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
