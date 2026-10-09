//! Informix Prepared Statement.

use crate::result::column::ColumnInfo;

/// Represents a prepared statement on the Informix server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedStatement {
    pub(crate) stmt_id: u16,
    pub(crate) statement_type: u16,
    pub(crate) parameter_count: usize,
    pub(crate) columns: Vec<ColumnInfo>,
}

impl PreparedStatement {
    pub fn new(
        stmt_id: u16,
        statement_type: u16,
        parameter_count: usize,
        columns: Vec<ColumnInfo>,
    ) -> Self {
        Self {
            stmt_id,
            statement_type,
            parameter_count,
            columns,
        }
    }

    pub fn id(&self) -> u16 {
        self.stmt_id
    }

    pub fn parameter_count(&self) -> usize {
        self.parameter_count
    }

    pub fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    pub fn is_query(&self) -> bool {
        !self.columns.is_empty()
    }
}
