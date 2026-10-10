//! Informix Prepared Statement.

use crate::result::column::ColumnInfo;

/// Represents a prepared statement handle on the Informix server.
///
/// Contains statement metadata including statement ID, expected parameter placeholder count,
/// and output column schemas (if the statement is a query).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedStatement {
    pub(crate) stmt_id: u16,
    pub(crate) statement_type: u16,
    pub(crate) parameter_count: usize,
    pub(crate) columns: Vec<ColumnInfo>,
}

impl PreparedStatement {
    /// Creates a new `PreparedStatement` instance.
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

    /// Returns the internal server statement identifier (`SQ_ID`).
    pub fn id(&self) -> u16 {
        self.stmt_id
    }

    /// Returns the number of parameter placeholders (`?`) expected by this statement.
    pub fn parameter_count(&self) -> usize {
        self.parameter_count
    }

    /// Returns the list of output column descriptors for query statements.
    pub fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    /// Returns `true` if the statement produces result rows (e.g. `SELECT`).
    pub fn is_query(&self) -> bool {
        !self.columns.is_empty()
    }

    /// Returns the Informix statement type code (e.g. 2 for SELECT).
    pub fn statement_type(&self) -> u16 {
        self.statement_type
    }
}
