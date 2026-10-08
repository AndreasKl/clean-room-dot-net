//! Writes a [`MetadataModel`] as a PE32 CLI image (§II.24, §II.25).

use crate::bytes::{ByteWriter, align_up};
use crate::cli::{CLI_HEADER_SIZE, COMIMAGE_FLAGS_STRONGNAMESIGNED, CliHeader};
use crate::heaps::{BlobHeapBuilder, GuidHeapBuilder, StringHeapBuilder, UserStringHeapBuilder};
use crate::model::{Cell, MetadataModel, Row};
use crate::pe::DataDirectory;
use crate::tables::{
    ColumnKind, HEAP_WIDE_BLOB, HEAP_WIDE_GUID, HEAP_WIDE_STRINGS, IndexSizes, TABLE_COUNT,
    TableId, columns,
};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Exe,
    Dll,
}

const TEXT_RVA: u32 = 0x2000;
const FILE_ALIGNMENT: u32 = 0x200;
const SECTION_ALIGNMENT: u32 = 0x2000;
const IMAGE_BASE: u32 = 0x0040_0000;
const HEADERS_SIZE: u32 = 0x200;
const RELOC_SIZE: u32 = 12;
const METADATA_SIGNATURE: u32 = 0x424A_5342;

/// Tables flagged as sorted, as emitted by current compilers (§II.22).
const SORTED_TABLES: u64 = 0x0000_1600_3301_FA00;

/// Sorted tables nothing refers to by row: the writer sorts them (stable)
/// by their key column.
const LEAF_SORTED: &[(TableId, usize)] = &[
    (TableId::Constant, 1),
    (TableId::CustomAttribute, 0),
    (TableId::FieldMarshal, 0),
    (TableId::DeclSecurity, 1),
    (TableId::ClassLayout, 2),
    (TableId::FieldLayout, 1),
    (TableId::MethodSemantics, 2),
    (TableId::MethodImpl, 0),
    (TableId::ImplMap, 1),
    (TableId::FieldRva, 1),
    (TableId::NestedClass, 0),
];

/// Sorted tables that other rows refer to: reordering would break those
/// references, so they must arrive sorted.
const REFERENCED_SORTED: &[(TableId, usize)] = &[
    (TableId::InterfaceImpl, 0),
    (TableId::GenericParam, 2),
    (TableId::GenericParamConstraint, 0),
];

/// MS-DOS header and stub exactly as given in §II.25.2.1; e_lfanew = 0x80.
const DOS_HEADER: [u8; 128] = [
    0x4D, 0x5A, 0x90, 0x00, 0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00,
    0xB8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00,
    0x0E, 0x1F, 0xBA, 0x0E, 0x00, 0xB4, 0x09, 0xCD, 0x21, 0xB8, 0x01, 0x4C, 0xCD, 0x21, 0x54, 0x68,
    0x69, 0x73, 0x20, 0x70, 0x72, 0x6F, 0x67, 0x72, 0x61, 0x6D, 0x20, 0x63, 0x61, 0x6E, 0x6E, 0x6F,
    0x74, 0x20, 0x62, 0x65, 0x20, 0x72, 0x75, 0x6E, 0x20, 0x69, 0x6E, 0x20, 0x44, 0x4F, 0x53, 0x20,
    0x6D, 0x6F, 0x64, 0x65, 0x2E, 0x0D, 0x0D, 0x0A, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

fn align32(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

pub fn write_assembly(model: &MetadataModel, kind: ImageKind) -> Result<Vec<u8>> {
    validate(model, kind)?;
    let tables = sorted_tables(model)?;

    let mut text = ByteWriter::new();
    text.zeros(8); // import address table, patched below
    text.zeros(CLI_HEADER_SIZE); // CLI header, patched below
    let body_rvas = write_method_bodies(&tables[TableId::MethodDef.index()], &mut text);

    text.align(4);
    let metadata_rva = TEXT_RVA + text.len() as u32;
    let metadata = build_metadata(model, &tables, &body_rvas)?;
    text.bytes(&metadata);

    let imports = write_imports(&mut text, kind);
    text.patch_u32(0, imports.hint_name_rva);

    let cli = CliHeader {
        runtime_major: 2,
        runtime_minor: 5,
        metadata: DataDirectory {
            rva: metadata_rva,
            size: metadata.len() as u32,
        },
        flags: model.cli_flags & !COMIMAGE_FLAGS_STRONGNAMESIGNED,
        entry_point_token: model.entry_point_token,
        resources: DataDirectory::default(),
        strong_name: DataDirectory::default(),
    };
    let mut header = ByteWriter::new();
    cli.write(&mut header);
    text.buf[8..8 + CLI_HEADER_SIZE].copy_from_slice(&header.buf);

    Ok(write_pe(&text.buf, &imports, kind))
}

fn validate(model: &MetadataModel, kind: ImageKind) -> Result<()> {
    if model.tables.len() != TABLE_COUNT {
        return Err(Error::Malformed(format!(
            "model has {} tables, expected {TABLE_COUNT}",
            model.tables.len()
        )));
    }
    for t in [TableId::FieldRva, TableId::ManifestResource] {
        if !model.table(t).is_empty() {
            return Err(Error::Unsupported(format!("writing {t:?} rows")));
        }
    }
    if kind == ImageKind::Exe && model.entry_point_token == 0 {
        return Err(Error::Malformed(
            "an executable needs an entry point token".into(),
        ));
    }
    for t in TableId::ALL {
        let expected = columns(t).len();
        for (i, row) in model.table(t).iter().enumerate() {
            if row.len() != expected {
                return Err(Error::Malformed(format!(
                    "{t:?} row {} has {} cells, expected {expected}",
                    i + 1,
                    row.len()
                )));
            }
        }
    }
    Ok(())
}

fn sort_key(table: TableId, row: &Row, col: usize) -> Result<u32> {
    match (columns(table)[col], &row[col]) {
        (ColumnKind::Table(_), Cell::Ref(_, r)) => Ok(*r),
        (ColumnKind::Coded(c), Cell::Ref(t, r)) => c.encode(*t, *r),
        _ => Err(Error::Malformed(format!(
            "{table:?} row has no index in key column {col}"
        ))),
    }
}

fn sorted_tables(model: &MetadataModel) -> Result<Vec<Vec<Row>>> {
    let mut tables = model.tables.clone();
    for &(table, col) in LEAF_SORTED {
        let rows = std::mem::take(&mut tables[table.index()]);
        let mut keyed = rows
            .into_iter()
            .map(|row| Ok((sort_key(table, &row, col)?, row)))
            .collect::<Result<Vec<_>>>()?;
        keyed.sort_by_key(|(key, _)| *key);
        tables[table.index()] = keyed.into_iter().map(|(_, row)| row).collect();
    }
    for &(table, col) in REFERENCED_SORTED {
        let keys = tables[table.index()]
            .iter()
            .map(|row| sort_key(table, row, col))
            .collect::<Result<Vec<_>>>()?;
        if keys.windows(2).any(|w| w[0] > w[1]) {
            return Err(Error::Unsupported(format!(
                "{table:?} rows must already be sorted by column {col}"
            )));
        }
    }
    Ok(tables)
}

fn write_method_bodies(methods: &[Row], text: &mut ByteWriter) -> Vec<u32> {
    let mut rvas = vec![0; methods.len()];
    for (i, row) in methods.iter().enumerate() {
        if let Some(Cell::Body(Some(body))) = row.first() {
            if !body.is_tiny() {
                text.align(4);
            }
            rvas[i] = TEXT_RVA + text.len() as u32;
            text.bytes(&body.encode());
        }
    }
    rvas
}

struct HeapBuilders {
    strings: StringHeapBuilder,
    blobs: BlobHeapBuilder,
    guids: GuidHeapBuilder,
}

fn raw_value(
    kind: ColumnKind,
    cell: &Cell,
    heaps: &mut HeapBuilders,
    counts: &[u32; TABLE_COUNT],
    body_rva: u32,
) -> Result<u32> {
    Ok(match (kind, cell) {
        (ColumnKind::U16, Cell::U16(v)) => u32::from(*v),
        (ColumnKind::U32, Cell::U32(v)) => *v,
        (ColumnKind::Str, Cell::Str(s)) => heaps.strings.add(s)?,
        (ColumnKind::Blob, Cell::Blob(b)) => heaps.blobs.add(b)?,
        (ColumnKind::Guid, Cell::Guid(g)) => heaps.guids.add(*g),
        (ColumnKind::Table(target), Cell::Ref(t, row)) if *t == target => {
            // List columns may point one past the last row.
            if *row > counts[target.index()] + 1 {
                return Err(Error::Malformed(format!(
                    "{target:?} row {row} is out of range"
                )));
            }
            *row
        }
        (ColumnKind::Coded(c), Cell::Ref(t, row)) => {
            if *row > counts[t.index()] {
                return Err(Error::Malformed(format!("{t:?} row {row} is out of range")));
            }
            c.encode(*t, *row)?
        }
        (ColumnKind::MethodRva, Cell::Body(_)) => body_rva,
        (kind, cell) => {
            return Err(Error::Malformed(format!(
                "expected a {kind:?} cell, found {}",
                cell.kind_name()
            )));
        }
    })
}

fn build_metadata(
    model: &MetadataModel,
    tables: &[Vec<Row>],
    body_rvas: &[u32],
) -> Result<Vec<u8>> {
    let mut counts = [0u32; TABLE_COUNT];
    for t in TableId::ALL {
        counts[t.index()] = tables[t.index()].len() as u32;
    }

    let mut user_strings = UserStringHeapBuilder::new();
    for s in &model.user_strings {
        user_strings.add(s)?;
    }

    // Pass 1: intern heap values and turn every cell into its raw value.
    let mut heaps = HeapBuilders {
        strings: StringHeapBuilder::new(),
        blobs: BlobHeapBuilder::new(),
        guids: GuidHeapBuilder::new(),
    };
    let mut raw: Vec<Vec<u32>> = Vec::with_capacity(TABLE_COUNT);
    for t in TableId::ALL {
        let kinds = columns(t);
        let mut values = Vec::with_capacity(tables[t.index()].len() * kinds.len());
        for (i, row) in tables[t.index()].iter().enumerate() {
            let body_rva = if t == TableId::MethodDef {
                body_rvas[i]
            } else {
                0
            };
            for (&kind, cell) in kinds.iter().zip(row) {
                let value =
                    raw_value(kind, cell, &mut heaps, &counts, body_rva).map_err(|e| match e {
                        Error::Malformed(m) => {
                            Error::Malformed(format!("{t:?} row {}: {m}", i + 1))
                        }
                        other => other,
                    })?;
                values.push(value);
            }
        }
        raw.push(values);
    }

    let strings = heaps.strings.finish();
    let blobs = heaps.blobs.finish();
    let guids = heaps.guids.finish();
    let user_strings = user_strings.finish();
    let mut heap_sizes = 0;
    if strings.len() >= 0x1_0000 {
        heap_sizes |= HEAP_WIDE_STRINGS;
    }
    if guids.len() >= 0x1_0000 {
        heap_sizes |= HEAP_WIDE_GUID;
    }
    if blobs.len() >= 0x1_0000 {
        heap_sizes |= HEAP_WIDE_BLOB;
    }

    // Pass 2: the #~ stream (§II.24.2.6).
    let sizes = IndexSizes::new(counts, heap_sizes);
    let mut w = ByteWriter::new();
    w.u32(0);
    w.u8(2);
    w.u8(0);
    w.u8(heap_sizes);
    w.u8(1);
    let valid = TableId::ALL
        .iter()
        .filter(|t| counts[t.index()] > 0)
        .fold(0u64, |mask, t| mask | (1u64 << t.index()));
    w.u64(valid);
    w.u64(SORTED_TABLES);
    for t in TableId::ALL {
        if counts[t.index()] > 0 {
            w.u32(counts[t.index()]);
        }
    }
    for t in TableId::ALL {
        let kinds = columns(t);
        for (j, &value) in raw[t.index()].iter().enumerate() {
            let size = sizes.column_size(kinds[j % kinds.len()]);
            if size == 2 && value > 0xFFFF {
                return Err(Error::Malformed(format!(
                    "{t:?}: value {value:#x} does not fit a 2-byte column"
                )));
            }
            w.uint(value, size);
        }
    }
    w.align(4);

    metadata_root(
        &model.version,
        &[
            ("#~", &w.buf),
            ("#Strings", &strings),
            ("#US", &user_strings),
            ("#GUID", &guids),
            ("#Blob", &blobs),
        ],
    )
}

/// §II.24.2.1 metadata root followed by §II.24.2.2 stream headers and data.
fn metadata_root(version: &str, streams: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let mut version_bytes = version.as_bytes().to_vec();
    version_bytes.push(0);
    let version_len = align_up(version_bytes.len(), 4);
    if version_len > 255 {
        return Err(Error::Malformed(
            "metadata version string is too long".into(),
        ));
    }
    version_bytes.resize(version_len, 0);

    let header_len = 16
        + version_len
        + 4
        + streams
            .iter()
            .map(|(name, _)| 8 + align_up(name.len() + 1, 4))
            .sum::<usize>();
    let mut w = ByteWriter::new();
    w.u32(METADATA_SIGNATURE);
    w.u16(1);
    w.u16(1);
    w.u32(0);
    w.u32(version_len as u32);
    w.bytes(&version_bytes);
    w.u16(0);
    w.u16(streams.len() as u16);
    let mut offset = header_len;
    for (name, data) in streams {
        w.u32(offset as u32);
        w.u32(data.len() as u32);
        w.bytes(name.as_bytes());
        w.u8(0);
        w.align(4);
        offset += data.len();
    }
    debug_assert_eq!(w.len(), header_len);
    for (_, data) in streams {
        w.bytes(data);
    }
    Ok(w.into_vec())
}

struct Imports {
    import_dir_rva: u32,
    import_dir_size: u32,
    hint_name_rva: u32,
    entry_stub_rva: u32,
}

/// Import of mscoree.dll!_CorExeMain/_CorDllMain plus the x86 entry stub
/// (§II.25.3.1). CLI hosts on Linux ignore both; they keep the image valid PE.
fn write_imports(text: &mut ByteWriter, kind: ImageKind) -> Imports {
    let rva = |t: &ByteWriter| TEXT_RVA + t.len() as u32;
    text.align(4);
    let import_dir_rva = rva(text);
    let lookup_rva = import_dir_rva + 40;
    let hint_name_rva = lookup_rva + 8;
    let entry_name: &[u8] = match kind {
        ImageKind::Exe => b"_CorExeMain\0",
        ImageKind::Dll => b"_CorDllMain\0",
    };
    let dll_name_rva = hint_name_rva + 2 + entry_name.len() as u32;

    // Import directory: one entry and a null terminator.
    text.u32(lookup_rva);
    text.u32(0);
    text.u32(0);
    text.u32(dll_name_rva);
    text.u32(TEXT_RVA);
    text.zeros(20);
    // Import lookup table.
    text.u32(hint_name_rva);
    text.u32(0);
    // Hint/name entry, then the DLL name.
    text.u16(0);
    text.bytes(entry_name);
    text.bytes(b"mscoree.dll\0");
    let import_dir_size = rva(text) - import_dir_rva;

    // `jmp dword ptr [IAT]`, with its 4-byte operand aligned.
    text.align(4);
    text.zeros(2);
    let entry_stub_rva = rva(text);
    text.bytes(&[0xFF, 0x25]);
    text.u32(IMAGE_BASE + TEXT_RVA);

    Imports {
        import_dir_rva,
        import_dir_size,
        hint_name_rva,
        entry_stub_rva,
    }
}

fn section_header(
    w: &mut ByteWriter,
    name: &[u8; 8],
    virtual_size: u32,
    rva: u32,
    raw_size: u32,
    raw_pointer: u32,
    characteristics: u32,
) {
    w.bytes(name);
    w.u32(virtual_size);
    w.u32(rva);
    w.u32(raw_size);
    w.u32(raw_pointer);
    w.u32(0); // relocations
    w.u32(0); // line numbers
    w.u16(0);
    w.u16(0);
    w.u32(characteristics);
}

/// PE file header, PE32 optional header and section headers (§II.25.2).
fn write_pe(text: &[u8], imports: &Imports, kind: ImageKind) -> Vec<u8> {
    let text_size = text.len() as u32;
    let text_raw_size = align32(text_size, FILE_ALIGNMENT);
    let reloc_rva = align32(TEXT_RVA + text_size, SECTION_ALIGNMENT);
    let image_size = align32(reloc_rva + RELOC_SIZE, SECTION_ALIGNMENT);

    let mut w = ByteWriter::new();
    w.bytes(&DOS_HEADER);
    w.bytes(b"PE\0\0");

    // COFF file header.
    w.u16(0x014C); // i386
    w.u16(2); // sections
    w.u32(0); // timestamp: 0 keeps output deterministic
    w.u32(0);
    w.u32(0);
    w.u16(0xE0); // optional header size
    w.u16(match kind {
        ImageKind::Exe => 0x0102, // EXECUTABLE_IMAGE | 32BIT_MACHINE
        ImageKind::Dll => 0x2102, // ... | DLL
    });

    // Optional header: standard fields.
    w.u16(0x010B); // PE32
    w.u8(8);
    w.u8(0);
    w.u32(text_raw_size);
    w.u32(FILE_ALIGNMENT); // initialized data: .reloc
    w.u32(0);
    w.u32(imports.entry_stub_rva);
    w.u32(TEXT_RVA);
    w.u32(reloc_rva);
    // Optional header: Windows-specific fields.
    w.u32(IMAGE_BASE);
    w.u32(SECTION_ALIGNMENT);
    w.u32(FILE_ALIGNMENT);
    w.u16(4); // OS major
    w.u16(0);
    w.u16(0); // image version
    w.u16(0);
    w.u16(4); // subsystem major
    w.u16(0);
    w.u32(0); // Win32 version
    w.u32(image_size);
    w.u32(HEADERS_SIZE);
    w.u32(0); // checksum
    w.u16(3); // console subsystem
    w.u16(0x8540); // DYNAMIC_BASE | NX_COMPAT | NO_SEH | TERMINAL_SERVER_AWARE
    w.u32(0x10_0000); // stack reserve
    w.u32(0x1000); // stack commit
    w.u32(0x10_0000); // heap reserve
    w.u32(0x1000); // heap commit
    w.u32(0); // loader flags
    w.u32(16); // data directory count
    let mut directories = [(0u32, 0u32); 16];
    directories[1] = (imports.import_dir_rva, imports.import_dir_size);
    directories[5] = (reloc_rva, RELOC_SIZE);
    directories[12] = (TEXT_RVA, 8);
    directories[14] = (TEXT_RVA + 8, CLI_HEADER_SIZE as u32);
    for (rva, size) in directories {
        w.u32(rva);
        w.u32(size);
    }

    section_header(
        &mut w,
        b".text\0\0\0",
        text_size,
        TEXT_RVA,
        text_raw_size,
        HEADERS_SIZE,
        0x6000_0020,
    );
    section_header(
        &mut w,
        b".reloc\0\0",
        RELOC_SIZE,
        reloc_rva,
        FILE_ALIGNMENT,
        HEADERS_SIZE + text_raw_size,
        0x4200_0040,
    );
    w.pad_to(HEADERS_SIZE as usize);

    w.bytes(text);
    w.pad_to((HEADERS_SIZE + text_raw_size) as usize);

    // Base relocation block: one HIGHLOW fixup for the entry stub operand.
    let fixup_rva = imports.entry_stub_rva + 2;
    w.u32(fixup_rva & !0xFFF);
    w.u32(RELOC_SIZE);
    w.u16((3 << 12) | (fixup_rva & 0xFFF) as u16);
    w.u16(0);
    w.pad_to((HEADERS_SIZE + text_raw_size + FILE_ALIGNMENT) as usize);
    w.into_vec()
}
