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

    /// Returns the output column descriptors, or `None` if this statement doesn't produce
    /// result rows (see [`is_query`](Self::is_query)).
    ///
    /// Replaces the previous single `columns()` accessor, which returned the same raw
    /// `SQ_DESCRIBE` field list regardless of statement type — on a non-query statement
    /// that list actually holds input *parameter* descriptors (`libsqli`'s dual-purpose
    /// reuse of the DESCRIBE field array), not output columns, so reading it as columns
    /// there was mislabeled data the caller had to know to avoid via a separate
    /// `is_query()` check. Splitting into this and [`param_types`](Self::param_types)
    /// makes that distinction part of the type instead of a caveat in a doc comment.
    pub fn output_columns(&self) -> Option<&[ColumnInfo]> {
        self.is_query().then(|| self.columns.as_slice())
    }

    /// Returns the input parameter type descriptors, or `None` if this statement produces
    /// result rows (see [`is_query`](Self::is_query)) — the counterpart to
    /// [`output_columns`](Self::output_columns). Note this does **not** cover a query
    /// statement's own placeholders (e.g. `SELECT ... WHERE id = ?`): `libsqli`'s
    /// `build_bind_msg` consults `param_server_type` unconditionally for those, which is
    /// why that lookup stays separate and unguarded by statement type.
    pub fn param_types(&self) -> Option<&[ColumnInfo]> {
        (!self.is_query()).then(|| self.columns.as_slice())
    }

    /// Crate-internal raw access to the `SQ_DESCRIBE` field list, regardless of statement
    /// type — used by the query/fetch paths, which already know (or have just checked)
    /// that the statement is a query and need the output column schema to decode rows.
    pub(crate) fn columns_unchecked(&self) -> &[ColumnInfo] {
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
    fn output_columns_and_param_types_are_mutually_exclusive() {
        let query = PreparedStatement::new(1, STMT_SELECT, 0, vec![col(ColumnType::Int)]);
        assert_eq!(query.output_columns(), Some(&[col(ColumnType::Int)][..]));
        assert_eq!(query.param_types(), None);

        const STMT_INSERT: u16 = 6;
        let dml = PreparedStatement::new(1, STMT_INSERT, 1, vec![col(ColumnType::Byte)]);
        assert_eq!(dml.output_columns(), None);
        assert_eq!(dml.param_types(), Some(&[col(ColumnType::Byte)][..]));
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
