//! Stored-procedure/function call convenience API.
//!
//! `EXECUTE PROCEDURE`/`EXECUTE FUNCTION` need no dedicated wire opcodes in the SQLI
//! protocol: they are driven by the same `SQ_PREPARE`/`SQ_DESCRIBE`/`SQ_BIND`/`SQ_EXECUTE`/
//! `SQ_OPEN`/`SQ_NFETCH` sequence as any other statement, distinguished only by statement
//! type `56` (see [`PreparedStatement::is_query`]). This mirrors `libsqli`'s
//! `sqli_call_prepare`/`sqli_call_execute` (`include/libsqli/sqli.h`, `sqli_prepare.c`),
//! which are themselves thin wrappers: `sqli_call_prepare` just calls `sqli_prepare`, and
//! `sqli_call_execute` just calls `sqli_execute` and, if the call produced output columns,
//! fetches the single result row -- identical to fetching a one-row `SELECT`.
//!
//! [`CallStatement`] adds the one piece of bookkeeping `libsqli` provides on top of that:
//! an `IN`/`OUT`/`INOUT` mode per placeholder, used only to map a parameter's declared
//! position to its position in the output row (Informix never echoes `IN` parameters back,
//! so the output row contains only the `OUT`/`INOUT` values, in declaration order) --
//! mirroring `sqli_call_param_to_out_col`.

use crate::result::{ColumnInfo, Row};
use crate::statement::prepared::PreparedStatement;

/// How a callable statement's placeholder at a given position should be treated.
///
/// Every placeholder must still be bound with a `?`-matching value at execute time (the
/// wire protocol has no distinct "output parameter" encoding); this only controls whether
/// [`CallStatement::out_column_index`] maps the parameter to a position in the result row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    /// Input-only; not echoed back in the output row. The default for every placeholder.
    In,
    /// Output-only.
    Out,
    /// Both an input value and an output value.
    InOut,
}

/// A prepared callable statement (`EXECUTE PROCEDURE`/`EXECUTE FUNCTION`/`CALL`), wrapping
/// a [`PreparedStatement`] with per-placeholder [`ParamMode`] bookkeeping.
///
/// Obtained from [`Connection::call_prepare`](crate::connection::client::Connection::call_prepare)
/// and executed with
/// [`Connection::call_execute`](crate::connection::client::Connection::call_execute).
#[derive(Debug, Clone)]
pub struct CallStatement {
    stmt: PreparedStatement,
    modes: Vec<ParamMode>,
}

impl CallStatement {
    pub(crate) fn new(stmt: PreparedStatement) -> Self {
        let modes = vec![ParamMode::In; stmt.parameter_count()];
        Self { stmt, modes }
    }

    /// Returns the underlying [`PreparedStatement`].
    pub fn stmt(&self) -> &PreparedStatement {
        &self.stmt
    }

    /// Returns the server statement identifier (`SQ_ID`).
    pub fn id(&self) -> u16 {
        self.stmt.id()
    }

    /// Sets the parameter mode for the placeholder at `index` (0-based). Returns
    /// `Err` if `index` is out of range for this statement's placeholder count.
    pub fn set_param_mode(&mut self, index: usize, mode: ParamMode) -> crate::error::Result<()> {
        match self.modes.get_mut(index) {
            Some(slot) => {
                *slot = mode;
                Ok(())
            }
            None => Err(crate::error::Error::InvalidState(format!(
                "parameter index {index} out of range (statement has {} placeholders)",
                self.modes.len()
            ))),
        }
    }

    /// Returns the parameter mode for the placeholder at `index`, or `None` if out of range.
    pub fn param_mode(&self, index: usize) -> Option<ParamMode> {
        self.modes.get(index).copied()
    }

    /// Maps a 0-based parameter index to its 0-based position in the output row returned by
    /// [`CallResult::row`], or `None` if the parameter is `In` (never echoed back) or out of
    /// range. Mirrors `libsqli`'s `sqli_call_param_to_out_col`: it counts only `Out`/`InOut`
    /// parameters up to and including `index`, in declaration order.
    pub fn out_column_index(&self, index: usize) -> Option<usize> {
        let mode = *self.modes.get(index)?;
        if mode == ParamMode::In {
            return None;
        }
        Some(
            self.modes[..index]
                .iter()
                .filter(|m| **m != ParamMode::In)
                .count(),
        )
    }
}

/// Outcome of [`Connection::call_execute`](crate::connection::client::Connection::call_execute).
#[derive(Debug, Clone)]
pub struct CallResult {
    /// Rows affected, for a call with no output columns.
    pub rows_affected: i64,
    /// The single output row, present only when the call's `DESCRIBE` reported output
    /// columns (statement type 56 with a non-empty column list -- see
    /// [`PreparedStatement::is_query`]).
    pub row: Option<Row>,
    /// Output column descriptors; empty when `row` is `None`.
    pub columns: Vec<ColumnInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::column_type::ColumnType;

    fn stmt_with_params(n: usize) -> PreparedStatement {
        let cols: Vec<ColumnInfo> = (0..n)
            .map(|_| ColumnInfo::new("p", ColumnType::Int, 0, 0, 0, 0))
            .collect();
        PreparedStatement::new(1, 6, n, cols)
    }

    #[test]
    fn default_modes_are_in_and_map_to_no_output_column() {
        let call = CallStatement::new(stmt_with_params(3));
        for i in 0..3 {
            assert_eq!(call.param_mode(i), Some(ParamMode::In));
            assert_eq!(call.out_column_index(i), None);
        }
        assert_eq!(call.param_mode(3), None);
    }

    #[test]
    fn out_column_index_counts_only_non_in_params_in_declaration_order() {
        let mut call = CallStatement::new(stmt_with_params(4));
        // modes: [In, Out, In, InOut]
        call.set_param_mode(1, ParamMode::Out).unwrap();
        call.set_param_mode(3, ParamMode::InOut).unwrap();

        assert_eq!(call.out_column_index(0), None);
        assert_eq!(call.out_column_index(1), Some(0));
        assert_eq!(call.out_column_index(2), None);
        assert_eq!(call.out_column_index(3), Some(1));
    }

    #[test]
    fn set_param_mode_out_of_range_errs() {
        let mut call = CallStatement::new(stmt_with_params(1));
        assert!(call.set_param_mode(1, ParamMode::Out).is_err());
    }
}
