//! Column metadata and descriptors.

use crate::types::column_type::ColumnType;

/// Metadata describing a result column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub col_type: ColumnType,
    pub flags: u16,
    pub encoded_length: u32,
    pub col_start_pos: u32,
    pub field_index: i32,
    pub type_name: Option<String>,
}

impl ColumnInfo {
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

    pub fn with_type_name(mut self, t_name: impl Into<String>) -> Self {
        self.type_name = Some(t_name.into());
        self
    }

    pub fn is_nullable(&self) -> bool {
        (self.flags & crate::types::column_type::flags::NOT_NULLABLE) == 0
    }
}
