//! Column metadata and descriptors.

use crate::types::column_type::ColumnType;

/// Metadata describing an individual result set column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    /// Name or alias of the column.
    pub name: String,
    /// Semantic data type.
    pub col_type: ColumnType,
    /// Type qualifier / nullability flags.
    pub flags: u16,
    /// Wire-encoded length or precision specification.
    pub encoded_length: u32,
    /// Byte offset within the unaligned row tuple.
    pub col_start_pos: u32,
    /// 1-based index of the field in the SELECT projection.
    pub field_index: i32,
    /// Extended user-defined or distinct type name (e.g. `VARCHAR`, `BLOB`, `UDT`).
    pub type_name: Option<String>,
}

impl ColumnInfo {
    /// Creates a new `ColumnInfo` descriptor.
    pub fn new(
        name: impl Into<String>,
        col_type: ColumnType,
        flags: u16,
        encoded_length: u32,
        col_start_pos: u32,
        field_index: i32,
    ) -> Self {
        Self {
            name: name.into(),
            col_type,
            flags,
            encoded_length,
            col_start_pos,
            field_index,
            type_name: None,
        }
    }

    /// Attaches an extended type name to the column descriptor.
    pub fn with_type_name(mut self, t_name: impl Into<String>) -> Self {
        self.type_name = Some(t_name.into());
        self
    }

    /// Checks if the column can contain NULL values based on its type flags.
    pub fn is_nullable(&self) -> bool {
        (self.flags & crate::types::column_type::flags::NOT_NULLABLE) == 0
    }
}
