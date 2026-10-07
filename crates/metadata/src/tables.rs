//! Metadata table schema (§II.22) and index widths (§II.24.2.6).

use crate::{Error, Result};

pub const TABLE_COUNT: usize = 45;

pub const HEAP_WIDE_STRINGS: u8 = 0x01;
pub const HEAP_WIDE_GUID: u8 = 0x02;
pub const HEAP_WIDE_BLOB: u8 = 0x04;

/// Metadata tables in token order (§II.22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum TableId {
    Module = 0x00,
    TypeRef,
    TypeDef,
    FieldPtr,
    Field,
    MethodPtr,
    MethodDef,
    ParamPtr,
    Param,
    InterfaceImpl,
    MemberRef,
    Constant,
    CustomAttribute,
    FieldMarshal,
    DeclSecurity,
    ClassLayout,
    FieldLayout,
    StandAloneSig,
    EventMap,
    EventPtr,
    Event,
    PropertyMap,
    PropertyPtr,
    Property,
    MethodSemantics,
    MethodImpl,
    ModuleRef,
    TypeSpec,
    ImplMap,
    FieldRva,
    EncLog,
    EncMap,
    Assembly,
    AssemblyProcessor,
    AssemblyOs,
    AssemblyRef,
    AssemblyRefProcessor,
    AssemblyRefOs,
    File,
    ExportedType,
    ManifestResource,
    NestedClass,
    GenericParam,
    MethodSpec,
    GenericParamConstraint,
}

impl TableId {
    pub const ALL: [TableId; TABLE_COUNT] = {
        use TableId::*;
        [
            Module,
            TypeRef,
            TypeDef,
            FieldPtr,
            Field,
            MethodPtr,
            MethodDef,
            ParamPtr,
            Param,
            InterfaceImpl,
            MemberRef,
            Constant,
            CustomAttribute,
            FieldMarshal,
            DeclSecurity,
            ClassLayout,
            FieldLayout,
            StandAloneSig,
            EventMap,
            EventPtr,
            Event,
            PropertyMap,
            PropertyPtr,
            Property,
            MethodSemantics,
            MethodImpl,
            ModuleRef,
            TypeSpec,
            ImplMap,
            FieldRva,
            EncLog,
            EncMap,
            Assembly,
            AssemblyProcessor,
            AssemblyOs,
            AssemblyRef,
            AssemblyRefProcessor,
            AssemblyRefOs,
            File,
            ExportedType,
            ManifestResource,
            NestedClass,
            GenericParam,
            MethodSpec,
            GenericParamConstraint,
        ]
    };

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<TableId> {
        Self::ALL.get(i).copied()
    }

    /// Metadata token for a 1-based row of this table.
    pub fn token(self, row: u32) -> u32 {
        ((self as u32) << 24) | row
    }
}

/// Coded index kinds (§II.24.2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodedIndex {
    TypeDefOrRef,
    HasConstant,
    HasCustomAttribute,
    HasFieldMarshal,
    HasDeclSecurity,
    MemberRefParent,
    HasSemantics,
    MethodDefOrRef,
    MemberForwarded,
    Implementation,
    CustomAttributeType,
    ResolutionScope,
    TypeOrMethodDef,
}

impl CodedIndex {
    /// Tables in tag order; `None` marks tags that are reserved.
    pub fn tables(self) -> &'static [Option<TableId>] {
        use TableId as T;
        match self {
            CodedIndex::TypeDefOrRef => &[Some(T::TypeDef), Some(T::TypeRef), Some(T::TypeSpec)],
            CodedIndex::HasConstant => &[Some(T::Field), Some(T::Param), Some(T::Property)],
            CodedIndex::HasCustomAttribute => &[
                Some(T::MethodDef),
                Some(T::Field),
                Some(T::TypeRef),
                Some(T::TypeDef),
                Some(T::Param),
                Some(T::InterfaceImpl),
                Some(T::MemberRef),
                Some(T::Module),
                Some(T::DeclSecurity),
                Some(T::Property),
                Some(T::Event),
                Some(T::StandAloneSig),
                Some(T::ModuleRef),
                Some(T::TypeSpec),
                Some(T::Assembly),
                Some(T::AssemblyRef),
                Some(T::File),
                Some(T::ExportedType),
                Some(T::ManifestResource),
                Some(T::GenericParam),
                Some(T::GenericParamConstraint),
                Some(T::MethodSpec),
            ],
            CodedIndex::HasFieldMarshal => &[Some(T::Field), Some(T::Param)],
            CodedIndex::HasDeclSecurity => {
                &[Some(T::TypeDef), Some(T::MethodDef), Some(T::Assembly)]
            }
            CodedIndex::MemberRefParent => &[
                Some(T::TypeDef),
                Some(T::TypeRef),
                Some(T::ModuleRef),
                Some(T::MethodDef),
                Some(T::TypeSpec),
            ],
            CodedIndex::HasSemantics => &[Some(T::Event), Some(T::Property)],
            CodedIndex::MethodDefOrRef => &[Some(T::MethodDef), Some(T::MemberRef)],
            CodedIndex::MemberForwarded => &[Some(T::Field), Some(T::MethodDef)],
            CodedIndex::Implementation => {
                &[Some(T::File), Some(T::AssemblyRef), Some(T::ExportedType)]
            }
            CodedIndex::CustomAttributeType => {
                &[None, None, Some(T::MethodDef), Some(T::MemberRef), None]
            }
            CodedIndex::ResolutionScope => &[
                Some(T::Module),
                Some(T::ModuleRef),
                Some(T::AssemblyRef),
                Some(T::TypeRef),
            ],
            CodedIndex::TypeOrMethodDef => &[Some(T::TypeDef), Some(T::MethodDef)],
        }
    }

    pub fn tag_bits(self) -> u32 {
        match self {
            CodedIndex::HasFieldMarshal
            | CodedIndex::HasSemantics
            | CodedIndex::MethodDefOrRef
            | CodedIndex::MemberForwarded
            | CodedIndex::TypeOrMethodDef => 1,
            CodedIndex::TypeDefOrRef
            | CodedIndex::HasConstant
            | CodedIndex::HasDeclSecurity
            | CodedIndex::Implementation
            | CodedIndex::ResolutionScope => 2,
            CodedIndex::MemberRefParent | CodedIndex::CustomAttributeType => 3,
            CodedIndex::HasCustomAttribute => 5,
        }
    }

    pub fn decode(self, raw: u32) -> Result<(TableId, u32)> {
        let bits = self.tag_bits();
        let tag = (raw & ((1 << bits) - 1)) as usize;
        let table = self.tables().get(tag).copied().flatten().ok_or_else(|| {
            Error::BadImage(format!(
                "invalid tag {tag} in {self:?} coded index {raw:#x}"
            ))
        })?;
        Ok((table, raw >> bits))
    }

    pub fn encode(self, table: TableId, row: u32) -> Result<u32> {
        let tag = self
            .tables()
            .iter()
            .position(|t| *t == Some(table))
            .ok_or_else(|| {
                Error::Malformed(format!("{table:?} cannot appear in a {self:?} coded index"))
            })?;
        Ok((row << self.tag_bits()) | tag as u32)
    }
}

/// Physical kind of one table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    U16,
    U32,
    Str,
    Guid,
    Blob,
    Table(TableId),
    Coded(CodedIndex),
    /// MethodDef.RVA: the reader/writer replace it with a decoded method body.
    MethodRva,
}

/// Column layout of every table (§II.22.2–§II.22.39). `Constant.Type` is a
/// byte followed by a padding byte and is modelled as `U16`.
pub fn columns(table: TableId) -> &'static [ColumnKind] {
    use CodedIndex as C;
    use ColumnKind::*;
    use TableId as T;
    match table {
        T::Module => &[U16, Str, Guid, Guid, Guid],
        T::TypeRef => &[Coded(C::ResolutionScope), Str, Str],
        T::TypeDef => &[
            U32,
            Str,
            Str,
            Coded(C::TypeDefOrRef),
            Table(T::Field),
            Table(T::MethodDef),
        ],
        T::FieldPtr => &[Table(T::Field)],
        T::Field => &[U16, Str, Blob],
        T::MethodPtr => &[Table(T::MethodDef)],
        T::MethodDef => &[MethodRva, U16, U16, Str, Blob, Table(T::Param)],
        T::ParamPtr => &[Table(T::Param)],
        T::Param => &[U16, U16, Str],
        T::InterfaceImpl => &[Table(T::TypeDef), Coded(C::TypeDefOrRef)],
        T::MemberRef => &[Coded(C::MemberRefParent), Str, Blob],
        T::Constant => &[U16, Coded(C::HasConstant), Blob],
        T::CustomAttribute => &[
            Coded(C::HasCustomAttribute),
            Coded(C::CustomAttributeType),
            Blob,
        ],
        T::FieldMarshal => &[Coded(C::HasFieldMarshal), Blob],
        T::DeclSecurity => &[U16, Coded(C::HasDeclSecurity), Blob],
        T::ClassLayout => &[U16, U32, Table(T::TypeDef)],
        T::FieldLayout => &[U32, Table(T::Field)],
        T::StandAloneSig => &[Blob],
        T::EventMap => &[Table(T::TypeDef), Table(T::Event)],
        T::EventPtr => &[Table(T::Event)],
        T::Event => &[U16, Str, Coded(C::TypeDefOrRef)],
        T::PropertyMap => &[Table(T::TypeDef), Table(T::Property)],
        T::PropertyPtr => &[Table(T::Property)],
        T::Property => &[U16, Str, Blob],
        T::MethodSemantics => &[U16, Table(T::MethodDef), Coded(C::HasSemantics)],
        T::MethodImpl => &[
            Table(T::TypeDef),
            Coded(C::MethodDefOrRef),
            Coded(C::MethodDefOrRef),
        ],
        T::ModuleRef => &[Str],
        T::TypeSpec => &[Blob],
        T::ImplMap => &[U16, Coded(C::MemberForwarded), Str, Table(T::ModuleRef)],
        T::FieldRva => &[U32, Table(T::Field)],
        T::EncLog => &[U32, U32],
        T::EncMap => &[U32],
        T::Assembly => &[U32, U16, U16, U16, U16, U32, Blob, Str, Str],
        T::AssemblyProcessor => &[U32],
        T::AssemblyOs => &[U32, U32, U32],
        T::AssemblyRef => &[U16, U16, U16, U16, U32, Blob, Str, Str, Blob],
        T::AssemblyRefProcessor => &[U32, Table(T::AssemblyRef)],
        T::AssemblyRefOs => &[U32, U32, U32, Table(T::AssemblyRef)],
        T::File => &[U32, Str, Blob],
        T::ExportedType => &[U32, U32, Str, Str, Coded(C::Implementation)],
        T::ManifestResource => &[U32, U32, Str, Coded(C::Implementation)],
        T::NestedClass => &[Table(T::TypeDef), Table(T::TypeDef)],
        T::GenericParam => &[U16, U16, Coded(C::TypeOrMethodDef), Str],
        T::MethodSpec => &[Coded(C::MethodDefOrRef), Blob],
        T::GenericParamConstraint => &[Table(T::GenericParam), Coded(C::TypeDefOrRef)],
    }
}

/// Byte widths of heap and table indexes for one `#~` stream (§II.24.2.6).
#[derive(Debug, Clone)]
pub struct IndexSizes {
    rows: [u32; TABLE_COUNT],
    heap_sizes: u8,
}

impl IndexSizes {
    pub fn new(rows: [u32; TABLE_COUNT], heap_sizes: u8) -> Self {
        Self { rows, heap_sizes }
    }

    fn heap(&self, flag: u8) -> usize {
        if self.heap_sizes & flag != 0 { 4 } else { 2 }
    }

    pub fn column_size(&self, kind: ColumnKind) -> usize {
        match kind {
            ColumnKind::U16 => 2,
            ColumnKind::U32 | ColumnKind::MethodRva => 4,
            ColumnKind::Str => self.heap(HEAP_WIDE_STRINGS),
            ColumnKind::Guid => self.heap(HEAP_WIDE_GUID),
            ColumnKind::Blob => self.heap(HEAP_WIDE_BLOB),
            ColumnKind::Table(t) => {
                if self.rows[t.index()] < 0x1_0000 {
                    2
                } else {
                    4
                }
            }
            ColumnKind::Coded(c) => {
                let max = c
                    .tables()
                    .iter()
                    .flatten()
                    .map(|t| self.rows[t.index()])
                    .max()
                    .unwrap_or(0);
                if max < (1 << (16 - c.tag_bits())) {
                    2
                } else {
                    4
                }
            }
        }
    }

    pub fn row_size(&self, table: TableId) -> usize {
        columns(table).iter().map(|&k| self.column_size(k)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_with(table: TableId, count: u32) -> [u32; TABLE_COUNT] {
        let mut rows = [0; TABLE_COUNT];
        rows[table.index()] = count;
        rows
    }

    #[test]
    fn table_ids_match_their_index() {
        for (i, t) in TableId::ALL.iter().enumerate() {
            assert_eq!(t.index(), i);
            assert_eq!(TableId::from_index(i), Some(*t));
        }
        assert_eq!(TableId::GenericParamConstraint as u8, 0x2C);
        assert_eq!(TableId::from_index(TABLE_COUNT), None);
        assert_eq!(TableId::MethodDef.token(1), 0x0600_0001);
    }

    #[test]
    fn row_sizes_with_small_heaps_and_tables() {
        let s = IndexSizes::new([0; TABLE_COUNT], 0);
        assert_eq!(s.row_size(TableId::Module), 10);
        assert_eq!(s.row_size(TableId::TypeDef), 14);
        assert_eq!(s.row_size(TableId::MethodDef), 14);
        assert_eq!(s.row_size(TableId::Assembly), 22);
        assert_eq!(s.row_size(TableId::AssemblyRef), 20);
    }

    #[test]
    fn wide_heaps_widen_heap_columns() {
        let s = IndexSizes::new(
            [0; TABLE_COUNT],
            HEAP_WIDE_STRINGS | HEAP_WIDE_GUID | HEAP_WIDE_BLOB,
        );
        assert_eq!(s.row_size(TableId::Module), 2 + 4 + 4 + 4 + 4);
        assert_eq!(s.column_size(ColumnKind::Blob), 4);
        let only_strings = IndexSizes::new([0; TABLE_COUNT], HEAP_WIDE_STRINGS);
        assert_eq!(only_strings.column_size(ColumnKind::Str), 4);
        assert_eq!(only_strings.column_size(ColumnKind::Blob), 2);
    }

    #[test]
    fn simple_index_widens_at_65536_rows() {
        let kind = ColumnKind::Table(TableId::Field);
        assert_eq!(
            IndexSizes::new(rows_with(TableId::Field, 0xFFFF), 0).column_size(kind),
            2
        );
        assert_eq!(
            IndexSizes::new(rows_with(TableId::Field, 0x1_0000), 0).column_size(kind),
            4
        );
    }

    #[test]
    fn coded_index_widens_by_tag_bits() {
        let tdor = ColumnKind::Coded(CodedIndex::TypeDefOrRef); // 2 tag bits
        assert_eq!(
            IndexSizes::new(rows_with(TableId::TypeRef, 16_383), 0).column_size(tdor),
            2
        );
        assert_eq!(
            IndexSizes::new(rows_with(TableId::TypeRef, 16_384), 0).column_size(tdor),
            4
        );
        let hca = ColumnKind::Coded(CodedIndex::HasCustomAttribute); // 5 tag bits
        assert_eq!(
            IndexSizes::new(rows_with(TableId::Param, 2_047), 0).column_size(hca),
            2
        );
        assert_eq!(
            IndexSizes::new(rows_with(TableId::Param, 2_048), 0).column_size(hca),
            4
        );
        let cat = ColumnKind::Coded(CodedIndex::CustomAttributeType); // 3 tag bits
        assert_eq!(
            IndexSizes::new(rows_with(TableId::MemberRef, 8_191), 0).column_size(cat),
            2
        );
        assert_eq!(
            IndexSizes::new(rows_with(TableId::MemberRef, 8_192), 0).column_size(cat),
            4
        );
    }

    #[test]
    fn coded_index_encode_decode() {
        assert_eq!(
            CodedIndex::TypeDefOrRef
                .encode(TableId::TypeRef, 5)
                .unwrap(),
            21
        );
        assert_eq!(
            CodedIndex::TypeDefOrRef.decode(21).unwrap(),
            (TableId::TypeRef, 5)
        );
        assert_eq!(
            CodedIndex::HasCustomAttribute
                .encode(TableId::MethodSpec, 1)
                .unwrap(),
            53
        );
        assert_eq!(
            CodedIndex::HasCustomAttribute.decode(53).unwrap(),
            (TableId::MethodSpec, 1)
        );
        assert_eq!(
            CodedIndex::CustomAttributeType
                .decode((7 << 3) | 3)
                .unwrap(),
            (TableId::MemberRef, 7)
        );
    }

    #[test]
    fn invalid_coded_index_is_an_error() {
        assert!(CodedIndex::CustomAttributeType.decode(0).is_err()); // tag 0 is unused
        assert!(CodedIndex::TypeDefOrRef.decode(3).is_err()); // tag 3 does not exist
        assert!(
            CodedIndex::TypeDefOrRef
                .encode(TableId::MethodDef, 1)
                .is_err()
        );
    }

    #[test]
    fn every_table_has_a_schema() {
        for t in TableId::ALL {
            assert!(!columns(t).is_empty(), "{t:?}");
        }
    }
}
