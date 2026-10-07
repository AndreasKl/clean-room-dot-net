//! The logical metadata model shared by reader and writer. Heap references
//! are resolved to values, so two models compare equal regardless of heap
//! layout. Row order and user-string order are significant: tokens in IL
//! refer to them.

use crate::body::MethodBody;
use crate::cli::COMIMAGE_FLAGS_ILONLY;
use crate::heaps::user_string_entry_size;
use crate::tables::{TABLE_COUNT, TableId};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    U16(u16),
    U32(u32),
    Str(String),
    Blob(Vec<u8>),
    Guid(Option<[u8; 16]>),
    /// Simple or coded table index; row 0 is the null reference.
    Ref(TableId, u32),
    /// MethodDef.RVA, decoded. `None` for methods without IL.
    Body(Option<MethodBody>),
}

impl Cell {
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Cell::U16(v) => Some(u32::from(*v)),
            Cell::U32(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Cell::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_blob(&self) -> Option<&[u8]> {
        match self {
            Cell::Blob(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_index(&self) -> Option<(TableId, u32)> {
        match self {
            Cell::Ref(t, row) => Some((*t, *row)),
            _ => None,
        }
    }

    pub fn as_body(&self) -> Option<&MethodBody> {
        match self {
            Cell::Body(Some(b)) => Some(b),
            _ => None,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Cell::U16(_) => "U16",
            Cell::U32(_) => "U32",
            Cell::Str(_) => "Str",
            Cell::Blob(_) => "Blob",
            Cell::Guid(_) => "Guid",
            Cell::Ref(..) => "Ref",
            Cell::Body(_) => "Body",
        }
    }
}

pub type Row = Vec<Cell>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataModel {
    /// Metadata root version string, e.g. "v4.0.30319".
    pub version: String,
    pub cli_flags: u32,
    /// MethodDef token of the entry point, or 0.
    pub entry_point_token: u32,
    /// One row list per table, indexed by `TableId::index()`.
    pub tables: Vec<Vec<Row>>,
    /// #US entries in heap order; entry offsets follow from this order.
    pub user_strings: Vec<Vec<u16>>,
}

impl Default for MetadataModel {
    fn default() -> Self {
        Self {
            version: "v4.0.30319".to_owned(),
            cli_flags: COMIMAGE_FLAGS_ILONLY,
            entry_point_token: 0,
            tables: vec![Vec::new(); TABLE_COUNT],
            user_strings: Vec::new(),
        }
    }
}

impl MetadataModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn table(&self, table: TableId) -> &[Row] {
        &self.tables[table.index()]
    }

    pub fn table_mut(&mut self, table: TableId) -> &mut Vec<Row> {
        &mut self.tables[table.index()]
    }

    /// Appends a row and returns its 1-based row number.
    pub fn push_row(&mut self, table: TableId, row: Row) -> u32 {
        let rows = self.table_mut(table);
        rows.push(row);
        rows.len() as u32
    }

    fn user_string_offsets(&self) -> impl Iterator<Item = (usize, &[u16])> {
        self.user_strings.iter().scan(1usize, |offset, s| {
            let current = *offset;
            *offset += user_string_entry_size(s.len());
            Some((current, s.as_slice()))
        })
    }

    /// Appends a #US entry and returns its `ldstr` token (0x70 << 24 | offset).
    pub fn add_user_string(&mut self, s: &str) -> Result<u32> {
        let offset: usize = 1 + self
            .user_strings
            .iter()
            .map(|u| user_string_entry_size(u.len()))
            .sum::<usize>();
        if offset > 0x00FF_FFFF {
            return Err(Error::Malformed(
                "#US heap exceeds the 16 MiB token range".into(),
            ));
        }
        self.user_strings.push(s.encode_utf16().collect());
        Ok(0x7000_0000 | offset as u32)
    }

    /// Looks up a #US entry by `ldstr` token (or raw offset).
    pub fn user_string(&self, token: u32) -> Option<&[u16]> {
        let wanted = (token & 0x00FF_FFFF) as usize;
        self.user_string_offsets()
            .take_while(|(offset, _)| *offset <= wanted)
            .find(|(offset, _)| *offset == wanted)
            .map(|(_, s)| s)
    }
}
