//! Informix Prepared Statement.

use crate::protocol::constants::{STMT_SELECT, STMT_SELECT_RETURNING};
use crate::result::column::ColumnInfo;
use crate::types::column_type::ColumnType;

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
    ///
    /// For a non-query statement (DML/DDL), the server's single `SQ_DESCRIBE` response
    /// describes input *parameters* instead of output columns, and this same list holds
    /// those parameter type descriptors — matching `libsqli`'s dual-purpose reuse of the
    /// DESCRIBE field array (`sqli_prepare.c`'s comment: "DESCRIBE for DML can describe
    /// input parameters"). Use [`param_server_type`](Self::param_server_type) to read it in
    /// that case; calling this accessor on a non-query statement returns the same raw data
    /// mislabeled as columns, so prefer `is_query()` to decide which interpretation applies.
    pub fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    /// Returns the server-declared type for the parameter at `index`, if this statement's
    /// `SQ_DESCRIBE` response was interpreted as parameter descriptors (i.e. not a query).
    /// Used to pick the wire encoding when binding — e.g. a server-declared legacy
    /// `BYTE`/`TEXT` parameter must be sent as a LOB locator descriptor, not the value's
    /// default encoding. Mirrors `libsqli`'s `stmt->param_server_types[i]` lookup in
    /// `build_bind_msg`, which is consulted unconditionally (bounds-checked only, not gated
    /// on statement type) — so this returns `Some` whenever `index` is in range, even though
    /// the data is only meaningful for non-query statements.
    pub fn param_server_type(&self, index: usize) -> Option<ColumnType> {
        self.columns.get(index).map(|c| c.col_type)
    }

    /// Returns `true` if the statement produces result rows and must be executed via a
    /// cursor (`SQ_OPEN`/`SQ_NFETCH`) rather than directly. Matches `libsqli`'s
    /// `sqli_execute` discriminator: statement type `2` (`SELECT`), or type `56`
    /// (`EXECUTE PROCEDURE`/`FUNCTION`) when it also describes output columns. Any other
    /// statement type (including DML with placeholders, whose DESCRIBE response describes
    /// parameters, not columns) is not a query, regardless of how many fields were described.
    pub fn is_query(&self) -> bool {
        self.statement_type == STMT_SELECT
            || (self.statement_type == STMT_SELECT_RETURNING && !self.columns.is_empty())
    }

    /// Returns the Informix statement type code (e.g. 2 for SELECT).
    pub fn statement_type(&self) -> u16 {
        self.statement_type
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(col_type: ColumnType) -> ColumnInfo {
        ColumnInfo::new("x", col_type, 0, 0, 0, 0)
    }

    #[test]
    fn select_with_columns_is_a_query() {
        let stmt = PreparedStatement::new(1, STMT_SELECT, 0, vec![col(ColumnType::Int)]);
        assert!(stmt.is_query());
    }

    #[test]
    fn insert_with_placeholder_describe_fields_is_not_a_query() {
        // INSERT INTO t VALUES (?, ?): the SQ_DESCRIBE response describes the two
        // placeholders, not output columns -- `columns` is non-empty but this must
        // not be mistaken for a query (the old `!self.columns.is_empty()` heuristic did).
        const STMT_INSERT: u16 = 6;
        let stmt = PreparedStatement::new(
            1,
            STMT_INSERT,
            2,
            vec![col(ColumnType::Byte), col(ColumnType::Int)],
        );
        assert!(!stmt.is_query());
        assert_eq!(stmt.param_server_type(0), Some(ColumnType::Byte));
        assert_eq!(stmt.param_server_type(1), Some(ColumnType::Int));
        assert_eq!(stmt.param_server_type(2), None);
    }

    #[test]
    fn execute_procedure_is_a_query_only_with_output_columns() {
        let with_cols =
            PreparedStatement::new(1, STMT_SELECT_RETURNING, 0, vec![col(ColumnType::Int)]);
        assert!(with_cols.is_query());

        let without_cols = PreparedStatement::new(1, STMT_SELECT_RETURNING, 0, vec![]);
        assert!(!without_cols.is_query());
    }

    #[test]
    fn dml_without_placeholders_is_not_a_query() {
        const STMT_DELETE: u16 = 5;
        let stmt = PreparedStatement::new(1, STMT_DELETE, 0, vec![]);
        assert!(!stmt.is_query());
    }
}
