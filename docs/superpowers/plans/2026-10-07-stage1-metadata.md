# Stage 1 — `metadata` Crate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust crate that reads ECMA-335 PE/CLI assemblies into a logical model and writes that model back as a valid PE32 image, proven by round-trip tests on `mcs`-compiled assemblies, a `mono` oracle, and a full read of Mono's `mscorlib.dll`.

**Architecture:** One dependency-free crate, `metadata`. A schema table (`tables.rs`) drives both a generic table reader and a generic table writer, so the reader and writer can never disagree about column layout. The logical model (`MetadataModel`) stores heap references as resolved values (`String`, `Vec<u8>`, GUIDs) and method RVAs as decoded `MethodBody`s, so models can be compared for equality independent of byte layout. Row order and user-string order are preserved, so metadata tokens embedded in IL survive a round trip unchanged.

**Tech Stack:** Rust (stable, edition 2024), no external crates. Mono 6.x (`mcs`, `mono`) for fixtures and oracle tests.

**Spec:** `docs/superpowers/specs/2026-10-07-hello-world-toolchain-design.md` (this plan implements stage 1, §7.1; plans for stages 2–4 follow separately)

## Global Constraints

- Language: Rust stable, `edition = "2024"`; the `metadata` crate has **zero external dependencies**.
- Toolchain is installed via rustup; in a non-login shell run `source "$HOME/.cargo/env"` before `cargo`.
- Clean room: allowed references are ECMA-335 6th edition (authoritative), MIT-licensed dotnet/runtime, and Mono sources. Never decompile any library or application. Inspecting bytes of assemblies *we* produced is fine.
- Corlib identity: `mscorlib`, version `4.0.0.0`.
- No panics on malformed input: every read goes through `ByteReader`, which returns `Result`. Writer bugs may `unreachable!`/`debug_assert!`, input problems may not.
- Doc comments cite ECMA-335 sections as `§II.x.y` where a format rule is implemented.
- Oracle tests that need `mono` or Mono's `mscorlib.dll` must be **ignored with a reason** when the tool is missing (never silently pass).
- Every commit message ends with:
  `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`

## Review Focus

1. **Truncated or corrupted input files** — a cut-off or bit-flipped assembly must produce `Err(..)`, never a panic or stack overflow. Pinned by `truncated_inputs_never_panic` / `bit_flipped_inputs_never_panic` (Task 7) and `deeply_nested_signature_is_an_error` (Task 5).
2. **Heaps ≥ 64 KiB** — the writer must switch to 4-byte heap indexes and set `HeapSizes` flags; the reader must honour them; mono must still load the result. Pinned by `wide_heaps_round_trip` / `mono_runs_hello_with_wide_heaps` (Task 8).
3. **Tables large enough to widen coded indexes** (e.g. > 16 383 TypeRefs) — column widths change across many tables at once. Pinned by `wide_table_indexes_round_trip` / `mono_runs_hello_with_wide_indexes` (Task 8).
4. **Unusual user strings** — `""`, non-ASCII, characters that set the trailing-byte flag, and unpaired surrogates must keep their exact offsets so `ldstr` tokens stay valid. Pinned by `user_string_edge_cases_round_trip` (Task 3).
5. **Unsorted input for sorted tables** — a model whose CustomAttribute rows arrive out of order must still produce a valid (sorted) image. Pinned by `writer_sorts_custom_attributes` (Task 8).

---

## File Structure

```
Cargo.toml                                  # workspace
.gitignore
.github/workflows/ci.yml                    # Task 9
scripts/regen-fixtures.sh                   # Task 6
crates/metadata/
├── Cargo.toml
├── build.rs                                # Task 6: detects mono / Mono mscorlib → cfgs
├── src/
│   ├── lib.rs                              # module list + re-exports
│   ├── error.rs                            # Error, Result
│   ├── bytes.rs                            # ByteReader, ByteWriter, compressed ints
│   ├── tables.rs                           # TableId, CodedIndex, ColumnKind, columns(), IndexSizes
│   ├── heaps.rs                            # #Strings/#Blob/#GUID/#US readers + builders
│   ├── body.rs                             # MethodBody (tiny/fat headers, extra sections)
│   ├── sig.rs                              # TypeSig, MethodSig, signature decode/encode
│   ├── pe.rs                               # PeImage (PE/COFF reader), DataDirectory
│   ├── cli.rs                              # CliHeader read/write
│   ├── model.rs                            # Cell, Row, MetadataModel
│   ├── reader.rs                           # read_assembly
│   └── writer.rs                           # write_assembly, ImageKind
└── tests/
    ├── fixtures/
    │   ├── src/hello.cs
    │   ├── src/lib.cs
    │   ├── hello.exe                       # committed mcs output
    │   └── lib.dll                         # committed mcs output
    ├── pe.rs                               # Task 6
    ├── reader.rs                           # Task 7
    ├── writer.rs                           # Task 8
    └── corlib.rs                           # Task 9
```

---

### Task 1: Workspace scaffold, error type, byte cursor

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `crates/metadata/Cargo.toml`, `crates/metadata/src/lib.rs`, `crates/metadata/src/error.rs`, `crates/metadata/src/bytes.rs`

**Interfaces:**
- Produces:
  - `metadata::Error { BadImage(String), Unsupported(String), Malformed(String) }`, `metadata::Result<T>`
  - `metadata::bytes::align_up(value: usize, alignment: usize) -> usize`
  - `metadata::bytes::compressed_len(value: u32) -> usize`
  - `ByteReader<'a>`: `new(&'a [u8])`, `at(&'a [u8], usize)`, `pos()`, `seek(usize)`, `align(usize)`, `remaining()`, `bytes(n) -> Result<&'a [u8]>`, `peek_u8()`, `u8()`, `u16()`, `u32()`, `u64()`, `uint(size: usize) -> Result<u32>`, `compressed_u32()`, `compressed_i32()`
  - `ByteWriter { pub buf: Vec<u8> }`: `new()`, `len()`, `is_empty()`, `into_vec()`, `u8/u16/u32/u64(v)`, `bytes(&[u8])`, `zeros(n)`, `align(n)`, `pad_to(len)`, `uint(v: u32, size: usize)`, `patch_u32(at, v)`, `compressed_u32(v) -> Result<()>`, `compressed_i32(v) -> Result<()>`

- [ ] **Step 1: Create the workspace files**

`Cargo.toml`:
```toml
[workspace]
resolver = "3"
members = ["crates/metadata"]
```

`.gitignore`:
```
/target
```

`crates/metadata/Cargo.toml`:
```toml
[package]
name = "metadata"
version = "0.1.0"
edition = "2024"
description = "ECMA-335 metadata: PE/CLI image reader and writer"

[dependencies]
```

`crates/metadata/src/lib.rs`:
```rust
//! ECMA-335 metadata: PE/COFF images, CLI header, heaps, tables and signatures.
//!
//! Section references such as §II.23.2 point into ECMA-335, 6th edition,
//! which is the authoritative specification for everything in this crate.

pub mod bytes;
pub mod error;

pub use error::{Error, Result};
```

`crates/metadata/src/error.rs`:
```rust
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The input is not a well-formed PE/CLI image.
    BadImage(String),
    /// The input is well-formed but uses a feature not supported yet.
    Unsupported(String),
    /// A logical model handed to the writer is inconsistent.
    Malformed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadImage(m) => write!(f, "bad image: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Malformed(m) => write!(f, "malformed model: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 2: Write the failing tests for `bytes.rs`**

Create `crates/metadata/src/bytes.rs` containing only the test module for now:
```rust
//! Little-endian byte cursors and ECMA-335 compressed integers (§II.23.2).

#[cfg(test)]
mod tests {
    use super::*;

    // Examples from ECMA-335 §II.23.2.
    const UNSIGNED: &[(u32, &[u8])] = &[
        (0x03, &[0x03]),
        (0x7F, &[0x7F]),
        (0x80, &[0x80, 0x80]),
        (0x2E57, &[0xAE, 0x57]),
        (0x3FFF, &[0xBF, 0xFF]),
        (0x4000, &[0xC0, 0x00, 0x40, 0x00]),
        (0x1FFF_FFFF, &[0xDF, 0xFF, 0xFF, 0xFF]),
    ];

    const SIGNED: &[(i32, &[u8])] = &[
        (3, &[0x06]),
        (-3, &[0x7B]),
        (64, &[0x80, 0x80]),
        (-64, &[0x01]),
        (8192, &[0xC0, 0x00, 0x40, 0x00]),
        (-8192, &[0x80, 0x01]),
        (268_435_455, &[0xDF, 0xFF, 0xFF, 0xFE]),
        (-268_435_456, &[0xC0, 0x00, 0x00, 0x01]),
    ];

    #[test]
    fn compressed_unsigned_matches_ecma_examples() {
        for &(value, encoded) in UNSIGNED {
            let mut w = ByteWriter::new();
            w.compressed_u32(value).unwrap();
            assert_eq!(w.buf, encoded, "encoding {value:#x}");
            assert_eq!(ByteReader::new(encoded).compressed_u32().unwrap(), value);
            assert_eq!(compressed_len(value), encoded.len());
        }
    }

    #[test]
    fn compressed_signed_matches_ecma_examples() {
        for &(value, encoded) in SIGNED {
            let mut w = ByteWriter::new();
            w.compressed_i32(value).unwrap();
            assert_eq!(w.buf, encoded, "encoding {value}");
            assert_eq!(ByteReader::new(encoded).compressed_i32().unwrap(), value);
        }
    }

    #[test]
    fn compressed_out_of_range_is_an_error() {
        assert!(ByteWriter::new().compressed_u32(0x2000_0000).is_err());
        assert!(ByteWriter::new().compressed_i32(0x1000_0000).is_err());
        assert!(ByteWriter::new().compressed_i32(-0x1000_0001).is_err());
    }

    #[test]
    fn invalid_compressed_lead_byte_is_an_error() {
        assert!(ByteReader::new(&[0xE0]).compressed_u32().is_err());
    }

    #[test]
    fn reading_past_end_is_an_error_not_a_panic() {
        let mut r = ByteReader::new(&[1, 2, 3]);
        assert!(r.u32().is_err());
        assert_eq!(r.u16().unwrap(), 0x0201);
        assert!(r.u16().is_err());
        assert!(r.peek_u8().is_ok());
        r.seek(100);
        assert!(r.u8().is_err());
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn little_endian_round_trip() {
        let mut w = ByteWriter::new();
        w.u8(0x11);
        w.u16(0x2233);
        w.u32(0x4455_6677);
        w.u64(0x8899_AABB_CCDD_EEFF);
        w.uint(0xABCD, 2);
        w.uint(0x1234_5678, 4);
        let mut r = ByteReader::new(&w.buf);
        assert_eq!(r.u8().unwrap(), 0x11);
        assert_eq!(r.u16().unwrap(), 0x2233);
        assert_eq!(r.u32().unwrap(), 0x4455_6677);
        assert_eq!(r.u64().unwrap(), 0x8899_AABB_CCDD_EEFF);
        assert_eq!(r.uint(2).unwrap(), 0xABCD);
        assert_eq!(r.uint(4).unwrap(), 0x1234_5678);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn align_and_patch() {
        let mut w = ByteWriter::new();
        w.u8(1);
        w.align(4);
        assert_eq!(w.len(), 4);
        w.u32(0);
        w.patch_u32(4, 0xDEAD_BEEF);
        assert_eq!(&w.buf[4..8], &[0xEF, 0xBE, 0xAD, 0xDE]);
        w.pad_to(16);
        assert_eq!(w.len(), 16);
        assert_eq!(align_up(17, 8), 24);
        assert_eq!(align_up(16, 8), 16);
    }
}
```
Add `pub mod bytes;` already exists in `lib.rs`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p metadata bytes`
Expected: compile errors — `ByteWriter`, `ByteReader`, `compressed_len`, `align_up` not found.

- [ ] **Step 4: Implement `bytes.rs`**

Insert above the test module:
```rust
use crate::{Error, Result};

pub fn align_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

/// Encoded size in bytes of an unsigned compressed integer (§II.23.2).
pub fn compressed_len(value: u32) -> usize {
    if value < 0x80 {
        1
    } else if value < 0x4000 {
        2
    } else {
        4
    }
}

/// Bounds-checked little-endian reader. Every read past the end is an
/// `Error::BadImage`, never a panic.
pub struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn align(&mut self, alignment: usize) {
        self.pos = align_up(self.pos, alignment);
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| {
                Error::BadImage(format!(
                    "unexpected end of data: wanted {n} bytes at offset {:#x}",
                    self.pos
                ))
            })?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn peek_u8(&self) -> Result<u8> {
        self.data.get(self.pos).copied().ok_or_else(|| {
            Error::BadImage(format!("unexpected end of data at offset {:#x}", self.pos))
        })
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    /// Reads a 2- or 4-byte unsigned value, as used by table columns.
    pub fn uint(&mut self, size: usize) -> Result<u32> {
        match size {
            2 => Ok(u32::from(self.u16()?)),
            4 => self.u32(),
            _ => unreachable!("table columns are 2 or 4 bytes wide"),
        }
    }

    /// §II.23.2 unsigned compressed integer.
    pub fn compressed_u32(&mut self) -> Result<u32> {
        let b0 = u32::from(self.u8()?);
        if b0 & 0x80 == 0 {
            Ok(b0)
        } else if b0 & 0xC0 == 0x80 {
            let b1 = u32::from(self.u8()?);
            Ok(((b0 & 0x3F) << 8) | b1)
        } else if b0 & 0xE0 == 0xC0 {
            let rest = self.bytes(3)?;
            Ok(((b0 & 0x1F) << 24)
                | (u32::from(rest[0]) << 16)
                | (u32::from(rest[1]) << 8)
                | u32::from(rest[2]))
        } else {
            Err(Error::BadImage(format!(
                "invalid compressed integer lead byte {b0:#04x}"
            )))
        }
    }

    /// §II.23.2 signed compressed integer: the sign bit is rotated into
    /// bit 0 of a 7-, 14- or 29-bit field.
    pub fn compressed_i32(&mut self) -> Result<i32> {
        let start = self.pos;
        let raw = self.compressed_u32()?;
        let bias: i32 = match self.pos - start {
            1 => 0x40,
            2 => 0x2000,
            _ => 0x1000_0000,
        };
        let magnitude = (raw >> 1) as i32;
        Ok(if raw & 1 == 1 { magnitude - bias } else { magnitude })
    }
}

#[derive(Debug, Default)]
pub struct ByteWriter {
    pub buf: Vec<u8>,
}

impl ByteWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn zeros(&mut self, n: usize) {
        self.buf.resize(self.buf.len() + n, 0);
    }

    pub fn align(&mut self, alignment: usize) {
        let len = align_up(self.buf.len(), alignment);
        self.buf.resize(len, 0);
    }

    pub fn pad_to(&mut self, len: usize) {
        if self.buf.len() < len {
            self.buf.resize(len, 0);
        }
    }

    /// Writes a 2- or 4-byte unsigned value, as used by table columns.
    pub fn uint(&mut self, v: u32, size: usize) {
        match size {
            2 => self.u16(v as u16),
            4 => self.u32(v),
            _ => unreachable!("table columns are 2 or 4 bytes wide"),
        }
    }

    pub fn patch_u32(&mut self, at: usize, v: u32) {
        self.buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// §II.23.2 unsigned compressed integer.
    pub fn compressed_u32(&mut self, v: u32) -> Result<()> {
        if v < 0x80 {
            self.u8(v as u8);
        } else if v < 0x4000 {
            self.u8(0x80 | (v >> 8) as u8);
            self.u8(v as u8);
        } else if v < 0x2000_0000 {
            self.u8(0xC0 | (v >> 24) as u8);
            self.u8((v >> 16) as u8);
            self.u8((v >> 8) as u8);
            self.u8(v as u8);
        } else {
            return Err(Error::Malformed(format!(
                "{v:#x} is too large for a compressed integer"
            )));
        }
        Ok(())
    }

    /// §II.23.2 signed compressed integer.
    pub fn compressed_i32(&mut self, v: i32) -> Result<()> {
        let sign = u32::from(v < 0);
        let bits = v as u32;
        if (-0x40..0x40).contains(&v) {
            self.u8((((bits & 0x3F) << 1) | sign) as u8);
        } else if (-0x2000..0x2000).contains(&v) {
            let raw = ((bits & 0x1FFF) << 1) | sign;
            self.u8(0x80 | (raw >> 8) as u8);
            self.u8(raw as u8);
        } else if (-0x1000_0000..0x1000_0000).contains(&v) {
            let raw = ((bits & 0x0FFF_FFFF) << 1) | sign;
            self.u8(0xC0 | (raw >> 24) as u8);
            self.u8((raw >> 16) as u8);
            self.u8((raw >> 8) as u8);
            self.u8(raw as u8);
        } else {
            return Err(Error::Malformed(format!(
                "{v} is out of range for a compressed signed integer"
            )));
        }
        Ok(())
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p metadata bytes`
Expected: 7 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml .gitignore crates/metadata
git commit -m "feat(metadata): workspace scaffold, error type and byte cursors

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Table schema, coded indexes, index sizes

**Files:**
- Create: `crates/metadata/src/tables.rs`
- Modify: `crates/metadata/src/lib.rs` (add `pub mod tables;`)

**Interfaces:**
- Consumes: `Error`, `Result` (Task 1)
- Produces:
  - `enum TableId` (45 variants, `#[repr(u8)]`, `Module = 0x00` … `GenericParamConstraint = 0x2C`), `TableId::ALL: [TableId; TABLE_COUNT]`, `TableId::index(self) -> usize`, `TableId::from_index(usize) -> Option<TableId>`, `TableId::token(self, row: u32) -> u32`
  - `const TABLE_COUNT: usize = 45`
  - `enum CodedIndex` (13 variants), `CodedIndex::tables(self) -> &'static [Option<TableId>]`, `tag_bits(self) -> u32`, `decode(self, raw: u32) -> Result<(TableId, u32)>`, `encode(self, table: TableId, row: u32) -> Result<u32>`
  - `enum ColumnKind { U16, U32, Str, Guid, Blob, Table(TableId), Coded(CodedIndex), MethodRva }`
  - `fn columns(table: TableId) -> &'static [ColumnKind]`
  - `const HEAP_WIDE_STRINGS: u8 = 0x01`, `HEAP_WIDE_GUID: u8 = 0x02`, `HEAP_WIDE_BLOB: u8 = 0x04`
  - `struct IndexSizes`: `new(rows: [u32; TABLE_COUNT], heap_sizes: u8)`, `column_size(&self, ColumnKind) -> usize`, `row_size(&self, TableId) -> usize`

- [ ] **Step 1: Write the failing tests**

Create `crates/metadata/src/tables.rs` with only:
```rust
//! Metadata table schema (§II.22) and index widths (§II.24.2.6).

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
        assert_eq!(IndexSizes::new(rows_with(TableId::Field, 0xFFFF), 0).column_size(kind), 2);
        assert_eq!(IndexSizes::new(rows_with(TableId::Field, 0x1_0000), 0).column_size(kind), 4);
    }

    #[test]
    fn coded_index_widens_by_tag_bits() {
        let tdor = ColumnKind::Coded(CodedIndex::TypeDefOrRef); // 2 tag bits
        assert_eq!(IndexSizes::new(rows_with(TableId::TypeRef, 16_383), 0).column_size(tdor), 2);
        assert_eq!(IndexSizes::new(rows_with(TableId::TypeRef, 16_384), 0).column_size(tdor), 4);
        let hca = ColumnKind::Coded(CodedIndex::HasCustomAttribute); // 5 tag bits
        assert_eq!(IndexSizes::new(rows_with(TableId::Param, 2_047), 0).column_size(hca), 2);
        assert_eq!(IndexSizes::new(rows_with(TableId::Param, 2_048), 0).column_size(hca), 4);
        let cat = ColumnKind::Coded(CodedIndex::CustomAttributeType); // 3 tag bits
        assert_eq!(IndexSizes::new(rows_with(TableId::MemberRef, 8_191), 0).column_size(cat), 2);
        assert_eq!(IndexSizes::new(rows_with(TableId::MemberRef, 8_192), 0).column_size(cat), 4);
    }

    #[test]
    fn coded_index_encode_decode() {
        assert_eq!(CodedIndex::TypeDefOrRef.encode(TableId::TypeRef, 5).unwrap(), 21);
        assert_eq!(CodedIndex::TypeDefOrRef.decode(21).unwrap(), (TableId::TypeRef, 5));
        assert_eq!(CodedIndex::HasCustomAttribute.encode(TableId::MethodSpec, 1).unwrap(), 53);
        assert_eq!(CodedIndex::HasCustomAttribute.decode(53).unwrap(), (TableId::MethodSpec, 1));
        assert_eq!(CodedIndex::CustomAttributeType.decode((7 << 3) | 3).unwrap(), (TableId::MemberRef, 7));
    }

    #[test]
    fn invalid_coded_index_is_an_error() {
        assert!(CodedIndex::CustomAttributeType.decode(0).is_err()); // tag 0 is unused
        assert!(CodedIndex::TypeDefOrRef.decode(3).is_err()); // tag 3 does not exist
        assert!(CodedIndex::TypeDefOrRef.encode(TableId::MethodDef, 1).is_err());
    }

    #[test]
    fn every_table_has_a_schema() {
        for t in TableId::ALL {
            assert!(!columns(t).is_empty(), "{t:?}");
        }
    }
}
```
Add `pub mod tables;` to `lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p metadata tables`
Expected: compile errors — `TableId`, `IndexSizes`, etc. not found.

- [ ] **Step 3: Implement `tables.rs`**

Insert above the test module:
```rust
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
            Module, TypeRef, TypeDef, FieldPtr, Field, MethodPtr, MethodDef, ParamPtr, Param,
            InterfaceImpl, MemberRef, Constant, CustomAttribute, FieldMarshal, DeclSecurity,
            ClassLayout, FieldLayout, StandAloneSig, EventMap, EventPtr, Event, PropertyMap,
            PropertyPtr, Property, MethodSemantics, MethodImpl, ModuleRef, TypeSpec, ImplMap,
            FieldRva, EncLog, EncMap, Assembly, AssemblyProcessor, AssemblyOs, AssemblyRef,
            AssemblyRefProcessor, AssemblyRefOs, File, ExportedType, ManifestResource,
            NestedClass, GenericParam, MethodSpec, GenericParamConstraint,
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
                Some(T::MethodDef), Some(T::Field), Some(T::TypeRef), Some(T::TypeDef),
                Some(T::Param), Some(T::InterfaceImpl), Some(T::MemberRef), Some(T::Module),
                Some(T::DeclSecurity), Some(T::Property), Some(T::Event), Some(T::StandAloneSig),
                Some(T::ModuleRef), Some(T::TypeSpec), Some(T::Assembly), Some(T::AssemblyRef),
                Some(T::File), Some(T::ExportedType), Some(T::ManifestResource),
                Some(T::GenericParam), Some(T::GenericParamConstraint), Some(T::MethodSpec),
            ],
            CodedIndex::HasFieldMarshal => &[Some(T::Field), Some(T::Param)],
            CodedIndex::HasDeclSecurity => &[Some(T::TypeDef), Some(T::MethodDef), Some(T::Assembly)],
            CodedIndex::MemberRefParent => &[
                Some(T::TypeDef), Some(T::TypeRef), Some(T::ModuleRef), Some(T::MethodDef),
                Some(T::TypeSpec),
            ],
            CodedIndex::HasSemantics => &[Some(T::Event), Some(T::Property)],
            CodedIndex::MethodDefOrRef => &[Some(T::MethodDef), Some(T::MemberRef)],
            CodedIndex::MemberForwarded => &[Some(T::Field), Some(T::MethodDef)],
            CodedIndex::Implementation => &[Some(T::File), Some(T::AssemblyRef), Some(T::ExportedType)],
            CodedIndex::CustomAttributeType => &[None, None, Some(T::MethodDef), Some(T::MemberRef), None],
            CodedIndex::ResolutionScope => &[
                Some(T::Module), Some(T::ModuleRef), Some(T::AssemblyRef), Some(T::TypeRef),
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
            Error::BadImage(format!("invalid tag {tag} in {self:?} coded index {raw:#x}"))
        })?;
        Ok((table, raw >> bits))
    }

    pub fn encode(self, table: TableId, row: u32) -> Result<u32> {
        let tag = self
            .tables()
            .iter()
            .position(|t| *t == Some(table))
            .ok_or_else(|| Error::Malformed(format!("{table:?} cannot appear in a {self:?} coded index")))?;
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
        T::TypeDef => &[U32, Str, Str, Coded(C::TypeDefOrRef), Table(T::Field), Table(T::MethodDef)],
        T::FieldPtr => &[Table(T::Field)],
        T::Field => &[U16, Str, Blob],
        T::MethodPtr => &[Table(T::MethodDef)],
        T::MethodDef => &[MethodRva, U16, U16, Str, Blob, Table(T::Param)],
        T::ParamPtr => &[Table(T::Param)],
        T::Param => &[U16, U16, Str],
        T::InterfaceImpl => &[Table(T::TypeDef), Coded(C::TypeDefOrRef)],
        T::MemberRef => &[Coded(C::MemberRefParent), Str, Blob],
        T::Constant => &[U16, Coded(C::HasConstant), Blob],
        T::CustomAttribute => &[Coded(C::HasCustomAttribute), Coded(C::CustomAttributeType), Blob],
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
        T::MethodImpl => &[Table(T::TypeDef), Coded(C::MethodDefOrRef), Coded(C::MethodDefOrRef)],
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
                if self.rows[t.index()] < 0x1_0000 { 2 } else { 4 }
            }
            ColumnKind::Coded(c) => {
                let max = c
                    .tables()
                    .iter()
                    .flatten()
                    .map(|t| self.rows[t.index()])
                    .max()
                    .unwrap_or(0);
                if max < (1 << (16 - c.tag_bits())) { 2 } else { 4 }
            }
        }
    }

    pub fn row_size(&self, table: TableId) -> usize {
        columns(table).iter().map(|&k| self.column_size(k)).sum()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p metadata tables`
Expected: 8 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/metadata/src
git commit -m "feat(metadata): table schema, coded indexes and index sizes

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Heaps — readers and builders

**Files:**
- Create: `crates/metadata/src/heaps.rs`
- Modify: `crates/metadata/src/lib.rs` (add `pub mod heaps;`)

**Interfaces:**
- Consumes: `ByteReader`, `ByteWriter`, `compressed_len` (Task 1)
- Produces:
  - `StringHeap<'a>::new(&'a [u8])`, `get(&self, offset: u32) -> Result<&'a str>`
  - `BlobHeap<'a>::new(&'a [u8])`, `get(&self, offset: u32) -> Result<&'a [u8]>`
  - `GuidHeap<'a>::new(&'a [u8])`, `get(&self, index: u32) -> Result<Option<[u8; 16]>>`
  - `read_user_strings(data: &[u8]) -> Result<Vec<Vec<u16>>>` (entries in heap order, starting at offset 1, padding excluded)
  - `user_string_entry_size(units: usize) -> usize`, `user_string_flag(units: &[u16]) -> u8`
  - `StringHeapBuilder::new()`, `add(&mut self, &str) -> Result<u32>`, `finish(self) -> Vec<u8>`
  - `BlobHeapBuilder::new()`, `add(&mut self, &[u8]) -> Result<u32>`, `finish(self) -> Vec<u8>`
  - `GuidHeapBuilder::new()`, `add(&mut self, Option<[u8; 16]>) -> u32`, `finish(self) -> Vec<u8>`
  - `UserStringHeapBuilder::new()`, `add(&mut self, &[u16]) -> Result<u32>` (no dedupe — order is significant), `finish(self) -> Vec<u8>`
  - All `finish` results are zero-padded to a multiple of 4.

- [ ] **Step 1: Write the failing tests**

Create `crates/metadata/src/heaps.rs` with only:
```rust
//! The four metadata heaps: #Strings, #Blob, #GUID and #US (§II.24.2.2–§II.24.2.5).

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn string_heap_reads_nul_terminated_utf8() {
        let heap = StringHeap::new(b"\0abc\0de\0");
        assert_eq!(heap.get(0).unwrap(), "");
        assert_eq!(heap.get(1).unwrap(), "abc");
        assert_eq!(heap.get(5).unwrap(), "de");
        assert_eq!(heap.get(2).unwrap(), "bc");
        assert!(heap.get(9).is_err());
        assert!(StringHeap::new(b"\0ab").get(1).is_err(), "unterminated");
        assert!(StringHeap::new(b"\0\xFF\0").get(1).is_err(), "invalid UTF-8");
    }

    #[test]
    fn string_builder_dedupes_and_pads() {
        let mut b = StringHeapBuilder::new();
        assert_eq!(b.add("").unwrap(), 0);
        assert_eq!(b.add("abc").unwrap(), 1);
        assert_eq!(b.add("de").unwrap(), 5);
        assert_eq!(b.add("abc").unwrap(), 1);
        assert!(b.add("a\0b").is_err());
        let data = b.finish();
        assert_eq!(data.len() % 4, 0);
        let heap = StringHeap::new(&data);
        assert_eq!(heap.get(1).unwrap(), "abc");
        assert_eq!(heap.get(5).unwrap(), "de");
    }

    #[test]
    fn blob_heap_round_trip() {
        let mut b = BlobHeapBuilder::new();
        assert_eq!(b.add(&[]).unwrap(), 0);
        let small = b.add(&[1, 2, 3]).unwrap();
        let large_bytes = vec![7u8; 200];
        let large = b.add(&large_bytes).unwrap();
        assert_eq!(b.add(&[1, 2, 3]).unwrap(), small);
        let data = b.finish();
        assert_eq!(data.len() % 4, 0);
        let heap = BlobHeap::new(&data);
        assert_eq!(heap.get(0).unwrap(), &[] as &[u8]);
        assert_eq!(heap.get(small).unwrap(), &[1, 2, 3]);
        assert_eq!(heap.get(large).unwrap(), &large_bytes[..]);
        assert!(BlobHeap::new(&[0, 5, 1]).get(1).is_err(), "length past end");
    }

    #[test]
    fn guid_heap_is_one_based() {
        let g = [0xAB; 16];
        let mut b = GuidHeapBuilder::new();
        assert_eq!(b.add(None), 0);
        assert_eq!(b.add(Some(g)), 1);
        assert_eq!(b.add(Some(g)), 1);
        assert_eq!(b.add(Some([1; 16])), 2);
        let data = b.finish();
        let heap = GuidHeap::new(&data);
        assert_eq!(heap.get(0).unwrap(), None);
        assert_eq!(heap.get(1).unwrap(), Some(g));
        assert_eq!(heap.get(2).unwrap(), Some([1; 16]));
        assert!(heap.get(3).is_err());
    }

    #[test]
    fn user_string_offsets_and_flag() {
        let mut b = UserStringHeapBuilder::new();
        assert_eq!(b.add(&utf16("Hello")).unwrap(), 1);
        // "Hello": 5 units → 11 bytes (10 + flag) + 1 length byte = 12.
        assert_eq!(b.add(&utf16("x")).unwrap(), 13);
        assert_eq!(user_string_entry_size(5), 12);
        assert_eq!(user_string_flag(&utf16("abc")), 0);
        assert_eq!(user_string_flag(&utf16("it's")), 1);
        assert_eq!(user_string_flag(&utf16("a-b")), 1);
        assert_eq!(user_string_flag(&utf16("é")), 0);
        assert_eq!(user_string_flag(&utf16("€")), 1);
        let data = b.finish();
        assert_eq!(data.len() % 4, 0);
        assert_eq!(read_user_strings(&data).unwrap(), vec![utf16("Hello"), utf16("x")]);
    }

    #[test]
    fn user_string_edge_cases_round_trip() {
        let entries = vec![
            utf16(""),
            utf16("Grüße, 世界"),
            utf16("😀"),
            vec![0xD800], // unpaired surrogate
            utf16("tab\tquote'"),
            utf16(&"long".repeat(100)), // needs a 2-byte length prefix
        ];
        let mut b = UserStringHeapBuilder::new();
        let mut expected_offset = 1;
        for e in &entries {
            assert_eq!(b.add(e).unwrap() as usize, expected_offset);
            expected_offset += user_string_entry_size(e.len());
        }
        let data = b.finish();
        assert_eq!(read_user_strings(&data).unwrap(), entries);
    }

    #[test]
    fn malformed_user_string_heaps_are_errors() {
        assert!(read_user_strings(&[1]).is_err(), "heap must start with 0");
        assert!(read_user_strings(&[0, 2, 0x41, 0x00]).is_err(), "even length");
        assert!(read_user_strings(&[0, 9, 0x41]).is_err(), "length past end");
        assert_eq!(read_user_strings(&[]).unwrap(), Vec::<Vec<u16>>::new());
    }
}
```
Add `pub mod heaps;` to `lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p metadata heaps`
Expected: compile errors — `StringHeap` etc. not found.

- [ ] **Step 3: Implement `heaps.rs`**

Insert above the test module:
```rust
use std::collections::HashMap;

use crate::bytes::{ByteReader, ByteWriter, compressed_len};
use crate::{Error, Result};

/// #Strings: NUL-terminated UTF-8 strings addressed by byte offset (§II.24.2.3).
pub struct StringHeap<'a>(&'a [u8]);

impl<'a> StringHeap<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub fn get(&self, offset: u32) -> Result<&'a str> {
        let offset = offset as usize;
        if offset == 0 {
            return Ok("");
        }
        let tail = self.0.get(offset..).filter(|t| !t.is_empty()).ok_or_else(|| {
            Error::BadImage(format!("#Strings offset {offset:#x} out of range"))
        })?;
        let len = tail.iter().position(|&b| b == 0).ok_or_else(|| {
            Error::BadImage(format!("unterminated #Strings entry at {offset:#x}"))
        })?;
        std::str::from_utf8(&tail[..len])
            .map_err(|_| Error::BadImage(format!("invalid UTF-8 in #Strings at {offset:#x}")))
    }
}

/// #Blob: length-prefixed byte strings (§II.24.2.4).
pub struct BlobHeap<'a>(&'a [u8]);

impl<'a> BlobHeap<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub fn get(&self, offset: u32) -> Result<&'a [u8]> {
        if offset == 0 {
            return Ok(&[]);
        }
        let mut r = ByteReader::at(self.0, offset as usize);
        let len = r.compressed_u32()? as usize;
        r.bytes(len)
    }
}

/// #GUID: 16-byte GUIDs addressed by 1-based index (§II.24.2.5).
pub struct GuidHeap<'a>(&'a [u8]);

impl<'a> GuidHeap<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub fn get(&self, index: u32) -> Result<Option<[u8; 16]>> {
        if index == 0 {
            return Ok(None);
        }
        let start = (index as usize - 1) * 16;
        let bytes = self
            .0
            .get(start..start + 16)
            .ok_or_else(|| Error::BadImage(format!("#GUID index {index} out of range")))?;
        Ok(Some(bytes.try_into().unwrap()))
    }
}

/// Size in bytes of one #US entry holding `units` UTF-16 code units.
pub fn user_string_entry_size(units: usize) -> usize {
    let len = units * 2 + 1;
    compressed_len(len as u32) + len
}

/// Trailing byte of a #US entry: 1 if any unit needs more than 8-bit
/// handling, per §II.24.2.4.
pub fn user_string_flag(units: &[u16]) -> u8 {
    units.iter().any(|&c| {
        c > 0xFF || matches!(c, 0x01..=0x08 | 0x0E..=0x1F | 0x27 | 0x2D | 0x7F)
    }) as u8
}

/// Reads all #US entries in heap order. Offset 0 holds the mandatory empty
/// entry; a zero length byte after it marks the start of padding.
pub fn read_user_strings(data: &[u8]) -> Result<Vec<Vec<u16>>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if data[0] != 0 {
        return Err(Error::BadImage("#US heap must start with an empty entry".into()));
    }
    let mut r = ByteReader::at(data, 1);
    let mut entries = Vec::new();
    while r.remaining() > 0 {
        let start = r.pos();
        let len = r.compressed_u32()? as usize;
        if len == 0 {
            break;
        }
        if len % 2 == 0 {
            return Err(Error::BadImage(format!(
                "#US entry at {start:#x} has even length {len}"
            )));
        }
        let bytes = r.bytes(len)?;
        entries.push(
            bytes[..len - 1]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect(),
        );
    }
    Ok(entries)
}

fn padded(mut data: Vec<u8>) -> Vec<u8> {
    let len = crate::bytes::align_up(data.len(), 4);
    data.resize(len, 0);
    data
}

pub struct StringHeapBuilder {
    data: Vec<u8>,
    offsets: HashMap<String, u32>,
}

impl StringHeapBuilder {
    pub fn new() -> Self {
        Self { data: vec![0], offsets: HashMap::from([(String::new(), 0)]) }
    }

    pub fn add(&mut self, s: &str) -> Result<u32> {
        if let Some(&offset) = self.offsets.get(s) {
            return Ok(offset);
        }
        if s.contains('\0') {
            return Err(Error::Malformed(format!("string {s:?} contains NUL")));
        }
        let offset = self.data.len() as u32;
        self.data.extend_from_slice(s.as_bytes());
        self.data.push(0);
        self.offsets.insert(s.to_owned(), offset);
        Ok(offset)
    }

    pub fn finish(self) -> Vec<u8> {
        padded(self.data)
    }
}

impl Default for StringHeapBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BlobHeapBuilder {
    data: Vec<u8>,
    offsets: HashMap<Vec<u8>, u32>,
}

impl BlobHeapBuilder {
    pub fn new() -> Self {
        Self { data: vec![0], offsets: HashMap::new() }
    }

    pub fn add(&mut self, blob: &[u8]) -> Result<u32> {
        if blob.is_empty() {
            return Ok(0);
        }
        if let Some(&offset) = self.offsets.get(blob) {
            return Ok(offset);
        }
        let offset = self.data.len() as u32;
        let mut w = ByteWriter::new();
        w.compressed_u32(blob.len() as u32)?;
        self.data.extend_from_slice(&w.buf);
        self.data.extend_from_slice(blob);
        self.offsets.insert(blob.to_vec(), offset);
        Ok(offset)
    }

    pub fn finish(self) -> Vec<u8> {
        padded(self.data)
    }
}

impl Default for BlobHeapBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct GuidHeapBuilder {
    guids: Vec<[u8; 16]>,
}

impl GuidHeapBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, guid: Option<[u8; 16]>) -> u32 {
        let Some(guid) = guid else { return 0 };
        if let Some(i) = self.guids.iter().position(|g| *g == guid) {
            return i as u32 + 1;
        }
        self.guids.push(guid);
        self.guids.len() as u32
    }

    pub fn finish(self) -> Vec<u8> {
        self.guids.concat()
    }
}

/// Builds #US in insertion order. No deduplication: offsets must match the
/// order of `MetadataModel::user_strings` so `ldstr` tokens stay valid.
pub struct UserStringHeapBuilder {
    data: Vec<u8>,
}

impl UserStringHeapBuilder {
    pub fn new() -> Self {
        Self { data: vec![0] }
    }

    pub fn add(&mut self, units: &[u16]) -> Result<u32> {
        let offset = self.data.len() as u32;
        let mut w = ByteWriter::new();
        w.compressed_u32((units.len() * 2 + 1) as u32)?;
        for u in units {
            w.u16(*u);
        }
        w.u8(user_string_flag(units));
        self.data.extend_from_slice(&w.buf);
        Ok(offset)
    }

    pub fn finish(self) -> Vec<u8> {
        padded(self.data)
    }
}

impl Default for UserStringHeapBuilder {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p metadata heaps`
Expected: 7 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/metadata/src
git commit -m "feat(metadata): metadata heap readers and builders

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Method bodies

**Files:**
- Create: `crates/metadata/src/body.rs`
- Modify: `crates/metadata/src/lib.rs` (add `pub mod body;`)

**Interfaces:**
- Consumes: `ByteReader`, `ByteWriter`, `align_up` (Task 1)
- Produces:
  - `struct MethodBody { pub max_stack: u16, pub init_locals: bool, pub local_var_sig_token: u32, pub code: Vec<u8>, pub extra_sections: Vec<u8> }` (derives `Debug, Clone, PartialEq, Eq`)
  - `MethodBody::parse(data: &[u8]) -> Result<MethodBody>` — `data` starts at the header and may extend past the body
  - `MethodBody::is_tiny(&self) -> bool`
  - `MethodBody::encode(&self) -> Vec<u8>` — a fat body must be placed at a 4-byte-aligned address by the caller

Note: a tiny header implies `max_stack = 8`. A model body with `max_stack <= 8` and no locals/sections/init_locals is written tiny and re-reads with `max_stack = 8`; `mcs` never emits a fat header for such a body, so round trips of `mcs` output are exact.

- [ ] **Step 1: Write the failing tests**

Create `crates/metadata/src/body.rs` with only:
```rust
//! CIL method bodies: tiny and fat headers plus extra data sections (§II.25.4).

#[cfg(test)]
mod tests {
    use super::*;

    // `ldstr 0x70000001; call 0x0A000001; ret` with a tiny header.
    const TINY: &[u8] = &[0x2E, 0x72, 0x01, 0x00, 0x00, 0x70, 0x28, 0x01, 0x00, 0x00, 0x0A, 0x2A];

    #[test]
    fn parses_and_reencodes_a_tiny_body() {
        let body = MethodBody::parse(TINY).unwrap();
        assert_eq!(body.max_stack, 8);
        assert!(!body.init_locals);
        assert_eq!(body.local_var_sig_token, 0);
        assert_eq!(body.code, &TINY[1..]);
        assert!(body.is_tiny());
        assert_eq!(body.encode(), TINY);
    }

    #[test]
    fn tiny_parse_ignores_trailing_bytes() {
        let mut data = TINY.to_vec();
        data.extend_from_slice(&[0xFF; 8]);
        assert_eq!(MethodBody::parse(&data).unwrap().code, &TINY[1..]);
    }

    #[test]
    fn fat_body_with_locals() {
        let body = MethodBody {
            max_stack: 2,
            init_locals: true,
            local_var_sig_token: 0x1100_0001,
            code: vec![0x00, 0x00, 0x2A],
            extra_sections: vec![],
        };
        assert!(!body.is_tiny());
        let bytes = body.encode();
        assert_eq!(&bytes[..12], &[0x13, 0x30, 2, 0, 3, 0, 0, 0, 1, 0, 0, 0x11]);
        assert_eq!(MethodBody::parse(&bytes).unwrap(), body);
    }

    #[test]
    fn fat_body_with_exception_section() {
        // Small EH section: kind=1, DataSize=16 (4-byte header + one 12-byte clause).
        let mut section = vec![0x01, 16, 0, 0];
        section.extend_from_slice(&[0; 12]);
        let body = MethodBody {
            max_stack: 8,
            init_locals: false,
            local_var_sig_token: 0,
            code: vec![0x00, 0x00, 0x00, 0x00, 0x2A], // 12 + 5 = 17 → section at 20
            extra_sections: section.clone(),
        };
        let bytes = body.encode();
        assert_eq!(bytes[0] & 0x08, 0x08, "MoreSects flag set");
        assert_eq!(&bytes[20..], &section[..]);
        let mut with_trailing = bytes.clone();
        with_trailing.extend_from_slice(&[0xEE; 4]);
        assert_eq!(MethodBody::parse(&with_trailing).unwrap(), body);
    }

    #[test]
    fn code_of_64_bytes_needs_a_fat_header() {
        let body = MethodBody {
            max_stack: 8,
            init_locals: false,
            local_var_sig_token: 0,
            code: vec![0; 64],
            extra_sections: vec![],
        };
        assert!(!body.is_tiny());
        assert_eq!(MethodBody::parse(&body.encode()).unwrap(), body);
    }

    #[test]
    fn malformed_bodies_are_errors() {
        assert!(MethodBody::parse(&[]).is_err());
        assert!(MethodBody::parse(&[0x00]).is_err(), "invalid header kind");
        assert!(MethodBody::parse(&TINY[..5]).is_err(), "truncated tiny");
        assert!(MethodBody::parse(&[0x03, 0x30, 8, 0, 0xFF, 0, 0, 0, 0, 0, 0, 0]).is_err(), "truncated fat");
        assert!(MethodBody::parse(&[0x03, 0x20, 8, 0]).is_err(), "bad fat header size");
    }
}
```
Add `pub mod body;` to `lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p metadata body`
Expected: compile errors — `MethodBody` not found.

- [ ] **Step 3: Implement `body.rs`**

Insert above the test module:
```rust
use crate::bytes::{ByteReader, ByteWriter, align_up};
use crate::{Error, Result};

const TINY_FORMAT: u8 = 0x2;
const FAT_FORMAT: u16 = 0x3;
const FAT_MORE_SECTS: u16 = 0x8;
const FAT_INIT_LOCALS: u16 = 0x10;
const FAT_HEADER_DWORDS: u16 = 3;
const SECT_FAT_FORMAT: u8 = 0x40;
const SECT_MORE_SECTS: u8 = 0x80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodBody {
    pub max_stack: u16,
    pub init_locals: bool,
    /// StandAloneSig token of the local variable signature, or 0.
    pub local_var_sig_token: u32,
    pub code: Vec<u8>,
    /// Raw extra data sections (exception tables), starting at the first
    /// section header. Empty when the body has none.
    pub extra_sections: Vec<u8>,
}

impl MethodBody {
    pub fn parse(data: &[u8]) -> Result<MethodBody> {
        let first = *data
            .first()
            .ok_or_else(|| Error::BadImage("empty method body".into()))?;
        match first & 0x3 {
            TINY_FORMAT => {
                let size = (first >> 2) as usize;
                let code = data
                    .get(1..1 + size)
                    .ok_or_else(|| Error::BadImage("truncated tiny method body".into()))?;
                Ok(MethodBody {
                    max_stack: 8,
                    init_locals: false,
                    local_var_sig_token: 0,
                    code: code.to_vec(),
                    extra_sections: Vec::new(),
                })
            }
            0x3 => Self::parse_fat(data),
            _ => Err(Error::BadImage(format!(
                "invalid method body header byte {first:#04x}"
            ))),
        }
    }

    fn parse_fat(data: &[u8]) -> Result<MethodBody> {
        let mut r = ByteReader::new(data);
        let flags_and_size = r.u16()?;
        let flags = flags_and_size & 0x0FFF;
        if flags_and_size >> 12 != FAT_HEADER_DWORDS {
            return Err(Error::BadImage(format!(
                "fat method header size {} is not 3 dwords",
                flags_and_size >> 12
            )));
        }
        let max_stack = r.u16()?;
        let code_size = r.u32()? as usize;
        let local_var_sig_token = r.u32()?;
        let code = r.bytes(code_size)?.to_vec();
        let mut extra_sections = Vec::new();
        if flags & FAT_MORE_SECTS != 0 {
            r.align(4);
            let start = r.pos();
            loop {
                let section_start = r.pos();
                let kind = r.u8()?;
                let size = if kind & SECT_FAT_FORMAT != 0 {
                    let b = r.bytes(3)?;
                    u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16)
                } else {
                    u32::from(r.u8()?)
                } as usize;
                if size < 4 {
                    return Err(Error::BadImage(format!(
                        "method data section of size {size} is smaller than its header"
                    )));
                }
                r.seek(section_start);
                r.bytes(size)?;
                if kind & SECT_MORE_SECTS == 0 {
                    break;
                }
                r.align(4);
            }
            extra_sections = data[start..r.pos()].to_vec();
        }
        Ok(MethodBody {
            max_stack,
            init_locals: flags & FAT_INIT_LOCALS != 0,
            local_var_sig_token,
            code,
            extra_sections,
        })
    }

    pub fn is_tiny(&self) -> bool {
        self.code.len() < 64
            && self.max_stack <= 8
            && self.local_var_sig_token == 0
            && !self.init_locals
            && self.extra_sections.is_empty()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = ByteWriter::new();
        if self.is_tiny() {
            w.u8(((self.code.len() as u8) << 2) | TINY_FORMAT);
            w.bytes(&self.code);
            return w.into_vec();
        }
        let mut flags = FAT_FORMAT | (FAT_HEADER_DWORDS << 12);
        if self.init_locals {
            flags |= FAT_INIT_LOCALS;
        }
        if !self.extra_sections.is_empty() {
            flags |= FAT_MORE_SECTS;
        }
        w.u16(flags);
        w.u16(self.max_stack);
        w.u32(self.code.len() as u32);
        w.u32(self.local_var_sig_token);
        w.bytes(&self.code);
        if !self.extra_sections.is_empty() {
            w.pad_to(align_up(w.len(), 4));
            w.bytes(&self.extra_sections);
        }
        w.into_vec()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p metadata body`
Expected: 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/metadata/src
git commit -m "feat(metadata): parse and encode CIL method bodies

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Signatures

**Files:**
- Create: `crates/metadata/src/sig.rs`
- Modify: `crates/metadata/src/lib.rs` (add `pub mod sig;`)

**Interfaces:**
- Consumes: `ByteReader`, `ByteWriter` (Task 1), `TableId` (Task 2)
- Produces:
  - `struct TypeDefOrRef { pub table: TableId, pub row: u32 }` (`Copy`)
  - `enum TypeSig` — `Void, Boolean, Char, I1, U1, I2, U2, I4, U4, I8, U8, R4, R8, String, Object, I, U, TypedByRef, Ptr(Box<TypeSig>), ByRef(Box<TypeSig>), Pinned(Box<TypeSig>), SzArray(Box<TypeSig>), ValueType(TypeDefOrRef), Class(TypeDefOrRef), Var(u32), MVar(u32), Array { elem: Box<TypeSig>, rank: u32, sizes: Vec<u32>, lo_bounds: Vec<i32> }, GenericInst { value_type: bool, ty: TypeDefOrRef, args: Vec<TypeSig> }, FnPtr(Box<MethodSig>), Modified { required: bool, modifier: TypeDefOrRef, ty: Box<TypeSig> }`
  - `struct MethodSig { pub conv: u8, pub generic_params: u32, pub ret: TypeSig, pub params: Vec<TypeSig>, pub sentinel: Option<usize> }`
  - `mod conv { DEFAULT=0x00, VARARG=0x05, FIELD=0x06, LOCAL=0x07, PROPERTY=0x08, GENERIC=0x10, HAS_THIS=0x20, EXPLICIT_THIS=0x40 }` (all `u8`)
  - `decode_method_sig(&[u8]) -> Result<MethodSig>`, `encode_method_sig(&MethodSig) -> Result<Vec<u8>>`
  - `decode_field_sig(&[u8]) -> Result<TypeSig>`, `encode_field_sig(&TypeSig) -> Result<Vec<u8>>`
  - `decode_local_sig(&[u8]) -> Result<Vec<TypeSig>>`, `encode_local_sig(&[TypeSig]) -> Result<Vec<u8>>`
  - `decode_type_spec(&[u8]) -> Result<TypeSig>`, `encode_type_spec(&TypeSig) -> Result<Vec<u8>>`
  - Decoders reject trailing bytes and nesting deeper than 64 levels.

- [ ] **Step 1: Write the failing tests**

Create `crates/metadata/src/sig.rs` with only:
```rust
//! Signature blobs (§II.23.2).

#[cfg(test)]
mod tests {
    use super::*;

    fn type_ref(row: u32) -> TypeDefOrRef {
        TypeDefOrRef { table: TableId::TypeRef, row }
    }

    fn method_round_trip(blob: &[u8], expected: MethodSig) {
        let decoded = decode_method_sig(blob).unwrap();
        assert_eq!(decoded, expected);
        assert_eq!(encode_method_sig(&decoded).unwrap(), blob);
    }

    #[test]
    fn static_void_with_string_parameter() {
        method_round_trip(
            &[0x00, 0x01, 0x01, 0x0E],
            MethodSig { conv: 0, generic_params: 0, ret: TypeSig::Void, params: vec![TypeSig::String], sentinel: None },
        );
    }

    #[test]
    fn generic_instance_method() {
        // instance !!0 M<T>(!!0[], int32)
        method_round_trip(
            &[0x30, 0x01, 0x02, 0x1E, 0x00, 0x1D, 0x1E, 0x00, 0x08],
            MethodSig {
                conv: conv::HAS_THIS | conv::GENERIC,
                generic_params: 1,
                ret: TypeSig::MVar(0),
                params: vec![TypeSig::SzArray(Box::new(TypeSig::MVar(0))), TypeSig::I4],
                sentinel: None,
            },
        );
    }

    #[test]
    fn vararg_sentinel() {
        method_round_trip(
            &[0x05, 0x02, 0x01, 0x08, 0x41, 0x0E],
            MethodSig { conv: conv::VARARG, generic_params: 0, ret: TypeSig::Void, params: vec![TypeSig::I4, TypeSig::String], sentinel: Some(1) },
        );
    }

    #[test]
    fn type_specs() {
        // List<int32> where List is TypeRef row 3: tag 1 → (3 << 2) | 1 = 0x0D.
        let generic = [0x15, 0x12, 0x0D, 0x01, 0x08];
        let ty = decode_type_spec(&generic).unwrap();
        assert_eq!(ty, TypeSig::GenericInst { value_type: false, ty: type_ref(3), args: vec![TypeSig::I4] });
        assert_eq!(encode_type_spec(&ty).unwrap(), generic);

        // int32[,] with rank 2, no sizes, two zero lower bounds.
        let array = [0x14, 0x08, 0x02, 0x00, 0x02, 0x00, 0x00];
        let ty = decode_type_spec(&array).unwrap();
        assert_eq!(ty, TypeSig::Array { elem: Box::new(TypeSig::I4), rank: 2, sizes: vec![], lo_bounds: vec![0, 0] });
        assert_eq!(encode_type_spec(&ty).unwrap(), array);

        // Negative lower bound uses the signed compressed form.
        let negative = [0x14, 0x08, 0x01, 0x01, 0x05, 0x01, 0x7B];
        let ty = decode_type_spec(&negative).unwrap();
        assert_eq!(ty, TypeSig::Array { elem: Box::new(TypeSig::I4), rank: 1, sizes: vec![5], lo_bounds: vec![-3] });
        assert_eq!(encode_type_spec(&ty).unwrap(), negative);
    }

    #[test]
    fn field_with_required_modifier() {
        let blob = [0x06, 0x1F, 0x05, 0x08];
        let ty = decode_field_sig(&blob).unwrap();
        assert_eq!(ty, TypeSig::Modified { required: true, modifier: type_ref(1), ty: Box::new(TypeSig::I4) });
        assert_eq!(encode_field_sig(&ty).unwrap(), blob);
    }

    #[test]
    fn local_signature() {
        let blob = [0x07, 0x03, 0x45, 0x0F, 0x08, 0x10, 0x0E, 0x11, 0x08];
        let locals = decode_local_sig(&blob).unwrap();
        assert_eq!(
            locals,
            vec![
                TypeSig::Pinned(Box::new(TypeSig::Ptr(Box::new(TypeSig::I4)))),
                TypeSig::ByRef(Box::new(TypeSig::String)),
                TypeSig::ValueType(TypeDefOrRef { table: TableId::TypeDef, row: 2 }),
            ]
        );
        assert_eq!(encode_local_sig(&locals).unwrap(), blob);
    }

    #[test]
    fn function_pointer() {
        let blob = [0x1B, 0x00, 0x00, 0x08];
        let ty = decode_type_spec(&blob).unwrap();
        assert_eq!(
            ty,
            TypeSig::FnPtr(Box::new(MethodSig { conv: 0, generic_params: 0, ret: TypeSig::I4, params: vec![], sentinel: None }))
        );
        assert_eq!(encode_type_spec(&ty).unwrap(), blob);
    }

    #[test]
    fn malformed_signatures_are_errors() {
        assert!(decode_type_spec(&[0x50]).is_err(), "unknown element type");
        assert!(decode_type_spec(&[0x08, 0x08]).is_err(), "trailing bytes");
        assert!(decode_field_sig(&[0x07, 0x08]).is_err(), "wrong lead byte");
        assert!(decode_local_sig(&[0x06, 0x00]).is_err(), "wrong lead byte");
        assert!(decode_type_spec(&[0x12, 0x03]).is_err(), "TypeDefOrRef tag 3");
        assert!(decode_method_sig(&[0x00, 0x05, 0x01]).is_err(), "truncated params");
    }

    #[test]
    fn deeply_nested_signature_is_an_error() {
        let mut blob = vec![0x0F; 10_000]; // Ptr(Ptr(Ptr(...)))
        blob.push(0x08);
        assert!(decode_type_spec(&blob).is_err());
    }
}
```
Add `pub mod sig;` to `lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p metadata sig`
Expected: compile errors — `TypeSig`, `decode_method_sig` etc. not found.

- [ ] **Step 3: Implement `sig.rs`**

Insert above the test module:
```rust
use crate::bytes::{ByteReader, ByteWriter};
use crate::tables::TableId;
use crate::{Error, Result};

const MAX_DEPTH: usize = 64;

/// Calling-convention byte values (§II.23.2.1–§II.23.2.6).
pub mod conv {
    pub const DEFAULT: u8 = 0x00;
    pub const VARARG: u8 = 0x05;
    pub const FIELD: u8 = 0x06;
    pub const LOCAL: u8 = 0x07;
    pub const PROPERTY: u8 = 0x08;
    pub const GENERIC: u8 = 0x10;
    pub const HAS_THIS: u8 = 0x20;
    pub const EXPLICIT_THIS: u8 = 0x40;
}

mod element {
    pub const VOID: u8 = 0x01;
    pub const BOOLEAN: u8 = 0x02;
    pub const CHAR: u8 = 0x03;
    pub const I1: u8 = 0x04;
    pub const U1: u8 = 0x05;
    pub const I2: u8 = 0x06;
    pub const U2: u8 = 0x07;
    pub const I4: u8 = 0x08;
    pub const U4: u8 = 0x09;
    pub const I8: u8 = 0x0A;
    pub const U8: u8 = 0x0B;
    pub const R4: u8 = 0x0C;
    pub const R8: u8 = 0x0D;
    pub const STRING: u8 = 0x0E;
    pub const PTR: u8 = 0x0F;
    pub const BYREF: u8 = 0x10;
    pub const VALUETYPE: u8 = 0x11;
    pub const CLASS: u8 = 0x12;
    pub const VAR: u8 = 0x13;
    pub const ARRAY: u8 = 0x14;
    pub const GENERICINST: u8 = 0x15;
    pub const TYPEDBYREF: u8 = 0x16;
    pub const I: u8 = 0x18;
    pub const U: u8 = 0x19;
    pub const FNPTR: u8 = 0x1B;
    pub const OBJECT: u8 = 0x1C;
    pub const SZARRAY: u8 = 0x1D;
    pub const MVAR: u8 = 0x1E;
    pub const CMOD_REQD: u8 = 0x1F;
    pub const CMOD_OPT: u8 = 0x20;
    pub const SENTINEL: u8 = 0x41;
    pub const PINNED: u8 = 0x45;
}

/// A TypeDefOrRefOrSpecEncoded reference (§II.23.2.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeDefOrRef {
    pub table: TableId,
    pub row: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSig {
    Void,
    Boolean,
    Char,
    I1,
    U1,
    I2,
    U2,
    I4,
    U4,
    I8,
    U8,
    R4,
    R8,
    String,
    Object,
    I,
    U,
    TypedByRef,
    Ptr(Box<TypeSig>),
    ByRef(Box<TypeSig>),
    Pinned(Box<TypeSig>),
    SzArray(Box<TypeSig>),
    ValueType(TypeDefOrRef),
    Class(TypeDefOrRef),
    Var(u32),
    MVar(u32),
    Array { elem: Box<TypeSig>, rank: u32, sizes: Vec<u32>, lo_bounds: Vec<i32> },
    GenericInst { value_type: bool, ty: TypeDefOrRef, args: Vec<TypeSig> },
    FnPtr(Box<MethodSig>),
    Modified { required: bool, modifier: TypeDefOrRef, ty: Box<TypeSig> },
}

/// Method, property or function-pointer signature (§II.23.2.1–§II.23.2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodSig {
    /// Raw calling-convention byte, including HAS_THIS/GENERIC flags.
    pub conv: u8,
    pub generic_params: u32,
    pub ret: TypeSig,
    pub params: Vec<TypeSig>,
    /// Index into `params` before which a vararg SENTINEL appears.
    pub sentinel: Option<usize>,
}

const TDOR_TABLES: [TableId; 3] = [TableId::TypeDef, TableId::TypeRef, TableId::TypeSpec];

fn finish<T>(r: &ByteReader, value: T) -> Result<T> {
    if r.remaining() != 0 {
        return Err(Error::BadImage(format!(
            "{} trailing bytes in signature",
            r.remaining()
        )));
    }
    Ok(value)
}

fn read_tdor(r: &mut ByteReader) -> Result<TypeDefOrRef> {
    let raw = r.compressed_u32()?;
    let table = *TDOR_TABLES.get((raw & 3) as usize).ok_or_else(|| {
        Error::BadImage(format!("invalid TypeDefOrRef tag in signature value {raw:#x}"))
    })?;
    Ok(TypeDefOrRef { table, row: raw >> 2 })
}

fn write_tdor(w: &mut ByteWriter, t: TypeDefOrRef) -> Result<()> {
    let tag = TDOR_TABLES.iter().position(|x| *x == t.table).ok_or_else(|| {
        Error::Malformed(format!("{:?} is not a TypeDefOrRef table", t.table))
    })?;
    w.compressed_u32((t.row << 2) | tag as u32)
}

fn read_type(r: &mut ByteReader, depth: usize) -> Result<TypeSig> {
    use element as e;
    if depth > MAX_DEPTH {
        return Err(Error::BadImage("signature nested too deeply".into()));
    }
    let inner = |r: &mut ByteReader| read_type(r, depth + 1).map(Box::new);
    let b = r.u8()?;
    Ok(match b {
        e::VOID => TypeSig::Void,
        e::BOOLEAN => TypeSig::Boolean,
        e::CHAR => TypeSig::Char,
        e::I1 => TypeSig::I1,
        e::U1 => TypeSig::U1,
        e::I2 => TypeSig::I2,
        e::U2 => TypeSig::U2,
        e::I4 => TypeSig::I4,
        e::U4 => TypeSig::U4,
        e::I8 => TypeSig::I8,
        e::U8 => TypeSig::U8,
        e::R4 => TypeSig::R4,
        e::R8 => TypeSig::R8,
        e::STRING => TypeSig::String,
        e::OBJECT => TypeSig::Object,
        e::I => TypeSig::I,
        e::U => TypeSig::U,
        e::TYPEDBYREF => TypeSig::TypedByRef,
        e::PTR => TypeSig::Ptr(inner(r)?),
        e::BYREF => TypeSig::ByRef(inner(r)?),
        e::PINNED => TypeSig::Pinned(inner(r)?),
        e::SZARRAY => TypeSig::SzArray(inner(r)?),
        e::VALUETYPE => TypeSig::ValueType(read_tdor(r)?),
        e::CLASS => TypeSig::Class(read_tdor(r)?),
        e::VAR => TypeSig::Var(r.compressed_u32()?),
        e::MVAR => TypeSig::MVar(r.compressed_u32()?),
        e::ARRAY => {
            let elem = inner(r)?;
            let rank = r.compressed_u32()?;
            let num_sizes = r.compressed_u32()?;
            let sizes = (0..num_sizes).map(|_| r.compressed_u32()).collect::<Result<_>>()?;
            let num_lo = r.compressed_u32()?;
            let lo_bounds = (0..num_lo).map(|_| r.compressed_i32()).collect::<Result<_>>()?;
            TypeSig::Array { elem, rank, sizes, lo_bounds }
        }
        e::GENERICINST => {
            let value_type = match r.u8()? {
                e::VALUETYPE => true,
                e::CLASS => false,
                other => {
                    return Err(Error::BadImage(format!(
                        "GENERICINST must be followed by CLASS or VALUETYPE, found {other:#04x}"
                    )));
                }
            };
            let ty = read_tdor(r)?;
            let count = r.compressed_u32()?;
            let args = (0..count).map(|_| read_type(r, depth + 1)).collect::<Result<_>>()?;
            TypeSig::GenericInst { value_type, ty, args }
        }
        e::FNPTR => TypeSig::FnPtr(Box::new(read_method(r, depth + 1)?)),
        e::CMOD_REQD | e::CMOD_OPT => TypeSig::Modified {
            required: b == e::CMOD_REQD,
            modifier: read_tdor(r)?,
            ty: inner(r)?,
        },
        _ => {
            return Err(Error::BadImage(format!(
                "unknown element type {b:#04x} in signature"
            )));
        }
    })
}

fn write_type(w: &mut ByteWriter, t: &TypeSig) -> Result<()> {
    use element as e;
    match t {
        TypeSig::Void => w.u8(e::VOID),
        TypeSig::Boolean => w.u8(e::BOOLEAN),
        TypeSig::Char => w.u8(e::CHAR),
        TypeSig::I1 => w.u8(e::I1),
        TypeSig::U1 => w.u8(e::U1),
        TypeSig::I2 => w.u8(e::I2),
        TypeSig::U2 => w.u8(e::U2),
        TypeSig::I4 => w.u8(e::I4),
        TypeSig::U4 => w.u8(e::U4),
        TypeSig::I8 => w.u8(e::I8),
        TypeSig::U8 => w.u8(e::U8),
        TypeSig::R4 => w.u8(e::R4),
        TypeSig::R8 => w.u8(e::R8),
        TypeSig::String => w.u8(e::STRING),
        TypeSig::Object => w.u8(e::OBJECT),
        TypeSig::I => w.u8(e::I),
        TypeSig::U => w.u8(e::U),
        TypeSig::TypedByRef => w.u8(e::TYPEDBYREF),
        TypeSig::Ptr(inner) => {
            w.u8(e::PTR);
            write_type(w, inner)?;
        }
        TypeSig::ByRef(inner) => {
            w.u8(e::BYREF);
            write_type(w, inner)?;
        }
        TypeSig::Pinned(inner) => {
            w.u8(e::PINNED);
            write_type(w, inner)?;
        }
        TypeSig::SzArray(inner) => {
            w.u8(e::SZARRAY);
            write_type(w, inner)?;
        }
        TypeSig::ValueType(t) => {
            w.u8(e::VALUETYPE);
            write_tdor(w, *t)?;
        }
        TypeSig::Class(t) => {
            w.u8(e::CLASS);
            write_tdor(w, *t)?;
        }
        TypeSig::Var(n) => {
            w.u8(e::VAR);
            w.compressed_u32(*n)?;
        }
        TypeSig::MVar(n) => {
            w.u8(e::MVAR);
            w.compressed_u32(*n)?;
        }
        TypeSig::Array { elem, rank, sizes, lo_bounds } => {
            w.u8(e::ARRAY);
            write_type(w, elem)?;
            w.compressed_u32(*rank)?;
            w.compressed_u32(sizes.len() as u32)?;
            for s in sizes {
                w.compressed_u32(*s)?;
            }
            w.compressed_u32(lo_bounds.len() as u32)?;
            for lo in lo_bounds {
                w.compressed_i32(*lo)?;
            }
        }
        TypeSig::GenericInst { value_type, ty, args } => {
            w.u8(e::GENERICINST);
            w.u8(if *value_type { e::VALUETYPE } else { e::CLASS });
            write_tdor(w, *ty)?;
            w.compressed_u32(args.len() as u32)?;
            for a in args {
                write_type(w, a)?;
            }
        }
        TypeSig::FnPtr(m) => {
            w.u8(e::FNPTR);
            write_method(w, m)?;
        }
        TypeSig::Modified { required, modifier, ty } => {
            w.u8(if *required { e::CMOD_REQD } else { e::CMOD_OPT });
            write_tdor(w, *modifier)?;
            write_type(w, ty)?;
        }
    }
    Ok(())
}

fn read_method(r: &mut ByteReader, depth: usize) -> Result<MethodSig> {
    let conv = r.u8()?;
    let generic_params = if conv & conv::GENERIC != 0 { r.compressed_u32()? } else { 0 };
    let count = r.compressed_u32()?;
    let ret = read_type(r, depth + 1)?;
    let mut params = Vec::with_capacity(count.min(256) as usize);
    let mut sentinel = None;
    for i in 0..count as usize {
        if r.peek_u8()? == element::SENTINEL {
            r.u8()?;
            sentinel = Some(i);
        }
        params.push(read_type(r, depth + 1)?);
    }
    Ok(MethodSig { conv, generic_params, ret, params, sentinel })
}

fn write_method(w: &mut ByteWriter, m: &MethodSig) -> Result<()> {
    w.u8(m.conv);
    if m.conv & conv::GENERIC != 0 {
        w.compressed_u32(m.generic_params)?;
    }
    w.compressed_u32(m.params.len() as u32)?;
    write_type(w, &m.ret)?;
    for (i, p) in m.params.iter().enumerate() {
        if m.sentinel == Some(i) {
            w.u8(element::SENTINEL);
        }
        write_type(w, p)?;
    }
    Ok(())
}

pub fn decode_method_sig(blob: &[u8]) -> Result<MethodSig> {
    let mut r = ByteReader::new(blob);
    let sig = read_method(&mut r, 0)?;
    finish(&r, sig)
}

pub fn encode_method_sig(sig: &MethodSig) -> Result<Vec<u8>> {
    let mut w = ByteWriter::new();
    write_method(&mut w, sig)?;
    Ok(w.into_vec())
}

pub fn decode_field_sig(blob: &[u8]) -> Result<TypeSig> {
    let mut r = ByteReader::new(blob);
    let lead = r.u8()?;
    if lead != conv::FIELD {
        return Err(Error::BadImage(format!("field signature starts with {lead:#04x}")));
    }
    let ty = read_type(&mut r, 0)?;
    finish(&r, ty)
}

pub fn encode_field_sig(ty: &TypeSig) -> Result<Vec<u8>> {
    let mut w = ByteWriter::new();
    w.u8(conv::FIELD);
    write_type(&mut w, ty)?;
    Ok(w.into_vec())
}

pub fn decode_local_sig(blob: &[u8]) -> Result<Vec<TypeSig>> {
    let mut r = ByteReader::new(blob);
    let lead = r.u8()?;
    if lead != conv::LOCAL {
        return Err(Error::BadImage(format!("local signature starts with {lead:#04x}")));
    }
    let count = r.compressed_u32()?;
    let mut locals = Vec::with_capacity(count.min(256) as usize);
    for _ in 0..count {
        locals.push(read_type(&mut r, 0)?);
    }
    finish(&r, locals)
}

pub fn encode_local_sig(locals: &[TypeSig]) -> Result<Vec<u8>> {
    let mut w = ByteWriter::new();
    w.u8(conv::LOCAL);
    w.compressed_u32(locals.len() as u32)?;
    for l in locals {
        write_type(&mut w, l)?;
    }
    Ok(w.into_vec())
}

pub fn decode_type_spec(blob: &[u8]) -> Result<TypeSig> {
    let mut r = ByteReader::new(blob);
    let ty = read_type(&mut r, 0)?;
    finish(&r, ty)
}

pub fn encode_type_spec(ty: &TypeSig) -> Result<Vec<u8>> {
    let mut w = ByteWriter::new();
    write_type(&mut w, ty)?;
    Ok(w.into_vec())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p metadata sig`
Expected: 9 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/metadata/src
git commit -m "feat(metadata): decode and encode signature blobs

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: PE reader, CLI header, fixtures, oracle detection

**Files:**
- Create: `crates/metadata/src/pe.rs`, `crates/metadata/src/cli.rs`, `crates/metadata/build.rs`, `scripts/regen-fixtures.sh`, `crates/metadata/tests/fixtures/src/hello.cs`, `crates/metadata/tests/fixtures/src/lib.cs`, `crates/metadata/tests/pe.rs`
- Generate + commit: `crates/metadata/tests/fixtures/hello.exe`, `crates/metadata/tests/fixtures/lib.dll`
- Modify: `crates/metadata/src/lib.rs` (add `pub mod cli; pub mod pe;`)

**Interfaces:**
- Consumes: `ByteReader`, `ByteWriter` (Task 1)
- Produces:
  - `pe::DataDirectory { pub rva: u32, pub size: u32 }` (`Debug, Clone, Copy, Default, PartialEq, Eq`)
  - `pe::Section { pub name: [u8; 8], pub virtual_address: u32, pub virtual_size: u32, pub raw_size: u32, pub raw_pointer: u32 }`
  - `pe::PeImage<'a> { pub sections: Vec<Section>, pub cli_header: DataDirectory, .. }`: `parse(&'a [u8]) -> Result<Self>`, `slice(&self, rva: u32, len: u32) -> Result<&'a [u8]>`, `slice_from(&self, rva: u32) -> Result<&'a [u8]>`
  - `cli::CliHeader { pub runtime_major: u16, pub runtime_minor: u16, pub metadata: DataDirectory, pub flags: u32, pub entry_point_token: u32, pub resources: DataDirectory, pub strong_name: DataDirectory }`: `parse(&[u8]) -> Result<Self>`, `write(&self, &mut ByteWriter)` (always 72 bytes), `cli::CLI_HEADER_SIZE: usize = 72`
  - `cli::COMIMAGE_FLAGS_ILONLY: u32 = 0x1`, `COMIMAGE_FLAGS_32BITREQUIRED = 0x2`, `COMIMAGE_FLAGS_STRONGNAMESIGNED = 0x8`
  - cfgs for tests: `has_mono` (mono runs), `has_mono_corlib` (with env `MONO_CORLIB` set at compile time to the path)
  - Fixtures: `tests/fixtures/hello.exe`, `tests/fixtures/lib.dll`

- [ ] **Step 1: Add fixture sources and the regeneration script**

`crates/metadata/tests/fixtures/src/hello.cs`:
```csharp
using System;

namespace HelloApp {
    static class Program {
        static void Main() {
            Console.WriteLine("Hello, World!");
        }
    }
}
```

`crates/metadata/tests/fixtures/src/lib.cs` (exercises most tables, fat bodies, exception sections, generics):
```csharp
using System;
using System.Collections.Generic;

namespace Fixtures {
    public interface IShape { double Area(); }

    public struct Point {
        public int X;
        public int Y;
        public Point(int x, int y) { X = x; Y = y; }
    }

    public class Circle : IShape {
        public const double Pi = 3.14159;
        private static int s_count;
        public int[,] Grid = new int[2, 3];

        public double Radius { get; set; }
        public event EventHandler Changed;

        public Circle(double r) { Radius = r; s_count++; }

        public double Area() { return Pi * Radius * Radius; }

        public static T First<T>(List<T> items) { return items[0]; }

        public static int FirstInt(List<int> items) { return First(items); } // emits a MethodSpec

        public static int SafeDivide(int a, int b) {
            try { return a / b; }
            catch (DivideByZeroException) { return 0; }
            finally { s_count--; }
        }

        public void Raise() {
            var handler = Changed;
            if (handler != null) handler(this, EventArgs.Empty);
        }

        public class Nested { }
    }

    [Obsolete("fixture")]
    public enum Color { Red, Green, Blue }
}
```

`scripts/regen-fixtures.sh`:
```bash
#!/usr/bin/env bash
# Regenerates the committed mcs-compiled test fixtures. Requires Mono's mcs.
set -euo pipefail
cd "$(dirname "$0")/.."
fixtures=crates/metadata/tests/fixtures
mcs -out:"$fixtures/hello.exe" "$fixtures/src/hello.cs"
mcs -target:library -out:"$fixtures/lib.dll" "$fixtures/src/lib.cs"
echo "fixtures regenerated in $fixtures"
```

Run:
```bash
chmod +x scripts/regen-fixtures.sh && scripts/regen-fixtures.sh && mono crates/metadata/tests/fixtures/hello.exe
```
Expected: `fixtures regenerated ...` then `Hello, World!`. (A CS0067-style warning from `lib.cs` is fine.)

- [ ] **Step 2: Add `build.rs` for oracle detection**

`crates/metadata/build.rs`:
```rust
//! Detects the Mono oracle tools so tests depending on them are ignored
//! (visibly, with a reason) instead of silently passing.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_mono)");
    println!("cargo::rustc-check-cfg=cfg(has_mono_corlib)");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=MONO_CORLIB");

    let mono_runs = Command::new("mono")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if mono_runs {
        println!("cargo::rustc-cfg=has_mono");
    } else {
        println!("cargo::warning=mono not found on PATH: mono oracle tests will be ignored");
    }

    let corlib = std::env::var("MONO_CORLIB")
        .unwrap_or_else(|_| "/usr/lib/mono/4.5/mscorlib.dll".to_owned());
    if Path::new(&corlib).is_file() {
        println!("cargo::rustc-cfg=has_mono_corlib");
        println!("cargo::rustc-env=MONO_CORLIB={corlib}");
    } else {
        println!(
            "cargo::warning=Mono mscorlib.dll not found at {corlib} (set MONO_CORLIB): corlib tests will be ignored"
        );
    }
}
```

- [ ] **Step 3: Write the failing tests**

`crates/metadata/tests/pe.rs`:
```rust
use metadata::cli::{COMIMAGE_FLAGS_ILONLY, CliHeader};
use metadata::pe::PeImage;

const HELLO: &[u8] = include_bytes!("fixtures/hello.exe");
const LIB: &[u8] = include_bytes!("fixtures/lib.dll");

#[test]
fn parses_hello_cli_header() {
    let pe = PeImage::parse(HELLO).unwrap();
    assert!(pe.cli_header.rva != 0);
    let cli = CliHeader::parse(pe.slice(pe.cli_header.rva, 72).unwrap()).unwrap();
    assert_eq!(cli.flags & COMIMAGE_FLAGS_ILONLY, COMIMAGE_FLAGS_ILONLY);
    assert_eq!(cli.entry_point_token, 0x0600_0001);
    let metadata = pe.slice(cli.metadata.rva, cli.metadata.size).unwrap();
    assert_eq!(&metadata[..4], b"BSJB");
}

#[test]
fn library_has_no_entry_point() {
    let pe = PeImage::parse(LIB).unwrap();
    let cli = CliHeader::parse(pe.slice(pe.cli_header.rva, 72).unwrap()).unwrap();
    assert_eq!(cli.entry_point_token, 0);
}

#[test]
fn slice_from_reaches_section_end() {
    let pe = PeImage::parse(HELLO).unwrap();
    let text = pe.sections.iter().find(|s| s.name.starts_with(b".text")).unwrap();
    let tail = pe.slice_from(text.virtual_address).unwrap();
    assert_eq!(tail.len(), text.raw_size as usize);
}

#[test]
fn rejects_non_pe_input() {
    assert!(PeImage::parse(b"hello").is_err());
    assert!(PeImage::parse(&[]).is_err());
    let mut no_pe_sig = HELLO.to_vec();
    no_pe_sig[0x80] = b'X';
    assert!(PeImage::parse(&no_pe_sig).is_err());
}

#[test]
fn rva_outside_sections_is_an_error() {
    let pe = PeImage::parse(HELLO).unwrap();
    assert!(pe.slice(0x10, 4).is_err());
    assert!(pe.slice(0x7FFF_0000, 4).is_err());
    assert!(pe.slice(pe.cli_header.rva, 0x10_0000).is_err());
}

#[test]
fn truncated_image_is_an_error() {
    assert!(PeImage::parse(&HELLO[..0x100]).is_err());
    let pe = PeImage::parse(&HELLO[..0x200]).unwrap(); // headers intact, sections cut off
    assert!(pe.slice(pe.cli_header.rva, 72).is_err());
}

#[test]
fn cli_header_round_trip() {
    let pe = PeImage::parse(HELLO).unwrap();
    let bytes = pe.slice(pe.cli_header.rva, 72).unwrap();
    let cli = CliHeader::parse(bytes).unwrap();
    let mut w = metadata::bytes::ByteWriter::new();
    cli.write(&mut w);
    assert_eq!(w.len(), 72);
    assert_eq!(CliHeader::parse(&w.buf).unwrap(), cli);
    assert!(CliHeader::parse(&bytes[..40]).is_err());
}
```
Add to `lib.rs`:
```rust
pub mod cli;
pub mod pe;
```
and create empty `crates/metadata/src/pe.rs` and `crates/metadata/src/cli.rs` (one doc-comment line each) so the crate compiles.

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p metadata --test pe`
Expected: compile errors — `PeImage`, `CliHeader` not found.

- [ ] **Step 5: Implement `pe.rs`**

```rust
//! PE/COFF image reader (§II.25.2): just enough to locate the CLI header and
//! map RVAs to file bytes. PE32 and PE32+ are both accepted.

use crate::bytes::ByteReader;
use crate::{Error, Result};

const CLI_DIRECTORY_INDEX: usize = 14;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DataDirectory {
    pub rva: u32,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: [u8; 8],
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_size: u32,
    pub raw_pointer: u32,
}

#[derive(Debug)]
pub struct PeImage<'a> {
    data: &'a [u8],
    pub sections: Vec<Section>,
    pub cli_header: DataDirectory,
}

impl<'a> PeImage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = ByteReader::new(data);
        if r.bytes(2)? != b"MZ" {
            return Err(Error::BadImage("missing MZ signature".into()));
        }
        r.seek(0x3C);
        let pe_offset = r.u32()? as usize;
        r.seek(pe_offset);
        if r.bytes(4)? != b"PE\0\0" {
            return Err(Error::BadImage("missing PE signature".into()));
        }
        let _machine = r.u16()?;
        let section_count = r.u16()?;
        r.bytes(12)?; // timestamp, symbol table pointer, symbol count
        let optional_size = r.u16()? as usize;
        let _characteristics = r.u16()?;

        let optional_start = r.pos();
        let directories_offset = match r.u16()? {
            0x10B => 96,  // PE32
            0x20B => 112, // PE32+
            magic => {
                return Err(Error::BadImage(format!("unknown optional header magic {magic:#x}")));
            }
        };
        r.seek(optional_start + directories_offset - 4);
        let directory_count = r.u32()? as usize;
        if directory_count <= CLI_DIRECTORY_INDEX {
            return Err(Error::BadImage("image has no CLI header data directory".into()));
        }
        r.seek(optional_start + directories_offset + CLI_DIRECTORY_INDEX * 8);
        let cli_header = DataDirectory { rva: r.u32()?, size: r.u32()? };
        if cli_header.rva == 0 {
            return Err(Error::BadImage("not a CLI image: CLI header directory is empty".into()));
        }

        r.seek(optional_start + optional_size);
        let mut sections = Vec::with_capacity(section_count as usize);
        for _ in 0..section_count {
            let name: [u8; 8] = r.bytes(8)?.try_into().unwrap();
            let virtual_size = r.u32()?;
            let virtual_address = r.u32()?;
            let raw_size = r.u32()?;
            let raw_pointer = r.u32()?;
            r.bytes(16)?; // relocations, line numbers, characteristics
            sections.push(Section { name, virtual_address, virtual_size, raw_size, raw_pointer });
        }
        Ok(PeImage { data, sections, cli_header })
    }

    /// File offset of `rva` and the number of file bytes available from there
    /// to the end of its section.
    fn locate(&self, rva: u32) -> Result<(usize, usize)> {
        for s in &self.sections {
            let span = s.virtual_size.max(s.raw_size);
            if rva < s.virtual_address || rva - s.virtual_address >= span {
                continue;
            }
            let delta = rva - s.virtual_address;
            if delta >= s.raw_size {
                return Err(Error::BadImage(format!("RVA {rva:#x} is not backed by file data")));
            }
            let offset = s.raw_pointer as usize + delta as usize;
            if offset >= self.data.len() {
                return Err(Error::BadImage(format!("RVA {rva:#x} lies beyond the end of the file")));
            }
            let available = ((s.raw_size - delta) as usize).min(self.data.len() - offset);
            return Ok((offset, available));
        }
        Err(Error::BadImage(format!("RVA {rva:#x} is not inside any section")))
    }

    pub fn slice(&self, rva: u32, len: u32) -> Result<&'a [u8]> {
        let (offset, available) = self.locate(rva)?;
        if len as usize > available {
            return Err(Error::BadImage(format!(
                "{len} bytes at RVA {rva:#x} exceed the section's file data"
            )));
        }
        Ok(&self.data[offset..offset + len as usize])
    }

    pub fn slice_from(&self, rva: u32) -> Result<&'a [u8]> {
        let (offset, available) = self.locate(rva)?;
        Ok(&self.data[offset..offset + available])
    }
}
```

- [ ] **Step 6: Implement `cli.rs`**

```rust
//! The CLI header (§II.25.3.3).

use crate::bytes::{ByteReader, ByteWriter};
use crate::pe::DataDirectory;
use crate::{Error, Result};

pub const CLI_HEADER_SIZE: usize = 72;

pub const COMIMAGE_FLAGS_ILONLY: u32 = 0x1;
pub const COMIMAGE_FLAGS_32BITREQUIRED: u32 = 0x2;
pub const COMIMAGE_FLAGS_STRONGNAMESIGNED: u32 = 0x8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliHeader {
    pub runtime_major: u16,
    pub runtime_minor: u16,
    pub metadata: DataDirectory,
    pub flags: u32,
    pub entry_point_token: u32,
    pub resources: DataDirectory,
    pub strong_name: DataDirectory,
}

fn read_dir(r: &mut ByteReader) -> Result<DataDirectory> {
    Ok(DataDirectory { rva: r.u32()?, size: r.u32()? })
}

fn write_dir(w: &mut ByteWriter, d: DataDirectory) {
    w.u32(d.rva);
    w.u32(d.size);
}

impl CliHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < CLI_HEADER_SIZE {
            return Err(Error::BadImage(format!("CLI header truncated to {} bytes", bytes.len())));
        }
        let mut r = ByteReader::new(bytes);
        let cb = r.u32()? as usize;
        if cb < CLI_HEADER_SIZE {
            return Err(Error::BadImage(format!("CLI header size {cb} is smaller than 72")));
        }
        Ok(CliHeader {
            runtime_major: r.u16()?,
            runtime_minor: r.u16()?,
            metadata: read_dir(&mut r)?,
            flags: r.u32()?,
            entry_point_token: r.u32()?,
            resources: read_dir(&mut r)?,
            strong_name: read_dir(&mut r)?,
        })
    }

    pub fn write(&self, w: &mut ByteWriter) {
        w.u32(CLI_HEADER_SIZE as u32);
        w.u16(self.runtime_major);
        w.u16(self.runtime_minor);
        write_dir(w, self.metadata);
        w.u32(self.flags);
        w.u32(self.entry_point_token);
        write_dir(w, self.resources);
        write_dir(w, self.strong_name);
        // Code manager table, VTable fixups, export address table jumps and
        // managed native header: always zero for IL-only images.
        w.zeros(32);
    }
}
```
Note: the explicit length check matters — the fields `parse` reads span exactly 40 bytes, so without it `CliHeader::parse(&bytes[..40])` would wrongly succeed.

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p metadata --test pe`
Expected: 7 tests PASS. The build prints no `mono not found` warning on this machine.

- [ ] **Step 8: Commit**

```bash
git add scripts crates/metadata
git commit -m "feat(metadata): PE reader, CLI header and mcs fixtures

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Logical model and assembly reader

**Files:**
- Create: `crates/metadata/src/model.rs`, `crates/metadata/src/reader.rs`, `crates/metadata/tests/reader.rs`
- Modify: `crates/metadata/src/lib.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–6
- Produces:
  - `enum Cell { U16(u16), U32(u32), Str(String), Blob(Vec<u8>), Guid(Option<[u8; 16]>), Ref(TableId, u32), Body(Option<MethodBody>) }` with `as_u32() -> Option<u32>` (U16 or U32), `as_str() -> Option<&str>`, `as_blob() -> Option<&[u8]>`, `as_index() -> Option<(TableId, u32)>`, `as_body() -> Option<&MethodBody>`, `kind_name() -> &'static str`. `Ref` holds a simple or coded index; row 0 means null.
  - `type Row = Vec<Cell>;`
  - `struct MetadataModel { pub version: String, pub cli_flags: u32, pub entry_point_token: u32, pub tables: Vec<Vec<Row>>, pub user_strings: Vec<Vec<u16>> }` (`Debug, Clone, PartialEq, Eq`); `tables.len() == TABLE_COUNT`, indexed by `TableId::index()`
  - `MetadataModel::new()` (= `Default`: version `"v4.0.30319"`, `cli_flags = COMIMAGE_FLAGS_ILONLY`, no entry point, empty tables), `table(&self, TableId) -> &[Row]`, `table_mut(&mut self, TableId) -> &mut Vec<Row>`, `push_row(&mut self, TableId, Row) -> u32` (returns 1-based row), `add_user_string(&mut self, &str) -> Result<u32>` (returns the `0x70xxxxxx` token), `user_string(&self, token: u32) -> Option<&[u16]>`
  - `read_assembly(data: &[u8]) -> Result<MetadataModel>`
  - Re-exports in `lib.rs`: `pub use model::{Cell, MetadataModel, Row}; pub use reader::read_assembly; pub use tables::TableId;`

Column positions used by tests (from `columns()`): TypeRef `[scope, name, namespace]`; TypeDef `[flags, name, namespace, extends, field_list, method_list]`; MethodDef `[body, impl_flags, flags, name, signature, param_list]`; MemberRef `[parent, name, signature]`; AssemblyRef `[major, minor, build, revision, flags, public_key_or_token, name, culture, hash]`; Module `[generation, name, mvid, enc_id, enc_base_id]`.

- [ ] **Step 1: Write the failing tests**

`crates/metadata/tests/reader.rs`:
```rust
use metadata::cli::COMIMAGE_FLAGS_ILONLY;
use metadata::{Cell, MetadataModel, Row, TableId, read_assembly};

const HELLO: &[u8] = include_bytes!("fixtures/hello.exe");
const LIB: &[u8] = include_bytes!("fixtures/lib.dll");

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn type_def<'a>(m: &'a MetadataModel, ns: &str, name: &str) -> Option<&'a Row> {
    m.table(TableId::TypeDef)
        .iter()
        .find(|r| r[1].as_str() == Some(name) && r[2].as_str() == Some(ns))
}

fn method<'a>(m: &'a MetadataModel, name: &str) -> &'a Row {
    m.table(TableId::MethodDef)
        .iter()
        .find(|r| r[3].as_str() == Some(name))
        .unwrap_or_else(|| panic!("method {name} not found"))
}

#[test]
fn reads_hello_header_values() {
    let m = read_assembly(HELLO).unwrap();
    assert_eq!(m.version, "v4.0.30319");
    assert_eq!(m.entry_point_token, 0x0600_0001);
    assert_eq!(m.cli_flags & COMIMAGE_FLAGS_ILONLY, COMIMAGE_FLAGS_ILONLY);
    assert_eq!(m.table(TableId::Module)[0][1].as_str(), Some("hello.exe"));
}

#[test]
fn reads_hello_types_and_main_body() {
    let m = read_assembly(HELLO).unwrap();
    assert!(type_def(&m, "HelloApp", "Program").is_some());
    let main = method(&m, "Main");
    let body = main[0].as_body().expect("Main has IL");
    // ldstr 0x70000001; call 0x0A000001; ret
    assert_eq!(body.code, [0x72, 0x01, 0x00, 0x00, 0x70, 0x28, 0x01, 0x00, 0x00, 0x0A, 0x2A]);
}

#[test]
fn reads_hello_references() {
    let m = read_assembly(HELLO).unwrap();
    let write_line = &m.table(TableId::MemberRef)[0];
    assert_eq!(write_line[1].as_str(), Some("WriteLine"));
    let (parent_table, parent_row) = write_line[0].as_index().unwrap();
    assert_eq!(parent_table, TableId::TypeRef);
    let console = &m.table(TableId::TypeRef)[parent_row as usize - 1];
    assert_eq!(console[1].as_str(), Some("Console"));
    assert_eq!(console[2].as_str(), Some("System"));

    let mscorlib = m
        .table(TableId::AssemblyRef)
        .iter()
        .find(|r| r[6].as_str() == Some("mscorlib"))
        .expect("mscorlib reference");
    let version: Vec<u32> = mscorlib[..4].iter().map(|c| c.as_u32().unwrap()).collect();
    assert_eq!(version, [4, 0, 0, 0]);

    assert_eq!(m.user_string(0x7000_0001), Some(&utf16("Hello, World!")[..]));
}

#[test]
fn reads_lib_tables() {
    let m = read_assembly(LIB).unwrap();
    for name in ["IShape", "Point", "Circle", "Color"] {
        assert!(type_def(&m, "Fixtures", name).is_some(), "{name}");
    }
    assert!(type_def(&m, "", "Nested").is_some(), "nested types have no namespace");
    for t in [
        TableId::Field,
        TableId::Param,
        TableId::InterfaceImpl,
        TableId::Constant,
        TableId::CustomAttribute,
        TableId::StandAloneSig,
        TableId::EventMap,
        TableId::Event,
        TableId::PropertyMap,
        TableId::Property,
        TableId::MethodSemantics,
        TableId::TypeSpec,
        TableId::NestedClass,
        TableId::GenericParam,
        TableId::MethodSpec,
    ] {
        assert!(!m.table(t).is_empty(), "{t:?} should not be empty");
    }
    let safe_divide = method(&m, "SafeDivide")[0].as_body().unwrap();
    assert!(!safe_divide.extra_sections.is_empty(), "try/catch/finally emits an EH section");
    assert!(method(&m, "Area")[0].as_body().is_some());
    assert!(m.table(TableId::MethodDef).iter().any(|r| matches!(r[0], Cell::Body(None))), "interface methods have no body");
}

#[test]
fn model_user_strings() {
    let mut m = MetadataModel::new();
    assert_eq!(m.tables.len(), metadata::tables::TABLE_COUNT);
    assert_eq!(m.add_user_string("Hello").unwrap(), 0x7000_0001);
    assert_eq!(m.add_user_string("x").unwrap(), 0x7000_000D);
    assert_eq!(m.user_string(0x7000_000D), Some(&utf16("x")[..]));
    assert_eq!(m.user_string(0x7000_0002), None, "not an entry start");
    assert_eq!(m.push_row(TableId::TypeRef, vec![]), 1);
    assert_eq!(m.push_row(TableId::TypeRef, vec![]), 2);
}

#[test]
fn truncated_inputs_never_panic() {
    for len in (0..HELLO.len()).step_by(7) {
        let _ = read_assembly(&HELLO[..len]);
    }
    assert!(read_assembly(&HELLO[..0x200]).is_err());
}

#[test]
fn bit_flipped_inputs_never_panic() {
    for i in 0..HELLO.len() {
        let mut corrupted = HELLO.to_vec();
        corrupted[i] ^= 0xFF;
        let _ = read_assembly(&corrupted);
    }
}

#[cfg_attr(not(has_mono_corlib), ignore = "Mono mscorlib.dll not found (set MONO_CORLIB)")]
#[test]
fn reads_mono_mscorlib() {
    let path = option_env!("MONO_CORLIB").unwrap();
    let m = read_assembly(&std::fs::read(path).unwrap()).unwrap();
    assert!(type_def(&m, "System", "Object").is_some());
    assert!(type_def(&m, "System", "String").is_some());
    assert!(type_def(&m, "System", "Console").is_some());
    assert!(m.table(TableId::TypeDef).len() > 1000);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p metadata --test reader`
Expected: compile errors — `read_assembly`, `MetadataModel`, `Cell` not found.

- [ ] **Step 3: Implement `model.rs`**

```rust
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
            return Err(Error::Malformed("#US heap exceeds the 16 MiB token range".into()));
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
```

- [ ] **Step 4: Implement `reader.rs`**

```rust
//! Reads a PE/CLI image into a [`MetadataModel`] (§II.24).

use crate::body::MethodBody;
use crate::bytes::{ByteReader, align_up};
use crate::cli::{CLI_HEADER_SIZE, CliHeader};
use crate::heaps::{BlobHeap, GuidHeap, StringHeap, read_user_strings};
use crate::model::{Cell, MetadataModel, Row};
use crate::pe::PeImage;
use crate::tables::{ColumnKind, IndexSizes, TABLE_COUNT, TableId, columns};
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
            return Err(Error::Unsupported("uncompressed (#-) metadata tables".into()));
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

    Ok(MetadataModel {
        version: root.version,
        cli_flags: cli.flags,
        entry_point_token: cli.entry_point_token,
        tables,
        user_strings,
    })
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
        let end = version_bytes.iter().position(|&b| b == 0).unwrap_or(version_len);
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
                .ok_or_else(|| Error::BadImage("metadata stream lies outside the metadata".into()))?;
            streams.push((String::from_utf8_lossy(&name).into_owned(), data));
        }
        Ok(MetadataRoot { version, streams })
    }

    fn stream(&self, name: &str) -> Option<&'a [u8]> {
        self.streams.iter().find(|(n, _)| n == name).map(|(_, d)| *d)
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
            return Err(Error::Unsupported(format!("unknown metadata table {bit:#04x}")));
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
```

Update `crates/metadata/src/lib.rs` to:
```rust
//! ECMA-335 metadata: PE/COFF images, CLI header, heaps, tables and signatures.
//!
//! Section references such as §II.23.2 point into ECMA-335, 6th edition,
//! which is the authoritative specification for everything in this crate.

pub mod body;
pub mod bytes;
pub mod cli;
pub mod error;
pub mod heaps;
pub mod model;
pub mod pe;
pub mod reader;
pub mod sig;
pub mod tables;

pub use error::{Error, Result};
pub use model::{Cell, MetadataModel, Row};
pub use reader::read_assembly;
pub use tables::TableId;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p metadata --test reader`
Expected: 8 tests PASS (including `reads_mono_mscorlib` on this machine). If `bit_flipped_inputs_never_panic` panics, the panic message names the code path: fix that path to return `Err` (do not catch panics).

- [ ] **Step 6: Run the whole crate's tests**

Run: `cargo test -p metadata`
Expected: all tests PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/metadata
git commit -m "feat(metadata): logical model and assembly reader

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Assembly writer

**Files:**
- Create: `crates/metadata/src/writer.rs`, `crates/metadata/tests/writer.rs`
- Modify: `crates/metadata/src/lib.rs` (add `pub mod writer;` and `pub use writer::{ImageKind, write_assembly};`)

**Interfaces:**
- Consumes: everything from Tasks 1–7
- Produces:
  - `enum ImageKind { Exe, Dll }`
  - `write_assembly(model: &MetadataModel, kind: ImageKind) -> Result<Vec<u8>>`
  - Guarantees: `read_assembly(&write_assembly(m, k)?)? == m` for any model the reader produced from a non-signed image without FieldRVA/ManifestResource rows, provided the model's sorted tables are sorted (leaf sorted tables are sorted by the writer).
  - Errors: `Malformed` for wrong cell kinds, wrong column counts, out-of-range references, an `Exe` with entry token 0, values that overflow 2-byte columns; `Unsupported` for FieldRVA/ManifestResource rows and unsorted InterfaceImpl/GenericParam/GenericParamConstraint.

Image layout (PE32, i386, one `.text` + one `.reloc` section, §II.25):
```
.text @ RVA 0x2000, file 0x200:
  +0x00 IAT (8 bytes: hint/name RVA, 0)
  +0x08 CLI header (72 bytes)
  +0x50 method bodies (fat bodies 4-aligned)
  align 4: metadata root + streams (#~, #Strings, #US, #GUID, #Blob)
  align 4: import directory (2 × 20 bytes), lookup table (8), hint/name ("_CorExeMain"/"_CorDllMain"), "mscoree.dll"
  align 4 + 2: entry stub FF 25 <IMAGE_BASE + IAT RVA>
.reloc: one block, one HIGHLOW fixup for the stub's operand
```

- [ ] **Step 1: Write the failing tests**

`crates/metadata/tests/writer.rs`:
```rust
use metadata::pe::PeImage;
use metadata::tables::CodedIndex;
use metadata::{Cell, Error, ImageKind, MetadataModel, TableId, read_assembly, write_assembly};

const HELLO: &[u8] = include_bytes!("fixtures/hello.exe");
const LIB: &[u8] = include_bytes!("fixtures/lib.dll");

fn round_trip(model: &MetadataModel, kind: ImageKind) -> MetadataModel {
    let bytes = write_assembly(model, kind).unwrap();
    read_assembly(&bytes).unwrap()
}

#[cfg(has_mono)]
fn run_on_mono(bytes: &[u8], name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, bytes).unwrap();
    let out = std::process::Command::new("mono").arg(&path).output().unwrap();
    assert!(out.status.success(), "mono failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn with_wide_heaps() -> MetadataModel {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::TypeRef).push(vec![
        Cell::Ref(TableId::AssemblyRef, 1),
        Cell::Str("N".repeat(70_000)),
        Cell::Str(String::new()),
    ]);
    m.table_mut(TableId::StandAloneSig).push(vec![Cell::Blob(vec![0x06; 70_000])]);
    m
}

fn with_wide_indexes() -> MetadataModel {
    let mut m = read_assembly(HELLO).unwrap();
    for i in 0..20_000 {
        m.table_mut(TableId::TypeRef).push(vec![
            Cell::Ref(TableId::AssemblyRef, 1),
            Cell::Str(format!("T{i}")),
            Cell::Str("Generated".into()),
        ]);
    }
    m
}

#[test]
fn hello_round_trips() {
    let m = read_assembly(HELLO).unwrap();
    assert_eq!(round_trip(&m, ImageKind::Exe), m);
}

#[test]
fn lib_round_trips() {
    let m = read_assembly(LIB).unwrap();
    assert_eq!(round_trip(&m, ImageKind::Dll), m);
}

#[test]
fn written_image_is_deterministic_and_parseable() {
    let m = read_assembly(HELLO).unwrap();
    let a = write_assembly(&m, ImageKind::Exe).unwrap();
    let b = write_assembly(&m, ImageKind::Exe).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len() % 0x200, 0);
    let pe = PeImage::parse(&a).unwrap();
    assert_eq!(pe.sections.len(), 2);
}

#[test]
fn wide_heaps_round_trip() {
    let m = with_wide_heaps();
    assert_eq!(round_trip(&m, ImageKind::Exe), m);
}

#[test]
fn wide_table_indexes_round_trip() {
    let m = with_wide_indexes();
    assert_eq!(round_trip(&m, ImageKind::Exe), m);
}

#[test]
fn writer_sorts_custom_attributes() {
    let original = read_assembly(LIB).unwrap();
    let mut shuffled = original.clone();
    shuffled.table_mut(TableId::CustomAttribute).reverse();
    let reread = round_trip(&shuffled, ImageKind::Dll);
    let attrs = reread.table(TableId::CustomAttribute);
    let keys: Vec<u32> = attrs
        .iter()
        .map(|r| {
            let (t, row) = r[0].as_index().unwrap();
            CodedIndex::HasCustomAttribute.encode(t, row).unwrap()
        })
        .collect();
    assert!(keys.windows(2).all(|w| w[0] <= w[1]), "sorted by parent");
    let as_strings = |rows: &[metadata::Row]| {
        let mut v: Vec<String> = rows.iter().map(|r| format!("{r:?}")).collect();
        v.sort();
        v
    };
    assert_eq!(as_strings(attrs), as_strings(original.table(TableId::CustomAttribute)));
}

#[test]
fn wrong_cell_kind_is_malformed() {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::TypeRef)[0][1] = Cell::U32(5);
    assert!(matches!(write_assembly(&m, ImageKind::Exe), Err(Error::Malformed(_))));
}

#[test]
fn wrong_column_count_is_malformed() {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::TypeRef)[0].pop();
    assert!(matches!(write_assembly(&m, ImageKind::Exe), Err(Error::Malformed(_))));
}

#[test]
fn out_of_range_reference_is_malformed() {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::MemberRef)[0][0] = Cell::Ref(TableId::TypeRef, 999);
    assert!(matches!(write_assembly(&m, ImageKind::Exe), Err(Error::Malformed(_))));
}

#[test]
fn exe_requires_entry_point() {
    let mut m = read_assembly(HELLO).unwrap();
    m.entry_point_token = 0;
    assert!(matches!(write_assembly(&m, ImageKind::Exe), Err(Error::Malformed(_))));
    assert!(write_assembly(&m, ImageKind::Dll).is_ok());
}

#[test]
fn field_rva_is_unsupported() {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::FieldRva).push(vec![Cell::U32(0x2000), Cell::Ref(TableId::Field, 1)]);
    assert!(matches!(write_assembly(&m, ImageKind::Exe), Err(Error::Unsupported(_))));
}

#[cfg_attr(not(has_mono), ignore = "mono not installed")]
#[test]
fn mono_runs_rewritten_hello() {
    #[cfg(has_mono)]
    {
        let bytes = write_assembly(&read_assembly(HELLO).unwrap(), ImageKind::Exe).unwrap();
        assert_eq!(run_on_mono(&bytes, "hello-rewritten.exe"), "Hello, World!\n");
    }
}

#[cfg_attr(not(has_mono), ignore = "mono not installed")]
#[test]
fn mono_runs_hello_with_wide_heaps() {
    #[cfg(has_mono)]
    {
        let bytes = write_assembly(&with_wide_heaps(), ImageKind::Exe).unwrap();
        assert_eq!(run_on_mono(&bytes, "hello-wide-heaps.exe"), "Hello, World!\n");
    }
}

#[cfg_attr(not(has_mono), ignore = "mono not installed")]
#[test]
fn mono_runs_hello_with_wide_indexes() {
    #[cfg(has_mono)]
    {
        let bytes = write_assembly(&with_wide_indexes(), ImageKind::Exe).unwrap();
        assert_eq!(run_on_mono(&bytes, "hello-wide-indexes.exe"), "Hello, World!\n");
    }
}
```
Add `pub mod writer;` and `pub use writer::{ImageKind, write_assembly};` to `lib.rs`, and create `writer.rs` with just `ImageKind` and a `write_assembly` that returns `Err(Error::Unsupported("not implemented".into()))` so the tests compile.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p metadata --test writer`
Expected: most tests FAIL with `Unsupported("not implemented")`.

- [ ] **Step 3: Implement `writer.rs`**

```rust
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
        metadata: DataDirectory { rva: metadata_rva, size: metadata.len() as u32 },
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
        return Err(Error::Malformed("an executable needs an entry point token".into()));
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
        _ => Err(Error::Malformed(format!("{table:?} row has no index in key column {col}"))),
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
                return Err(Error::Malformed(format!("{target:?} row {row} is out of range")));
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

fn build_metadata(model: &MetadataModel, tables: &[Vec<Row>], body_rvas: &[u32]) -> Result<Vec<u8>> {
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
            let body_rva = if t == TableId::MethodDef { body_rvas[i] } else { 0 };
            for (&kind, cell) in kinds.iter().zip(row) {
                let value = raw_value(kind, cell, &mut heaps, &counts, body_rva).map_err(|e| match e {
                    Error::Malformed(m) => Error::Malformed(format!("{t:?} row {}: {m}", i + 1)),
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
        return Err(Error::Malformed("metadata version string is too long".into()));
    }
    version_bytes.resize(version_len, 0);

    let header_len = 16
        + version_len
        + 4
        + streams.iter().map(|(name, _)| 8 + align_up(name.len() + 1, 4)).sum::<usize>();
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

    Imports { import_dir_rva, import_dir_size, hint_name_rva, entry_stub_rva }
}

fn section_header(w: &mut ByteWriter, name: &[u8; 8], virtual_size: u32, rva: u32, raw_size: u32, raw_pointer: u32, characteristics: u32) {
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

    section_header(&mut w, b".text\0\0\0", text_size, TEXT_RVA, text_raw_size, HEADERS_SIZE, 0x6000_0020);
    section_header(&mut w, b".reloc\0\0", RELOC_SIZE, reloc_rva, FILE_ALIGNMENT, HEADERS_SIZE + text_raw_size, 0x4200_0040);
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p metadata --test writer`
Expected: 14 tests PASS, including the three `mono_runs_*` oracle tests. If a `mono_runs_*` test fails while the round trips pass, our reader and writer agree on something mono reads differently: compare the field against ECMA-335 §II.24/§II.25 (use superpowers:systematic-debugging), not against decompiled code.

- [ ] **Step 5: Run the whole crate's tests**

Run: `cargo test -p metadata`
Expected: all tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/metadata
git commit -m "feat(metadata): write PE32 CLI images from the logical model

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Mono mscorlib stress test and CI

**Files:**
- Create: `crates/metadata/tests/corlib.rs`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `read_assembly`, `sig::*`, `TableId` (Tasks 5, 7)
- Produces: CI workflow running `cargo fmt --check` and `cargo test --workspace` with Mono installed.

- [ ] **Step 1: Write the stress test**

`crates/metadata/tests/corlib.rs`:
```rust
//! Decodes and re-encodes every signature in Mono's mscorlib.dll. The
//! re-encoded bytes must equal the originals, which pins both directions of
//! the signature codec against a large real-world assembly.

use metadata::sig::{
    decode_field_sig, decode_local_sig, decode_method_sig, decode_type_spec, encode_field_sig,
    encode_local_sig, encode_method_sig, encode_type_spec,
};
use metadata::{MetadataModel, Result, TableId, read_assembly};

fn reencode(blob: &[u8], kind: &str) -> Result<Vec<u8>> {
    match kind {
        "method" => encode_method_sig(&decode_method_sig(blob)?),
        "field" => encode_field_sig(&decode_field_sig(blob)?),
        "local" => encode_local_sig(&decode_local_sig(blob)?),
        "type" => encode_type_spec(&decode_type_spec(blob)?),
        _ => unreachable!(),
    }
}

fn member_kind(blob: &[u8]) -> &'static str {
    match blob.first() {
        Some(0x06) => "field",
        Some(0x07) => "local",
        _ => "method",
    }
}

fn check(m: &MetadataModel, table: TableId, col: usize, kind_of: impl Fn(&[u8]) -> &'static str) -> Vec<String> {
    let mut failures = Vec::new();
    for (i, row) in m.table(table).iter().enumerate() {
        let blob = row[col].as_blob().unwrap();
        let kind = kind_of(blob);
        match reencode(blob, kind) {
            Ok(bytes) if bytes == blob => {}
            Ok(bytes) => failures.push(format!("{table:?} row {}: {kind} re-encoded as {bytes:02X?}, original {blob:02X?}", i + 1)),
            Err(e) => failures.push(format!("{table:?} row {}: {kind} {e}", i + 1)),
        }
    }
    failures
}

#[cfg_attr(not(has_mono_corlib), ignore = "Mono mscorlib.dll not found (set MONO_CORLIB)")]
#[test]
fn every_mscorlib_signature_round_trips() {
    let path = option_env!("MONO_CORLIB").unwrap();
    let m = read_assembly(&std::fs::read(path).unwrap()).unwrap();
    let mut failures = Vec::new();
    failures.extend(check(&m, TableId::MethodDef, 4, |_| "method"));
    failures.extend(check(&m, TableId::Field, 2, |_| "field"));
    failures.extend(check(&m, TableId::MemberRef, 2, member_kind));
    failures.extend(check(&m, TableId::StandAloneSig, 0, member_kind));
    failures.extend(check(&m, TableId::TypeSpec, 0, |_| "type"));
    assert!(
        failures.is_empty(),
        "{} signatures failed, first 10:\n{}",
        failures.len(),
        failures.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[cfg_attr(not(has_mono_corlib), ignore = "Mono mscorlib.dll not found (set MONO_CORLIB)")]
#[test]
fn every_mscorlib_method_body_was_decoded() {
    let path = option_env!("MONO_CORLIB").unwrap();
    let m = read_assembly(&std::fs::read(path).unwrap()).unwrap();
    let with_bodies = m
        .table(TableId::MethodDef)
        .iter()
        .filter(|r| r[0].as_body().is_some())
        .count();
    assert!(with_bodies > 10_000, "only {with_bodies} bodies decoded");
}
```

- [ ] **Step 2: Run the stress test**

Run: `cargo test -p metadata --test corlib`
Expected: 2 tests PASS. If signatures fail, the message lists the first ten with bytes; fix the codec in `sig.rs` (add a unit test reproducing the failing blob in `sig.rs` first), then re-run.

- [ ] **Step 3: Add CI**

`.github/workflows/ci.yml`:
```yaml
name: CI

on:
  push:
  pull_request:

jobs:
  test:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - name: Install Mono (oracle for tests)
        run: sudo apt-get update && sudo apt-get install -y mono-devel
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt
      - run: cargo fmt --all --check
      - run: cargo test --workspace
```

- [ ] **Step 4: Format and run everything locally**

Run: `cargo fmt --all && cargo fmt --all --check && cargo test --workspace`
Expected: no formatting diff after `cargo fmt`; all tests PASS; no tests reported as `ignored` on this machine.

- [ ] **Step 5: Commit**

```bash
git add crates/metadata .github
git commit -m "test(metadata): mscorlib signature stress test and CI workflow

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>"
```

---

## Stage 1 completion check

Stage 1 of the spec is done when, on this machine:

- `cargo test --workspace` passes with **0 ignored** tests,
- `reads_mono_mscorlib`, `hello_round_trips`, `lib_round_trips` and `mono_runs_rewritten_hello` are among the passing tests.

The next plan (stage 2: runtime) builds on `read_assembly`, `MetadataModel`, `Cell`, `TableId`, `MethodBody` and `sig::*`.
