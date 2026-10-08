//! Reads a PE/CLI image into a [`MetadataModel`] (§II.24).

use crate::body::MethodBody;
use crate::bytes::{ByteReader, align_up};
use crate::cli::{CLI_HEADER_SIZE, CliHeader};
use crate::heaps::{BlobHeap, GuidHeap, StringHeap, read_user_strings};
use crate::model::{Cell, MetadataModel, Row};
use crate::pe::PeImage;
use crate::tables::{ColumnKind, IndexSizes, TABLE_COUNT, TableId, columns, ref_in_range};
use crate::{Error, Result};

const METADATA_SIGNATURE: u32 = 0x424A_5342; // "BSJB"
const HEAP_EXTRA_DATA: u8 = 0x40;

pub fn read_assembly(data: &[u8]) -> Result<MetadataModel> {
    let pe = PeImage::parse(data)?;
    let cli = CliHeader::parse(pe.slice(pe.cli_header.rva, CLI_HEADER_SIZE as u32)?)?;
    let root = MetadataRoot::parse(pe.slice(cli.metadata.rva, cli.metadata.size)?)?;

    let tables_stream = match root.stream("#~") {
        Some(s) => s,
        None if root.stream("#-").is_some() => {
            return Err(Error::Unsupported(
                "uncompressed (#-) metadata tables".into(),
            ));
        }
        None => return Err(Error::BadImage("metadata has no #~ stream".into())),
    };
    let heaps = Heaps {
        strings: StringHeap::new(root.stream("#Strings").unwrap_or(&[])),
        blobs: BlobHeap::new(root.stream("#Blob").unwrap_or(&[])),
        guids: GuidHeap::new(root.stream("#GUID").unwrap_or(&[])),
    };
    let user_strings = read_user_strings(root.stream("#US").unwrap_or(&[]))?;
    let tables = read_tables(tables_stream, &heaps, |rva| {
        let body = pe.slice_from(rva).and_then(MethodBody::parse);
        body.map_err(|e| Error::BadImage(format!("method body at RVA {rva:#x}: {e}")))
    })?;
    validate_refs(&tables)?;
    validate_entry_point(cli.entry_point_token, &tables)?;

    Ok(MetadataModel {
        version: root.version,
        cli_flags: cli.flags,
        entry_point_token: cli.entry_point_token,
        tables,
        user_strings,
    })
}

/// Rejects any cross-table reference that points outside its target table, so
/// consumers can index `table(t)[row - 1]` without panicking. Uses the same
/// rule as the writer (`ref_in_range`).
fn validate_refs(tables: &[Vec<Row>]) -> Result<()> {
    let mut counts = [0u32; TABLE_COUNT];
    for t in TableId::ALL {
        counts[t.index()] = tables[t.index()].len() as u32;
    }
    for table in TableId::ALL {
        for (i, row) in tables[table.index()].iter().enumerate() {
            for (&kind, cell) in columns(table).iter().zip(row) {
                if let Cell::Ref(t, r) = cell
                    && !ref_in_range(kind, *t, *r, &counts)
                {
                    return Err(Error::BadImage(format!(
                        "{table:?} row {}: reference to {t:?} row {r} is out of range",
                        i + 1
                    )));
                }
            }
        }
    }
    Ok(())
}

/// §II.25.3.3: the entry point is 0 or a MethodDef token (table 0x06).
fn validate_entry_point(token: u32, tables: &[Vec<Row>]) -> Result<()> {
    let row = (token & 0x00FF_FFFF) as usize;
    let in_range = token == 0
        || (token >> 24 == 0x06 && (1..=tables[TableId::MethodDef.index()].len()).contains(&row));
    if in_range {
        Ok(())
    } else {
        Err(Error::BadImage(format!(
            "entry point token {token:#010x} does not name an existing MethodDef"
        )))
    }
}

struct MetadataRoot<'a> {
    version: String,
    streams: Vec<(String, &'a [u8])>,
}

impl<'a> MetadataRoot<'a> {
    /// §II.24.2.1 metadata root and §II.24.2.2 stream headers.
    fn parse(md: &'a [u8]) -> Result<Self> {
        let mut r = ByteReader::new(md);
        if r.u32()? != METADATA_SIGNATURE {
            return Err(Error::BadImage("missing BSJB metadata signature".into()));
        }
        let _major = r.u16()?;
        let _minor = r.u16()?;
        let _reserved = r.u32()?;
        let version_len = r.u32()? as usize;
        let version_bytes = r.bytes(version_len)?;
        let end = version_bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(version_len);
        let version = std::str::from_utf8(&version_bytes[..end])
            .map_err(|_| Error::BadImage("metadata version is not UTF-8".into()))?
            .to_owned();
        let _flags = r.u16()?;
        let stream_count = r.u16()?;
        let mut streams = Vec::with_capacity(stream_count as usize);
        for _ in 0..stream_count {
            let offset = r.u32()? as usize;
            let size = r.u32()? as usize;
            let name_start = r.pos();
            let mut name = Vec::new();
            loop {
                let b = r.u8()?;
                if b == 0 {
                    break;
                }
                if name.len() == 32 {
                    return Err(Error::BadImage("stream name longer than 32 bytes".into()));
                }
                name.push(b);
            }
            r.seek(name_start + align_up(name.len() + 1, 4));
            let data = offset
                .checked_add(size)
                .and_then(|end| md.get(offset..end))
                .ok_or_else(|| {
                    Error::BadImage("metadata stream lies outside the metadata".into())
                })?;
            streams.push((String::from_utf8_lossy(&name).into_owned(), data));
        }
        Ok(MetadataRoot { version, streams })
    }

    fn stream(&self, name: &str) -> Option<&'a [u8]> {
        self.streams
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| *d)
    }
}

struct Heaps<'a> {
    strings: StringHeap<'a>,
    blobs: BlobHeap<'a>,
    guids: GuidHeap<'a>,
}

/// §II.24.2.6 `#~` stream.
fn read_tables(
    data: &[u8],
    heaps: &Heaps,
    body_at: impl Fn(u32) -> Result<MethodBody>,
) -> Result<Vec<Vec<Row>>> {
    let mut r = ByteReader::new(data);
    let _reserved = r.u32()?;
    let _major = r.u8()?;
    let _minor = r.u8()?;
    let heap_sizes = r.u8()?;
    let _reserved = r.u8()?;
    let valid = r.u64()?;
    let _sorted = r.u64()?;
    if heap_sizes & HEAP_EXTRA_DATA != 0 {
        return Err(Error::Unsupported("extra data in #~ stream header".into()));
    }

    let mut rows = [0u32; TABLE_COUNT];
    for bit in 0..64 {
        if valid & (1u64 << bit) == 0 {
            continue;
        }
        let count = r.u32()?;
        if bit >= TABLE_COUNT {
            return Err(Error::Unsupported(format!(
                "unknown metadata table {bit:#04x}"
            )));
        }
        rows[bit] = count;
    }

    let sizes = IndexSizes::new(rows, heap_sizes);
    let mut tables = vec![Vec::new(); TABLE_COUNT];
    for table in TableId::ALL {
        for _ in 0..rows[table.index()] {
            let mut row = Vec::with_capacity(columns(table).len());
            for &kind in columns(table) {
                let raw = r.uint(sizes.column_size(kind))?;
                row.push(cell(kind, raw, heaps, &body_at)?);
            }
            tables[table.index()].push(row);
        }
    }
    Ok(tables)
}

fn cell(
    kind: ColumnKind,
    raw: u32,
    heaps: &Heaps,
    body_at: &impl Fn(u32) -> Result<MethodBody>,
) -> Result<Cell> {
    Ok(match kind {
        ColumnKind::U16 => Cell::U16(raw as u16),
        ColumnKind::U32 => Cell::U32(raw),
        ColumnKind::Str => Cell::Str(heaps.strings.get(raw)?.to_owned()),
        ColumnKind::Blob => Cell::Blob(heaps.blobs.get(raw)?.to_vec()),
        ColumnKind::Guid => Cell::Guid(heaps.guids.get(raw)?),
        ColumnKind::Table(t) => Cell::Ref(t, raw),
        ColumnKind::Coded(c) => {
            let (t, row) = c.decode(raw)?;
            Cell::Ref(t, row)
        }
        ColumnKind::MethodRva => Cell::Body(if raw == 0 { None } else { Some(body_at(raw)?) }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &[u8] = include_bytes!("../tests/fixtures/hello.exe");

    #[test]
    fn out_of_range_references_are_bad_image() {
        let m = read_assembly(HELLO).unwrap();
        let mut tables = m.tables.clone();
        assert!(validate_refs(&tables).is_ok());

        // Coded index past the end of the target table.
        tables[TableId::MemberRef.index()][0][0] = Cell::Ref(TableId::TypeRef, 999);
        assert!(matches!(validate_refs(&tables), Err(Error::BadImage(_))));

        // Simple list column: one past the end is allowed, two is not.
        let mut tables = m.tables.clone();
        let fields = tables[TableId::Field.index()].len() as u32;
        let td = &mut tables[TableId::TypeDef.index()][0];
        td[4] = Cell::Ref(TableId::Field, fields + 1);
        assert!(validate_refs(&tables).is_ok());
        tables[TableId::TypeDef.index()][0][4] = Cell::Ref(TableId::Field, fields + 2);
        assert!(matches!(validate_refs(&tables), Err(Error::BadImage(_))));
    }
}
