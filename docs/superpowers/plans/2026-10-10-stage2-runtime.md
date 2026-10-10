# Stage 2 — Runtime (`clr`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust runtime and `clr` binary that loads an `mcs`-compiled `hello.exe`, binds it to **our** `mscorlib.dll` (built from `corlib/*.cs` with `mcs -nostdlib`), interprets its IL and prints `Hello, World!` — spec config R.

**Architecture:** A `runtime` library crate on top of the stage 1 `metadata` crate: a loader (assemblies bound by simple name), a lazy type system, method resolution (MemberRef → MethodDef by name + structural signature match), a bump-allocated heap addressed by opaque `ObjRef`s, and an IL interpreter with an explicit frame stack and typed evaluation stack. Body-less corlib methods are internal calls implemented in Rust. A thin `clr` binary crate adds argument parsing and exit codes; an integration harness compiles `tests/programs/*.cs` with `mcs` and runs them on `clr`.

**Tech Stack:** Rust stable (edition 2024), workspace crates `metadata` (stage 1), `runtime`, `clr`. Mono 6.x `mcs`/`mono` as build-time scaffolding and oracle only.

**Spec:** `docs/superpowers/specs/2026-10-07-hello-world-toolchain-design.md` (this plan implements stage 2, §7.2, and §5.3/§5.4/§6 config R)

## Global Constraints

- Language: Rust stable, `edition = "2024"`. `runtime` depends only on `metadata` (path dependency); `clr` only on `runtime`. No external crates.
- In a non-login shell run `source "$HOME/.cargo/env"` before `cargo`.
- Clean room: ECMA-335 6th edition is authoritative; MIT dotnet/runtime and Mono sources may be consulted. Never decompile any library or application. Dumping metadata of assemblies *we* compiled (with our `metadata` crate) is fine.
- Corlib identity: `mscorlib`, version `4.0.0.0`. Corlib source is C# in `corlib/`, compiled for now with `mcs -nostdlib -target:library -nowarn:626`, `corlib/AssemblyInfo.cs` listed **first** (mcs error CS1730 otherwise).
- Binding: `AssemblyRef` → loaded assembly by **simple name only**; version and public key token are ignored (spec §5.3).
- Laziness: TypeRefs/MemberRefs are resolved only when an executing instruction needs them. mcs-normal programs reference types our corlib lacks (e.g. `RuntimeCompatibilityAttribute`); they must still run.
- Internal calls: a MethodDef of `mscorlib` without an IL body (`Cell::Body(None)`, i.e. RVA 0) is an internal call, looked up by key `Namespace.Type::Method(paramtypes)`, e.g. `System.Console::WriteLine(string)`. mcs emits such methods with ImplFlags 0, so RVA 0 is the only marker.
- Spec §4 guardrails (verbatim intent): managed references are an opaque `ObjRef`, no raw pointers escape the heap module; the evaluation stack is typed; the managed call stack is an explicit `Vec<Frame>`, not Rust recursion; every object has a header pointing to its runtime type; strings use CLR layout (header, `length: i32`, UTF-16 code units); `ldstr` results are interned per (module, token).
- Stage 1 contract: `read_assembly` guarantees every `Cell::Ref` in a table is in range — **row 0 means null and must be checked first**; simple list columns (`FieldList`, `MethodList`, `ParamList`) may equal `count + 1`. Tokens found in **IL** are *not* validated by the reader: look them up with `LoadedAssembly::row`, which returns an error.
- Column positions come from `metadata::tables::col::*` (Task 1) — no bare `row[3]` in runtime code.
- Exit codes and messages (spec §5.4): void `Main` → 0; `int Main` → its return value; file missing / not a PE / bad metadata → `clr: <path>: <reason>`, exit 66; unsupported opcode, missing method, missing internal call → `clr: not implemented: <what> in <Namespace.Type::Method> @IL_xxxx` (4 lowercase hex digits), exit 70. Additionally: usage error → 64, output write failure → 74.
- No panics on malformed input (assemblies or IL). Internal invariants may `expect` with a message.
- Run `cargo fmt --all` before each commit. Every commit message ends with exactly:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- Tests that need `mcs`/`mono` are ignored **with a reason** when the tool is missing (never silently pass). CI fails on any ignored test; on Ubuntu `mono-devel` pulls in `mono-mcs`, which provides `mcs`.

## Decisions taken in this plan

- **MemberRef matching is structural.** Signatures are decoded and compared; `Class`/`ValueType` tokens are compared by (resolved assembly, namespace, name), so a `TypeRef` in the caller matches a `TypeDef` in the callee. Byte comparison would break as soon as a signature names a class.
- **`Main(string[] args)` receives a real `string[]`.** The heap gets a reference-array layout and the type system a synthesized `System.String[]` type; `clr` passes its trailing arguments.
- **`StackValue` has `I32`, `I64`, `F64`, `NativeInt`, `Obj`.** The spec's `ManagedPtr` is added with the first opcode that produces a managed pointer; an unconstructible placeholder variant would only be dead code.
- **Missing assembly → exit 66** (`cannot resolve assembly reference 'X'`); **missing type or method → exit 70** (`not implemented: type …` / `method …`), matching the spec's "missing method" row.
- **Recursion limit:** 10 000 managed frames, then exit 70 (`call depth above 10000 …`) instead of exhausting memory.

## Review Focus

1. **Malformed IL in a well-formed image** — truncated operands, call/ldstr tokens pointing at missing rows or the wrong table, stack underflow, `ret` with a dirty stack — must produce a clean error (66 or 70), never a panic. Pinned by `truncated_operand_is_invalid_program`, `call_to_missing_member_ref_is_a_load_error`, `ldstr_with_non_string_token_is_invalid_program`, `stack_underflow_is_invalid_program`, `ret_with_extra_values_is_invalid_program` (Task 6).
2. **Unbounded recursion** — `Main` calling itself must end with a 70 and a message, not an out-of-memory abort. Pinned by `unbounded_recursion_is_reported` (Task 6).
3. **Hostile inheritance chains** — a TypeDef whose base chain loops must give a load error, not a stack overflow in the type loader. Pinned by `cyclic_inheritance_is_a_load_error` (Task 3).
4. **Wrong object passed to an internal call** — IL can push a `string[]` where `WriteLine(string)` expects a string; the runtime must report it instead of reading the array as a string. Pinned by `write_line_rejects_non_string_objects` (Task 5).
5. **Output written before a failure** — a program that prints and then hits an unsupported opcode must still show the printed line on stdout. Pinned by the `unsupported_opcode` program (stdout `before\n`, Task 8).

---

## File Structure

```
Cargo.toml                                   # workspace: + crates/runtime, crates/clr
crates/metadata/src/tables.rs                # Task 1: + pub mod col
crates/runtime/
├── Cargo.toml
└── src/
    ├── lib.rs                               # modules, run(), run_files()
    ├── error.rs                             # RuntimeError, Location, exit codes
    ├── heap.rs                              # ObjRef, Heap (bump arena)
    ├── types.rs                             # TypeId, RuntimeType, TypeSystem
    ├── loader.rs                            # LoadedAssembly, Assemblies, text(), qualified()
    ├── methods.rs                           # MethodId, signatures, keys, MemberRef resolution
    ├── value.rs                             # StackValue
    ├── opcodes.rs                           # opcode constants + names
    ├── internal_calls.rs                    # Rust implementations of corlib externs
    ├── interp.rs                            # Interpreter, Frame
    └── testing.rs                           # #[cfg(test)] in-memory model builders
crates/clr/
├── Cargo.toml
├── build.rs                                 # detects mcs / mono
├── src/main.rs                              # CLI
└── tests/
    ├── cli.rs                               # Task 7: error exits, no mcs needed
    └── programs.rs                          # Task 8: config R harness
corlib/                                      # Task 8: C# source of mscorlib
├── README.md
├── AssemblyInfo.cs
├── System.cs
├── System.Collections.cs
├── System.Reflection.cs
└── System.Runtime.InteropServices.cs
tests/programs/                              # Task 8: *.cs + *.expected [+ .exitcode, .stderr]
```

---

### Task 1: Column constants in the `metadata` crate

**Files:**
- Modify: `crates/metadata/src/tables.rs` (add `pub mod col` after `columns()`, add one test)

**Interfaces:**
- Produces: `metadata::tables::col::{module, type_ref, type_def, method_def, member_ref, assembly, assembly_ref}::*` — `usize` column positions, e.g. `col::type_def::METHOD_LIST == 5`, `col::method_def::BODY == 0`, `col::assembly_ref::NAME == 6`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/metadata/src/tables.rs`:
```rust
    #[test]
    fn column_constants_match_schema() {
        use ColumnKind as K;
        let check = |t: TableId, c: usize, k: ColumnKind| {
            assert_eq!(columns(t)[c], k, "{t:?} column {c}");
        };
        check(TableId::Module, col::module::GENERATION, K::U16);
        check(TableId::Module, col::module::NAME, K::Str);
        check(TableId::Module, col::module::MVID, K::Guid);
        check(TableId::TypeRef, col::type_ref::RESOLUTION_SCOPE, K::Coded(CodedIndex::ResolutionScope));
        check(TableId::TypeRef, col::type_ref::NAME, K::Str);
        check(TableId::TypeRef, col::type_ref::NAMESPACE, K::Str);
        check(TableId::TypeDef, col::type_def::FLAGS, K::U32);
        check(TableId::TypeDef, col::type_def::NAME, K::Str);
        check(TableId::TypeDef, col::type_def::NAMESPACE, K::Str);
        check(TableId::TypeDef, col::type_def::EXTENDS, K::Coded(CodedIndex::TypeDefOrRef));
        check(TableId::TypeDef, col::type_def::FIELD_LIST, K::Table(TableId::Field));
        check(TableId::TypeDef, col::type_def::METHOD_LIST, K::Table(TableId::MethodDef));
        check(TableId::MethodDef, col::method_def::BODY, K::MethodRva);
        check(TableId::MethodDef, col::method_def::IMPL_FLAGS, K::U16);
        check(TableId::MethodDef, col::method_def::FLAGS, K::U16);
        check(TableId::MethodDef, col::method_def::NAME, K::Str);
        check(TableId::MethodDef, col::method_def::SIGNATURE, K::Blob);
        check(TableId::MethodDef, col::method_def::PARAM_LIST, K::Table(TableId::Param));
        check(TableId::MemberRef, col::member_ref::CLASS, K::Coded(CodedIndex::MemberRefParent));
        check(TableId::MemberRef, col::member_ref::NAME, K::Str);
        check(TableId::MemberRef, col::member_ref::SIGNATURE, K::Blob);
        check(TableId::Assembly, col::assembly::HASH_ALG_ID, K::U32);
        check(TableId::Assembly, col::assembly::MAJOR_VERSION, K::U16);
        check(TableId::Assembly, col::assembly::MINOR_VERSION, K::U16);
        check(TableId::Assembly, col::assembly::BUILD_NUMBER, K::U16);
        check(TableId::Assembly, col::assembly::REVISION_NUMBER, K::U16);
        check(TableId::Assembly, col::assembly::FLAGS, K::U32);
        check(TableId::Assembly, col::assembly::PUBLIC_KEY, K::Blob);
        check(TableId::Assembly, col::assembly::NAME, K::Str);
        check(TableId::Assembly, col::assembly::CULTURE, K::Str);
        check(TableId::AssemblyRef, col::assembly_ref::MAJOR_VERSION, K::U16);
        check(TableId::AssemblyRef, col::assembly_ref::MINOR_VERSION, K::U16);
        check(TableId::AssemblyRef, col::assembly_ref::BUILD_NUMBER, K::U16);
        check(TableId::AssemblyRef, col::assembly_ref::REVISION_NUMBER, K::U16);
        check(TableId::AssemblyRef, col::assembly_ref::FLAGS, K::U32);
        check(TableId::AssemblyRef, col::assembly_ref::PUBLIC_KEY_OR_TOKEN, K::Blob);
        check(TableId::AssemblyRef, col::assembly_ref::NAME, K::Str);
        check(TableId::AssemblyRef, col::assembly_ref::CULTURE, K::Str);
        check(TableId::AssemblyRef, col::assembly_ref::HASH_VALUE, K::Blob);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p metadata column_constants_match_schema`
Expected: compile error — `col` not found.

- [ ] **Step 3: Add the constants**

Insert directly after the `columns()` function in `crates/metadata/src/tables.rs`:
```rust
/// Column positions within rows, matching `columns()` (§II.22). Use these
/// instead of bare indexes.
pub mod col {
    /// §II.22.30
    pub mod module {
        pub const GENERATION: usize = 0;
        pub const NAME: usize = 1;
        pub const MVID: usize = 2;
    }

    /// §II.22.38
    pub mod type_ref {
        pub const RESOLUTION_SCOPE: usize = 0;
        pub const NAME: usize = 1;
        pub const NAMESPACE: usize = 2;
    }

    /// §II.22.37
    pub mod type_def {
        pub const FLAGS: usize = 0;
        pub const NAME: usize = 1;
        pub const NAMESPACE: usize = 2;
        pub const EXTENDS: usize = 3;
        pub const FIELD_LIST: usize = 4;
        pub const METHOD_LIST: usize = 5;
    }

    /// §II.22.26; `BODY` is the RVA column, decoded to a method body.
    pub mod method_def {
        pub const BODY: usize = 0;
        pub const IMPL_FLAGS: usize = 1;
        pub const FLAGS: usize = 2;
        pub const NAME: usize = 3;
        pub const SIGNATURE: usize = 4;
        pub const PARAM_LIST: usize = 5;
    }

    /// §II.22.25
    pub mod member_ref {
        pub const CLASS: usize = 0;
        pub const NAME: usize = 1;
        pub const SIGNATURE: usize = 2;
    }

    /// §II.22.2
    pub mod assembly {
        pub const HASH_ALG_ID: usize = 0;
        pub const MAJOR_VERSION: usize = 1;
        pub const MINOR_VERSION: usize = 2;
        pub const BUILD_NUMBER: usize = 3;
        pub const REVISION_NUMBER: usize = 4;
        pub const FLAGS: usize = 5;
        pub const PUBLIC_KEY: usize = 6;
        pub const NAME: usize = 7;
        pub const CULTURE: usize = 8;
    }

    /// §II.22.5
    pub mod assembly_ref {
        pub const MAJOR_VERSION: usize = 0;
        pub const MINOR_VERSION: usize = 1;
        pub const BUILD_NUMBER: usize = 2;
        pub const REVISION_NUMBER: usize = 3;
        pub const FLAGS: usize = 4;
        pub const PUBLIC_KEY_OR_TOKEN: usize = 5;
        pub const NAME: usize = 6;
        pub const CULTURE: usize = 7;
        pub const HASH_VALUE: usize = 8;
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p metadata`
Expected: all PASS, 0 ignored, no warnings.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/metadata/src/tables.rs
git commit -m "feat(metadata): named column positions for runtime consumers

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `runtime` crate scaffold — errors and heap

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Create: `crates/runtime/Cargo.toml`, `crates/runtime/src/lib.rs`, `crates/runtime/src/error.rs`, `crates/runtime/src/heap.rs`, `crates/runtime/src/types.rs`

**Interfaces:**
- Produces:
  - `runtime::error::{RuntimeError, Location, Result, EXIT_LOAD (66), EXIT_NOT_IMPLEMENTED (70), EXIT_IO (74)}`; `RuntimeError::{Load { path, reason }, NotImplemented { what, location }, InvalidProgram { what, location }, Io(String)}`; constructors `RuntimeError::load(path, reason)`, `not_implemented(what)`, `invalid_program(what)`; `at(self, Location) -> Self` (sets the location only if none); `exit_code(&self) -> i32`; `Display` per Global Constraints (without the `clr: ` prefix).
  - `runtime::types::TypeId(pub u32)` (`Copy, Eq, Hash, Debug`).
  - `runtime::heap::{ObjRef, Heap}`: `Heap::new()`, `bytes_allocated() -> usize`, `type_of(ObjRef) -> TypeId`, `alloc_string(TypeId, &[u16]) -> Result<ObjRef>`, `string_units(ObjRef) -> Vec<u16>`, `alloc_ref_array(TypeId, &[Option<ObjRef>]) -> Result<ObjRef>`, `ref_array_elements(ObjRef) -> Vec<Option<ObjRef>>`.

- [ ] **Step 1: Scaffold the crate**

`Cargo.toml` (workspace root) becomes:
```toml
[workspace]
resolver = "3"
members = ["crates/metadata", "crates/runtime"]
```

`crates/runtime/Cargo.toml`:
```toml
[package]
name = "runtime"
version = "0.1.0"
edition = "2024"
description = "Clean-room CLI runtime: loader, type system and IL interpreter"

[dependencies]
metadata = { path = "../metadata" }
```

`crates/runtime/src/lib.rs`:
```rust
//! Clean-room CLI runtime: loads assemblies with the `metadata` crate and
//! interprets their IL (spec §5.3). Section references such as §III.3.38
//! point into ECMA-335, 6th edition.

pub mod error;
pub mod heap;
pub mod types;

pub use error::{Location, Result, RuntimeError};
```

`crates/runtime/src/types.rs`:
```rust
//! Runtime type descriptors (spec §5.3 "Type system").

/// Index of a loaded type in the `TypeSystem`; stored in every object header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);
```

- [ ] **Step 2: Write the failing tests**

`crates/runtime/src/error.rs` — tests only for now:
```rust
//! Runtime errors and the process exit codes they map to (spec §5.4).

#[cfg(test)]
mod tests {
    use super::*;

    fn main_at(offset: u32) -> Location {
        Location { method: "HelloApp.Program::Main".into(), il_offset: offset }
    }

    #[test]
    fn display_matches_spec_format() {
        let e = RuntimeError::not_implemented("opcode 0x6F (callvirt)").at(main_at(5));
        assert_eq!(e.to_string(), "not implemented: opcode 0x6F (callvirt) in HelloApp.Program::Main @IL_0005");
        let e = RuntimeError::invalid_program("stack underflow").at(main_at(0x1a));
        assert_eq!(e.to_string(), "invalid program: stack underflow in HelloApp.Program::Main @IL_001a");
        let e = RuntimeError::load("app.exe", "bad image: missing MZ signature");
        assert_eq!(e.to_string(), "app.exe: bad image: missing MZ signature");
        assert_eq!(RuntimeError::not_implemented("x").to_string(), "not implemented: x");
        assert_eq!(RuntimeError::Io("broken pipe".into()).to_string(), "I/O error: broken pipe");
    }

    #[test]
    fn at_keeps_the_innermost_location() {
        let e = RuntimeError::not_implemented("x").at(main_at(1)).at(main_at(9));
        assert_eq!(e.to_string(), "not implemented: x in HelloApp.Program::Main @IL_0001");
        let load = RuntimeError::load("a", "b");
        assert_eq!(load.clone().at(main_at(1)), load);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(RuntimeError::load("a", "b").exit_code(), 66);
        assert_eq!(RuntimeError::not_implemented("x").exit_code(), 70);
        assert_eq!(RuntimeError::invalid_program("x").exit_code(), 70);
        assert_eq!(RuntimeError::Io("x".into()).exit_code(), 74);
    }
}
```

`crates/runtime/src/heap.rs` — tests only for now:
```rust
//! Bump-allocated managed heap (spec §5.3 "Heap"). Objects live in one
//! growable byte arena and are addressed by `ObjRef` offsets, so no raw
//! pointer escapes this module and a precise GC can replace the allocator.

#[cfg(test)]
mod tests {
    use super::*;

    const STRING: TypeId = TypeId(7);
    const ARRAY: TypeId = TypeId(9);

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn strings_round_trip() {
        let mut heap = Heap::new();
        for s in [utf16(""), utf16("Hello, World!"), utf16("Grüße 😀"), vec![0xD800]] {
            let obj = heap.alloc_string(STRING, &s).unwrap();
            assert_eq!(heap.type_of(obj), STRING);
            assert_eq!(heap.string_units(obj), s);
        }
    }

    #[test]
    fn objects_are_distinct_and_aligned() {
        let mut heap = Heap::new();
        assert_eq!(heap.bytes_allocated(), 0);
        let a = heap.alloc_string(STRING, &utf16("a")).unwrap();
        let after_a = heap.bytes_allocated();
        assert_eq!(after_a % 8, 0);
        let b = heap.alloc_string(STRING, &utf16("a")).unwrap();
        assert_ne!(a, b);
        assert_eq!(heap.bytes_allocated(), 2 * after_a);
        assert_eq!(heap.string_units(a), utf16("a"));
    }

    #[test]
    fn empty_string_is_header_plus_length() {
        let mut heap = Heap::new();
        heap.alloc_string(STRING, &[]).unwrap();
        assert_eq!(heap.bytes_allocated(), 8); // 4-byte type id + 4-byte length
    }

    #[test]
    fn reference_arrays_round_trip() {
        let mut heap = Heap::new();
        let s = heap.alloc_string(STRING, &utf16("x")).unwrap();
        let arr = heap.alloc_ref_array(ARRAY, &[Some(s), None, Some(s)]).unwrap();
        assert_eq!(heap.type_of(arr), ARRAY);
        assert_eq!(heap.ref_array_elements(arr), vec![Some(s), None, Some(s)]);
        let empty = heap.alloc_ref_array(ARRAY, &[]).unwrap();
        assert!(heap.ref_array_elements(empty).is_empty());
    }
}
```
Add `pub mod error; pub mod heap;` (already in lib.rs from Step 1).

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p runtime`
Expected: compile errors — `RuntimeError`, `Heap`, etc. not found.

- [ ] **Step 4: Implement `error.rs`**

Insert above the test module:
```rust
use std::fmt;

pub const EXIT_LOAD: i32 = 66;
pub const EXIT_NOT_IMPLEMENTED: i32 = 70;
pub const EXIT_IO: i32 = 74;

/// Where in managed code an error was raised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// `Namespace.Type::Method`
    pub method: String,
    pub il_offset: u32,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} @IL_{:04x}", self.method, self.il_offset)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    /// File missing, not a PE/CLI image, or inconsistent metadata.
    Load { path: String, reason: String },
    /// A feature this runtime does not implement yet.
    NotImplemented { what: String, location: Option<Location> },
    /// IL that breaks ECMA-335 rules, such as evaluation-stack underflow.
    InvalidProgram { what: String, location: Option<Location> },
    /// Writing program output failed.
    Io(String),
}

impl RuntimeError {
    pub fn load(path: impl Into<String>, reason: impl fmt::Display) -> Self {
        Self::Load { path: path.into(), reason: reason.to_string() }
    }

    pub fn not_implemented(what: impl Into<String>) -> Self {
        Self::NotImplemented { what: what.into(), location: None }
    }

    pub fn invalid_program(what: impl Into<String>) -> Self {
        Self::InvalidProgram { what: what.into(), location: None }
    }

    /// Attaches `here` unless the error already carries a location.
    pub fn at(self, here: Location) -> Self {
        match self {
            Self::NotImplemented { what, location: None } => {
                Self::NotImplemented { what, location: Some(here) }
            }
            Self::InvalidProgram { what, location: None } => {
                Self::InvalidProgram { what, location: Some(here) }
            }
            other => other,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Load { .. } => EXIT_LOAD,
            Self::NotImplemented { .. } | Self::InvalidProgram { .. } => EXIT_NOT_IMPLEMENTED,
            Self::Io(_) => EXIT_IO,
        }
    }
}

fn write_location(f: &mut fmt::Formatter<'_>, location: &Option<Location>) -> fmt::Result {
    match location {
        Some(l) => write!(f, " in {l}"),
        None => Ok(()),
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load { path, reason } => write!(f, "{path}: {reason}"),
            Self::NotImplemented { what, location } => {
                write!(f, "not implemented: {what}")?;
                write_location(f, location)
            }
            Self::InvalidProgram { what, location } => {
                write!(f, "invalid program: {what}")?;
                write_location(f, location)
            }
            Self::Io(message) => write!(f, "I/O error: {message}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

pub type Result<T> = std::result::Result<T, RuntimeError>;
```

- [ ] **Step 5: Implement `heap.rs`**

Insert above the test module:
```rust
use std::num::NonZeroU32;

use crate::error::{Result, RuntimeError};
use crate::types::TypeId;

const ALIGN: usize = 8;
/// Every object starts with the `TypeId` of its runtime type.
const HEADER_SIZE: usize = 4;
const LENGTH_SIZE: usize = 4;

/// Opaque reference to a heap object. Null is `Option::<ObjRef>::None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjRef(NonZeroU32);

impl ObjRef {
    fn offset(self) -> usize {
        self.0.get() as usize
    }
}

/// Bump allocator that never frees. Offset 0 is reserved so that every
/// `ObjRef` is non-zero.
pub struct Heap {
    arena: Vec<u8>,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    pub fn new() -> Self {
        Self { arena: vec![0; ALIGN] }
    }

    pub fn bytes_allocated(&self) -> usize {
        self.arena.len() - ALIGN
    }

    fn alloc(&mut self, ty: TypeId, payload: &[u8]) -> Result<ObjRef> {
        let offset = self.arena.len();
        let size = (HEADER_SIZE + payload.len()).div_ceil(ALIGN) * ALIGN;
        let end = offset
            .checked_add(size)
            .filter(|&end| end <= u32::MAX as usize)
            .ok_or_else(|| RuntimeError::not_implemented("managed heap larger than 4 GiB"))?;
        self.arena.extend_from_slice(&ty.0.to_le_bytes());
        self.arena.extend_from_slice(payload);
        self.arena.resize(end, 0);
        Ok(ObjRef(NonZeroU32::new(offset as u32).expect("offset 0 is reserved")))
    }

    fn u32_at(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.arena[at..at + 4].try_into().expect("4-byte slice"))
    }

    pub fn type_of(&self, obj: ObjRef) -> TypeId {
        TypeId(self.u32_at(obj.offset()))
    }

    fn length_prefix(len: usize) -> Result<[u8; 4]> {
        i32::try_from(len)
            .map(i32::to_le_bytes)
            .map_err(|_| RuntimeError::not_implemented("objects with more than 2^31 elements"))
    }

    /// CLR string layout: header, `length: i32`, UTF-16 code units.
    pub fn alloc_string(&mut self, ty: TypeId, units: &[u16]) -> Result<ObjRef> {
        let mut payload = Vec::with_capacity(LENGTH_SIZE + units.len() * 2);
        payload.extend_from_slice(&Self::length_prefix(units.len())?);
        for unit in units {
            payload.extend_from_slice(&unit.to_le_bytes());
        }
        self.alloc(ty, &payload)
    }

    pub fn string_units(&self, s: ObjRef) -> Vec<u16> {
        let at = s.offset() + HEADER_SIZE;
        let len = self.u32_at(at) as usize;
        let start = at + LENGTH_SIZE;
        self.arena[start..start + len * 2]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    /// Single-dimensional, zero-based array of references: header,
    /// `length: i32`, one 4-byte `ObjRef` per element (0 = null).
    pub fn alloc_ref_array(&mut self, ty: TypeId, elements: &[Option<ObjRef>]) -> Result<ObjRef> {
        let mut payload = Vec::with_capacity(LENGTH_SIZE + elements.len() * 4);
        payload.extend_from_slice(&Self::length_prefix(elements.len())?);
        for element in elements {
            let raw = element.map_or(0, |o| o.0.get());
            payload.extend_from_slice(&raw.to_le_bytes());
        }
        self.alloc(ty, &payload)
    }

    pub fn ref_array_elements(&self, array: ObjRef) -> Vec<Option<ObjRef>> {
        let at = array.offset() + HEADER_SIZE;
        let len = self.u32_at(at) as usize;
        (0..len)
            .map(|i| NonZeroU32::new(self.u32_at(at + LENGTH_SIZE + i * 4)).map(ObjRef))
            .collect()
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p runtime`
Expected: 7 tests PASS, no warnings.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add Cargo.toml Cargo.lock crates/runtime
git commit -m "feat(runtime): crate scaffold with errors and bump heap

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Loader and type system

**Files:**
- Create: `crates/runtime/src/loader.rs`, `crates/runtime/src/testing.rs`
- Modify: `crates/runtime/src/types.rs`, `crates/runtime/src/lib.rs`

**Interfaces:**
- Consumes: `RuntimeError`, `Result`, `TypeId` (Task 2); `metadata::{read_assembly, MetadataModel, Row, Cell, TableId}`, `metadata::tables::col` (Task 1).
- Produces:
  - `loader::CORLIB_NAME = "mscorlib"`; `loader::text(&Row, usize) -> &str`; `loader::qualified(namespace, name) -> String`.
  - `loader::AssemblyId(pub usize)` (`Copy, Eq, Hash, Debug`).
  - `loader::LoadedAssembly { pub name: String, pub path: String, pub model: MetadataModel }`: `from_file(&Path) -> Result<Self>`, `from_model(path, MetadataModel) -> Result<Self>`, `bad_image(reason) -> RuntimeError`, `row(TableId, u32) -> Result<&Row>`.
  - `loader::Assemblies`: `new(corlib: LoadedAssembly) -> Result<Self>` (corlib must be named `mscorlib`), `add(LoadedAssembly) -> AssemblyId`, `get(AssemblyId) -> &LoadedAssembly`, `corlib() -> AssemblyId`, `resolve_assembly_ref(from, row) -> Result<AssemblyId>`, `find_type_def(asm, namespace, name) -> Option<u32>`, `type_ref_identity(asm, row) -> Result<(AssemblyId, String, String)>`, `resolve_type_ref(asm, row) -> Result<(AssemblyId, u32)>`.
  - `types::RuntimeType { pub namespace, pub name, pub def: Option<(AssemblyId, u32)>, pub base: Option<TypeId>, pub element: Option<TypeId> }` with `full_name()`.
  - `types::TypeSystem`: `new()`, `get(TypeId) -> &RuntimeType`, `len()`, `is_empty()`, `load_type_def(&Assemblies, AssemblyId, u32) -> Result<TypeId>`, `resolve_type_token(&Assemblies, AssemblyId, TableId, u32) -> Result<TypeId>`, `corlib_type(&Assemblies, namespace, name) -> Result<TypeId>`, `szarray(element: TypeId, array_base: TypeId) -> TypeId`.
  - `testing` (test-only): `ModelBuilder { pub model }` with `new(name)`, `type_def(ns, name, extends: Cell) -> u32`, `method(name, signature, code: Option<&[u8]>) -> u32`, `assembly_ref(name) -> u32`, `type_ref(assembly_ref, ns, name) -> u32`, `member_ref(parent: Cell, name, signature) -> u32`, `user_string(&str) -> u32`, `entry_point(method_row)`, `build() -> LoadedAssembly`; `corlib_builder()`, `test_corlib()`, `app()`; constants `PUBLIC_STATIC`, `VOID_NO_ARGS`, `INT_NO_ARGS`, `VOID_STRING`, `WRITE_LINE`.

- [ ] **Step 1: Add the test model builder**

`crates/runtime/src/testing.rs`:
```rust
//! In-memory metadata models for unit tests. Rows follow the column layout
//! of `metadata::tables::columns()`.
#![allow(dead_code)] // shared helpers; not every test module uses every item

use metadata::body::MethodBody;
use metadata::{Cell, MetadataModel, TableId};

use crate::loader::LoadedAssembly;

/// `Public | Static | HideBySig`
pub const PUBLIC_STATIC: u16 = 0x0096;
pub const VOID_NO_ARGS: &[u8] = &[0x00, 0x00, 0x01];
pub const INT_NO_ARGS: &[u8] = &[0x00, 0x00, 0x08];
pub const VOID_STRING: &[u8] = &[0x00, 0x01, 0x01, 0x0E];
/// MemberRef 1 of `app()`: `System.Console::WriteLine(string)`.
pub const WRITE_LINE: u32 = 0x0A00_0001;

pub struct ModelBuilder {
    name: String,
    pub model: MetadataModel,
}

impl ModelBuilder {
    /// A module and assembly named `name`, version 4.0.0.0, with `<Module>` as TypeDef 1.
    pub fn new(name: &str) -> Self {
        let mut model = MetadataModel::new();
        model.push_row(
            TableId::Module,
            vec![
                Cell::U16(0),
                Cell::Str(format!("{name}.dll")),
                Cell::Guid(Some([1; 16])),
                Cell::Guid(None),
                Cell::Guid(None),
            ],
        );
        model.push_row(
            TableId::Assembly,
            vec![
                Cell::U32(0x8004),
                Cell::U16(4),
                Cell::U16(0),
                Cell::U16(0),
                Cell::U16(0),
                Cell::U32(0),
                Cell::Blob(Vec::new()),
                Cell::Str(name.to_owned()),
                Cell::Str(String::new()),
            ],
        );
        let mut builder = Self { name: name.to_owned(), model };
        builder.type_def("", "<Module>", Cell::Ref(TableId::TypeDef, 0));
        builder
    }

    /// Appends a type; methods added afterwards belong to it until the next type.
    pub fn type_def(&mut self, namespace: &str, name: &str, extends: Cell) -> u32 {
        let field_list = self.model.table(TableId::Field).len() as u32 + 1;
        let method_list = self.model.table(TableId::MethodDef).len() as u32 + 1;
        self.model.push_row(
            TableId::TypeDef,
            vec![
                Cell::U32(0),
                Cell::Str(name.to_owned()),
                Cell::Str(namespace.to_owned()),
                extends,
                Cell::Ref(TableId::Field, field_list),
                Cell::Ref(TableId::MethodDef, method_list),
            ],
        )
    }

    /// Appends a public static method; `code: None` makes it body-less.
    pub fn method(&mut self, name: &str, signature: &[u8], code: Option<&[u8]>) -> u32 {
        let param_list = self.model.table(TableId::Param).len() as u32 + 1;
        let body = code.map(|code| MethodBody {
            max_stack: 8,
            init_locals: false,
            local_var_sig_token: 0,
            code: code.to_vec(),
            extra_sections: Vec::new(),
        });
        self.model.push_row(
            TableId::MethodDef,
            vec![
                Cell::Body(body),
                Cell::U16(0),
                Cell::U16(PUBLIC_STATIC),
                Cell::Str(name.to_owned()),
                Cell::Blob(signature.to_vec()),
                Cell::Ref(TableId::Param, param_list),
            ],
        )
    }

    pub fn assembly_ref(&mut self, name: &str) -> u32 {
        self.model.push_row(
            TableId::AssemblyRef,
            vec![
                Cell::U16(4),
                Cell::U16(0),
                Cell::U16(0),
                Cell::U16(0),
                Cell::U32(0),
                Cell::Blob(Vec::new()),
                Cell::Str(name.to_owned()),
                Cell::Str(String::new()),
                Cell::Blob(Vec::new()),
            ],
        )
    }

    pub fn type_ref(&mut self, assembly_ref: u32, namespace: &str, name: &str) -> u32 {
        self.model.push_row(
            TableId::TypeRef,
            vec![
                Cell::Ref(TableId::AssemblyRef, assembly_ref),
                Cell::Str(name.to_owned()),
                Cell::Str(namespace.to_owned()),
            ],
        )
    }

    pub fn member_ref(&mut self, parent: Cell, name: &str, signature: &[u8]) -> u32 {
        self.model.push_row(
            TableId::MemberRef,
            vec![parent, Cell::Str(name.to_owned()), Cell::Blob(signature.to_vec())],
        )
    }

    /// Returns the `ldstr` token.
    pub fn user_string(&mut self, s: &str) -> u32 {
        self.model.add_user_string(s).expect("small #US heap")
    }

    pub fn entry_point(&mut self, method_row: u32) {
        self.model.entry_point_token = TableId::MethodDef.token(method_row);
    }

    pub fn build(self) -> LoadedAssembly {
        LoadedAssembly::from_model(format!("<{}>", self.name), self.model)
            .expect("test models have an Assembly row")
    }
}

/// mscorlib with System.Object (TypeDef 2), System.String (3), System.Array (4)
/// and System.Console (5) declaring body-less `WriteLine(string)` (MethodDef 1).
/// Console is the last type, so tests may add more Console methods.
pub fn corlib_builder() -> ModelBuilder {
    let mut b = ModelBuilder::new("mscorlib");
    let object = b.type_def("System", "Object", Cell::Ref(TableId::TypeDef, 0));
    b.type_def("System", "String", Cell::Ref(TableId::TypeDef, object));
    b.type_def("System", "Array", Cell::Ref(TableId::TypeDef, object));
    b.type_def("System", "Console", Cell::Ref(TableId::TypeDef, object));
    b.method("WriteLine", VOID_STRING, None);
    b
}

pub fn test_corlib() -> LoadedAssembly {
    corlib_builder().build()
}

/// Entry assembly `app`: AssemblyRef 1 = mscorlib, TypeRef 1 = System.Object,
/// TypeRef 2 = System.Console, MemberRef 1 = Console::WriteLine(string)
/// (`WRITE_LINE`), TypeDef 2 = `App` extending Object. Methods added
/// afterwards belong to `App`.
pub fn app() -> ModelBuilder {
    let mut b = ModelBuilder::new("app");
    let mscorlib = b.assembly_ref("mscorlib");
    let object = b.type_ref(mscorlib, "System", "Object");
    let console = b.type_ref(mscorlib, "System", "Console");
    b.member_ref(Cell::Ref(TableId::TypeRef, console), "WriteLine", VOID_STRING);
    b.type_def("", "App", Cell::Ref(TableId::TypeRef, object));
    b
}
```

Update `crates/runtime/src/lib.rs` to:
```rust
//! Clean-room CLI runtime: loads assemblies with the `metadata` crate and
//! interprets their IL (spec §5.3). Section references such as §III.3.38
//! point into ECMA-335, 6th edition.

pub mod error;
pub mod heap;
pub mod loader;
pub mod types;

#[cfg(test)]
mod testing;

pub use error::{Location, Result, RuntimeError};
pub use loader::{Assemblies, AssemblyId, LoadedAssembly};
```

- [ ] **Step 2: Write the failing tests**

`crates/runtime/src/loader.rs` — tests only for now:
```rust
//! Assembly loading and name-based binding (spec §5.3 "Loader").

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, test_corlib};
    use metadata::Cell;

    #[test]
    fn loads_an_mcs_fixture_from_disk() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../metadata/tests/fixtures/hello.exe");
        let asm = LoadedAssembly::from_file(&path).unwrap();
        assert_eq!(asm.name, "hello");
        assert_eq!(asm.path, path.display().to_string());
    }

    #[test]
    fn missing_file_is_a_load_error_naming_the_path() {
        let err = LoadedAssembly::from_file(Path::new("/nonexistent/app.exe")).unwrap_err();
        assert!(matches!(&err, RuntimeError::Load { path, .. } if path == "/nonexistent/app.exe"), "{err:?}");
        assert_eq!(err.exit_code(), 66);
    }

    #[test]
    fn non_pe_file_is_a_load_error() {
        let path = std::env::temp_dir().join(format!("runtime-loader-test-{}.exe", std::process::id()));
        std::fs::write(&path, b"not a PE file").unwrap();
        let err = LoadedAssembly::from_file(&path).unwrap_err();
        std::fs::remove_file(&path).unwrap();
        assert!(err.to_string().contains("bad image"), "{err}");
    }

    #[test]
    fn corlib_must_be_mscorlib() {
        let err = Assemblies::new(app().build()).err().expect("app is not mscorlib");
        assert!(err.to_string().contains("expected assembly 'mscorlib', found 'app'"), "{err}");
    }

    #[test]
    fn assembly_refs_bind_by_name_only() {
        let mut entry = app();
        // Mono's mscorlib reference: version 4.0.0.0 plus a public key token.
        entry.model.table_mut(TableId::AssemblyRef)[0][col::assembly_ref::MAJOR_VERSION] = Cell::U16(9);
        entry.model.table_mut(TableId::AssemblyRef)[0][col::assembly_ref::PUBLIC_KEY_OR_TOKEN] =
            Cell::Blob(vec![0xb7, 0x7a, 0x5c, 0x56, 0x19, 0x34, 0xe0, 0x89]);
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(entry.build());
        assert_eq!(asms.resolve_assembly_ref(app, 1).unwrap(), asms.corlib());
    }

    #[test]
    fn unknown_assembly_ref_is_a_load_error() {
        let mut entry = app();
        let other = entry.assembly_ref("System.Core");
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(entry.build());
        let err = asms.resolve_assembly_ref(app, other).unwrap_err();
        assert!(err.to_string().contains("cannot resolve assembly reference 'System.Core'"), "{err}");
        assert_eq!(err.exit_code(), 66);
    }

    #[test]
    fn type_refs_resolve_to_corlib_type_defs() {
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(app().build());
        assert_eq!(asms.resolve_type_ref(app, 2).unwrap(), (asms.corlib(), 5)); // System.Console
        assert_eq!(asms.find_type_def(asms.corlib(), "System", "String"), Some(3));
        assert_eq!(asms.find_type_def(asms.corlib(), "System", "Nope"), None);
    }

    #[test]
    fn missing_type_is_not_implemented() {
        let mut entry = app();
        let attr = entry.type_ref(1, "System.Runtime.CompilerServices", "RuntimeCompatibilityAttribute");
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(entry.build());
        let identity = asms.type_ref_identity(app, attr).unwrap();
        assert_eq!(identity.0, asms.corlib(), "identity does not need the type to exist");
        let err = asms.resolve_type_ref(app, attr).unwrap_err();
        assert_eq!(
            err.to_string(),
            "not implemented: type System.Runtime.CompilerServices.RuntimeCompatibilityAttribute in assembly 'mscorlib'"
        );
    }

    #[test]
    fn row_lookup_is_checked() {
        let corlib = test_corlib();
        assert!(corlib.row(TableId::TypeDef, 1).is_ok());
        assert!(corlib.row(TableId::TypeDef, 0).is_err());
        assert!(corlib.row(TableId::TypeDef, 99).is_err());
        assert!(corlib.row(TableId::MemberRef, 1).is_err());
    }
}
```

Append to `crates/runtime/src/types.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Assemblies;
    use crate::testing::{app, corlib_builder, test_corlib};
    use metadata::Cell;

    #[test]
    fn loads_types_with_their_base_chain_once() {
        let asms = Assemblies::new(test_corlib()).unwrap();
        let mut types = TypeSystem::new();
        let string = types.load_type_def(&asms, asms.corlib(), 3).unwrap();
        assert_eq!(types.get(string).full_name(), "System.String");
        let object = types.get(string).base.expect("String extends Object");
        assert_eq!(types.get(object).full_name(), "System.Object");
        assert_eq!(types.get(object).base, None);
        assert_eq!(types.len(), 2);
        assert_eq!(types.load_type_def(&asms, asms.corlib(), 3).unwrap(), string);
        assert_eq!(types.len(), 2, "cached");
    }

    #[test]
    fn resolves_type_refs_across_assemblies() {
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app_id = asms.add(app().build());
        let mut types = TypeSystem::new();
        let app_type = types.load_type_def(&asms, app_id, 2).unwrap();
        let base = types.get(app_type).base.unwrap();
        assert_eq!(types.get(base).def, Some((asms.corlib(), 2)));
        let console = types.resolve_type_token(&asms, app_id, TableId::TypeRef, 2).unwrap();
        assert_eq!(types.get(console).full_name(), "System.Console");
    }

    #[test]
    fn cyclic_inheritance_is_a_load_error() {
        let mut b = corlib_builder();
        // TypeDef 6 extends 7 and 7 extends 6.
        b.type_def("Loop", "A", Cell::Ref(TableId::TypeDef, 7));
        b.type_def("Loop", "B", Cell::Ref(TableId::TypeDef, 6));
        let asms = Assemblies::new(b.build()).unwrap();
        let err = TypeSystem::new().load_type_def(&asms, asms.corlib(), 6).unwrap_err();
        assert!(err.to_string().contains("cyclic"), "{err}");
        assert_eq!(err.exit_code(), 66);
    }

    #[test]
    fn corlib_types_by_name() {
        let asms = Assemblies::new(test_corlib()).unwrap();
        let mut types = TypeSystem::new();
        let array = types.corlib_type(&asms, "System", "Array").unwrap();
        assert_eq!(types.get(array).def, Some((asms.corlib(), 4)));
        let err = types.corlib_type(&asms, "System", "Int32").unwrap_err();
        assert!(err.to_string().contains("corlib does not define System.Int32"), "{err}");
    }

    #[test]
    fn szarray_types_are_synthesized_once() {
        let asms = Assemblies::new(test_corlib()).unwrap();
        let mut types = TypeSystem::new();
        let string = types.corlib_type(&asms, "System", "String").unwrap();
        let array = types.corlib_type(&asms, "System", "Array").unwrap();
        let strings = types.szarray(string, array);
        assert_eq!(types.get(strings).full_name(), "System.String[]");
        assert_eq!(types.get(strings).element, Some(string));
        assert_eq!(types.get(strings).base, Some(array));
        assert_eq!(types.get(strings).def, None);
        assert_eq!(types.szarray(string, array), strings);
    }

    #[test]
    fn type_spec_references_are_not_implemented() {
        let asms = Assemblies::new(test_corlib()).unwrap();
        let err = TypeSystem::new()
            .resolve_type_token(&asms, asms.corlib(), TableId::TypeSpec, 1)
            .unwrap_err();
        assert_eq!(err.exit_code(), 70);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p runtime`
Expected: compile errors — `LoadedAssembly`, `Assemblies`, `TypeSystem` not found.

- [ ] **Step 4: Implement `loader.rs`**

Insert above the test module:
```rust
use std::fmt;
use std::path::Path;

use metadata::tables::col;
use metadata::{MetadataModel, Row, TableId, read_assembly};

use crate::error::{Result, RuntimeError};

pub const CORLIB_NAME: &str = "mscorlib";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssemblyId(pub usize);

/// Text of a #Strings column. Rows produced by `read_assembly` always hold a
/// `Cell::Str` in such columns.
pub fn text(row: &Row, column: usize) -> &str {
    row[column].as_str().unwrap_or("")
}

/// `Namespace.Name`, or just `Name` in the global namespace.
pub fn qualified(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_owned()
    } else {
        format!("{namespace}.{name}")
    }
}

#[derive(Debug)]
pub struct LoadedAssembly {
    /// Simple name from the Assembly table, e.g. `mscorlib`.
    pub name: String,
    /// Where it came from; prefixes load errors.
    pub path: String,
    pub model: MetadataModel,
}

impl LoadedAssembly {
    pub fn from_file(path: &Path) -> Result<Self> {
        let shown = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|e| RuntimeError::load(&shown, e))?;
        let model = read_assembly(&bytes).map_err(|e| RuntimeError::load(&shown, e))?;
        Self::from_model(shown, model)
    }

    pub fn from_model(path: impl Into<String>, model: MetadataModel) -> Result<Self> {
        let path = path.into();
        let Some(assembly) = model.table(TableId::Assembly).first() else {
            return Err(RuntimeError::load(path, "not an assembly: the Assembly table is empty"));
        };
        let name = text(assembly, col::assembly::NAME).to_owned();
        Ok(Self { name, path, model })
    }

    pub fn bad_image(&self, reason: impl fmt::Display) -> RuntimeError {
        RuntimeError::load(&self.path, reason)
    }

    /// Row `row` (1-based) of `table`, or a load error if it does not exist.
    /// Use this for tokens taken from IL, which the reader does not validate.
    pub fn row(&self, table: TableId, row: u32) -> Result<&Row> {
        row.checked_sub(1)
            .and_then(|i| self.model.table(table).get(i as usize))
            .ok_or_else(|| self.bad_image(format!("{table:?} row {row} does not exist")))
    }
}

/// All loaded assemblies; the corlib is always `AssemblyId(0)`.
pub struct Assemblies {
    list: Vec<LoadedAssembly>,
}

impl Assemblies {
    pub fn new(corlib: LoadedAssembly) -> Result<Self> {
        if corlib.name != CORLIB_NAME {
            return Err(corlib.bad_image(format!(
                "expected assembly '{CORLIB_NAME}', found '{}'",
                corlib.name
            )));
        }
        Ok(Self { list: vec![corlib] })
    }

    pub fn add(&mut self, assembly: LoadedAssembly) -> AssemblyId {
        self.list.push(assembly);
        AssemblyId(self.list.len() - 1)
    }

    pub fn get(&self, id: AssemblyId) -> &LoadedAssembly {
        &self.list[id.0]
    }

    pub fn corlib(&self) -> AssemblyId {
        AssemblyId(0)
    }

    /// Binds AssemblyRef `row` of `from` by simple name; version and public
    /// key token are ignored (spec §5.3).
    pub fn resolve_assembly_ref(&self, from: AssemblyId, row: u32) -> Result<AssemblyId> {
        let assembly = self.get(from);
        let name = text(assembly.row(TableId::AssemblyRef, row)?, col::assembly_ref::NAME);
        self.list
            .iter()
            .position(|a| a.name == name)
            .map(AssemblyId)
            .ok_or_else(|| assembly.bad_image(format!("cannot resolve assembly reference '{name}'")))
    }

    pub fn find_type_def(&self, asm: AssemblyId, namespace: &str, name: &str) -> Option<u32> {
        self.get(asm)
            .model
            .table(TableId::TypeDef)
            .iter()
            .position(|r| {
                text(r, col::type_def::NAME) == name && text(r, col::type_def::NAMESPACE) == namespace
            })
            .map(|i| i as u32 + 1)
    }

    /// The (assembly, namespace, name) TypeRef `row` of `asm` denotes. Does
    /// not require the type to exist.
    pub fn type_ref_identity(&self, asm: AssemblyId, row: u32) -> Result<(AssemblyId, String, String)> {
        let r = self.get(asm).row(TableId::TypeRef, row)?;
        let namespace = text(r, col::type_ref::NAMESPACE).to_owned();
        let name = text(r, col::type_ref::NAME).to_owned();
        let target = match r[col::type_ref::RESOLUTION_SCOPE].as_index() {
            Some((TableId::AssemblyRef, scope)) if scope != 0 => self.resolve_assembly_ref(asm, scope)?,
            Some((TableId::Module, _)) => asm,
            Some((TableId::TypeRef, _)) => {
                return Err(RuntimeError::not_implemented(format!(
                    "nested type reference {}",
                    qualified(&namespace, &name)
                )));
            }
            _ => {
                return Err(RuntimeError::not_implemented(format!(
                    "resolution scope of type reference {}",
                    qualified(&namespace, &name)
                )));
            }
        };
        Ok((target, namespace, name))
    }

    /// Resolves TypeRef `row` of `asm` to its defining assembly and TypeDef
    /// row without loading the type.
    pub fn resolve_type_ref(&self, asm: AssemblyId, row: u32) -> Result<(AssemblyId, u32)> {
        let (target, namespace, name) = self.type_ref_identity(asm, row)?;
        let def = self.find_type_def(target, &namespace, &name).ok_or_else(|| {
            RuntimeError::not_implemented(format!(
                "type {} in assembly '{}'",
                qualified(&namespace, &name),
                self.get(target).name
            ))
        })?;
        Ok((target, def))
    }
}
```

- [ ] **Step 5: Implement the `TypeSystem`**

Insert in `crates/runtime/src/types.rs` between `TypeId` and the test module:
```rust
use std::collections::HashMap;

use metadata::TableId;
use metadata::tables::col;

use crate::error::{Result, RuntimeError};
use crate::loader::{Assemblies, AssemblyId, qualified, text};

/// Base-type chains longer than this are treated as cyclic metadata.
const MAX_INHERITANCE_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeType {
    pub namespace: String,
    pub name: String,
    /// Defining assembly and TypeDef row; `None` for synthesized array types.
    pub def: Option<(AssemblyId, u32)>,
    pub base: Option<TypeId>,
    /// Element type of a single-dimensional, zero-based array type.
    pub element: Option<TypeId>,
}

impl RuntimeType {
    pub fn full_name(&self) -> String {
        qualified(&self.namespace, &self.name)
    }
}

/// Types are loaded lazily, on first use, and cached.
#[derive(Default)]
pub struct TypeSystem {
    types: Vec<RuntimeType>,
    by_def: HashMap<(AssemblyId, u32), TypeId>,
    szarrays: HashMap<TypeId, TypeId>,
}

impl TypeSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: TypeId) -> &RuntimeType {
        &self.types[id.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    fn push(&mut self, t: RuntimeType) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        self.types.push(t);
        id
    }

    /// Loads TypeDef `row` of `asm` and its base chain.
    pub fn load_type_def(&mut self, asms: &Assemblies, asm: AssemblyId, row: u32) -> Result<TypeId> {
        self.load_at_depth(asms, asm, row, 0)
    }

    /// Resolves a TypeDef or TypeRef reference that appears in `asm`.
    pub fn resolve_type_token(
        &mut self,
        asms: &Assemblies,
        asm: AssemblyId,
        table: TableId,
        row: u32,
    ) -> Result<TypeId> {
        self.resolve_at_depth(asms, asm, table, row, 0)
    }

    fn resolve_at_depth(
        &mut self,
        asms: &Assemblies,
        asm: AssemblyId,
        table: TableId,
        row: u32,
        depth: usize,
    ) -> Result<TypeId> {
        match table {
            TableId::TypeDef => self.load_at_depth(asms, asm, row, depth),
            TableId::TypeRef => {
                let (target, target_row) = asms.resolve_type_ref(asm, row)?;
                self.load_at_depth(asms, target, target_row, depth)
            }
            other => Err(RuntimeError::not_implemented(format!("{other:?} type references"))),
        }
    }

    fn load_at_depth(&mut self, asms: &Assemblies, asm: AssemblyId, row: u32, depth: usize) -> Result<TypeId> {
        if let Some(&id) = self.by_def.get(&(asm, row)) {
            return Ok(id);
        }
        let assembly = asms.get(asm);
        if depth > MAX_INHERITANCE_DEPTH {
            return Err(assembly.bad_image(format!(
                "base-type chain at TypeDef row {row} is cyclic or deeper than {MAX_INHERITANCE_DEPTH}"
            )));
        }
        let def = assembly.row(TableId::TypeDef, row)?;
        let namespace = text(def, col::type_def::NAMESPACE).to_owned();
        let name = text(def, col::type_def::NAME).to_owned();
        let base = match def[col::type_def::EXTENDS].as_index() {
            Some((_, 0)) | None => None,
            Some((table, base_row)) => Some(self.resolve_at_depth(asms, asm, table, base_row, depth + 1)?),
        };
        let id = self.push(RuntimeType { namespace, name, def: Some((asm, row)), base, element: None });
        self.by_def.insert((asm, row), id);
        Ok(id)
    }

    /// A well-known corlib type such as `System.String`.
    pub fn corlib_type(&mut self, asms: &Assemblies, namespace: &str, name: &str) -> Result<TypeId> {
        let corlib = asms.corlib();
        let row = asms.find_type_def(corlib, namespace, name).ok_or_else(|| {
            asms.get(corlib).bad_image(format!("corlib does not define {}", qualified(namespace, name)))
        })?;
        self.load_type_def(asms, corlib, row)
    }

    /// The single-dimensional, zero-based array type `element[]`.
    pub fn szarray(&mut self, element: TypeId, array_base: TypeId) -> TypeId {
        if let Some(&id) = self.szarrays.get(&element) {
            return id;
        }
        let e = self.get(element);
        let array = RuntimeType {
            namespace: e.namespace.clone(),
            name: format!("{}[]", e.name),
            def: None,
            base: Some(array_base),
            element: Some(element),
        };
        let id = self.push(array);
        self.szarrays.insert(element, id);
        id
    }
}
```
Note the cyclic test: TypeDef 6 extends 7 extends 6 — `load_at_depth` recurses with `depth + 1` each step and stops with the "cyclic" error at depth 65, before caching either type.

- [ ] **Step 6: Run tests**

Run: `cargo test -p runtime`
Expected: all PASS (7 from Task 2 + 9 loader + 6 types = 22), no warnings.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/runtime
git commit -m "feat(runtime): assembly loader and lazy type system

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Method resolution and signature matching

**Files:**
- Create: `crates/runtime/src/methods.rs`
- Modify: `crates/runtime/src/lib.rs` (add `pub mod methods;`)

**Interfaces:**
- Consumes: `Assemblies`, `LoadedAssembly`, `AssemblyId`, `text`, `qualified` (Task 3); `metadata::sig::{decode_method_sig, MethodSig, TypeSig, TypeDefOrRef, conv}`.
- Produces:
  - `methods::MethodId { pub asm: AssemblyId, pub row: u32 }` (`Copy, Eq, Hash, Debug`).
  - `owner_type_row(&MetadataModel, method_row: u32) -> u32` (0 if no type owns it).
  - `method_sig(&Assemblies, MethodId) -> Result<MethodSig>`.
  - `method_name(&Assemblies, MethodId) -> Result<String>` — `Namespace.Type::Method`.
  - `method_key(&Assemblies, MethodId) -> Result<String>` — `Namespace.Type::Method(p1,p2)` with C# keywords for primitives, e.g. `System.Console::WriteLine(string)`.
  - `resolve_member_ref(&Assemblies, AssemblyId, row: u32) -> Result<MethodId>`; failure message `method <Type>::<Name>(<params>)` (NotImplemented).

- [ ] **Step 1: Write the failing tests**

`crates/runtime/src/methods.rs` — tests only for now:
```rust
//! Methods: ownership, signatures, internal-call keys and MemberRef →
//! MethodDef resolution by name and signature (spec §5.3 "Type system").

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{VOID_STRING, app, corlib_builder, test_corlib};
    use metadata::{Cell, TableId};

    #[test]
    fn owner_type_skips_types_without_methods() {
        let corlib = test_corlib();
        // Object, String and Array declare no methods; Console owns MethodDef 1.
        assert_eq!(owner_type_row(&corlib.model, 1), 5);
    }

    #[test]
    fn names_and_internal_call_keys() {
        let asms = Assemblies::new(test_corlib()).unwrap();
        let write_line = MethodId { asm: asms.corlib(), row: 1 };
        assert_eq!(method_name(&asms, write_line).unwrap(), "System.Console::WriteLine");
        assert_eq!(method_key(&asms, write_line).unwrap(), "System.Console::WriteLine(string)");
    }

    #[test]
    fn keys_render_parameter_types() {
        let mut b = corlib_builder();
        // (int, object, string[], System.String) — the last as Class(TypeDef 3).
        b.method("Mixed", &[0x00, 0x04, 0x01, 0x08, 0x1C, 0x1D, 0x0E, 0x12, 0x0C], None);
        let asms = Assemblies::new(b.build()).unwrap();
        let key = method_key(&asms, MethodId { asm: asms.corlib(), row: 2 }).unwrap();
        assert_eq!(key, "System.Console::Mixed(int,object,string[],System.String)");
    }

    #[test]
    fn member_ref_resolves_to_corlib_method_def() {
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(app().build());
        let target = resolve_member_ref(&asms, app, 1).unwrap();
        assert_eq!(target, MethodId { asm: asms.corlib(), row: 1 });
    }

    #[test]
    fn overloads_are_chosen_by_signature() {
        let mut corlib = corlib_builder();
        corlib.method("WriteLine", &[0x00, 0x01, 0x01, 0x1C], None); // WriteLine(object), row 2
        let mut entry = app();
        let by_object = entry.member_ref(Cell::Ref(TableId::TypeRef, 2), "WriteLine", &[0x00, 0x01, 0x01, 0x1C]);
        let mut asms = Assemblies::new(corlib.build()).unwrap();
        let app = asms.add(entry.build());
        assert_eq!(resolve_member_ref(&asms, app, 1).unwrap().row, 1);
        assert_eq!(resolve_member_ref(&asms, app, by_object).unwrap().row, 2);
    }

    #[test]
    fn class_parameters_match_across_assemblies() {
        let mut corlib = corlib_builder();
        // Show(System.Object) with Object as Class(TypeDef 2): (2 << 2) | 0 = 0x08.
        corlib.method("Show", &[0x00, 0x01, 0x01, 0x12, 0x08], None);
        let mut entry = app();
        // Caller names Object as Class(TypeRef 1): (1 << 2) | 1 = 0x05.
        let show = entry.member_ref(Cell::Ref(TableId::TypeRef, 2), "Show", &[0x00, 0x01, 0x01, 0x12, 0x05]);
        // Same name, but the parameter is Class(TypeRef 2) = System.Console.
        let wrong = entry.member_ref(Cell::Ref(TableId::TypeRef, 2), "Show", &[0x00, 0x01, 0x01, 0x12, 0x09]);
        let mut asms = Assemblies::new(corlib.build()).unwrap();
        let app = asms.add(entry.build());
        assert_eq!(resolve_member_ref(&asms, app, show).unwrap(), MethodId { asm: asms.corlib(), row: 2 });
        let err = resolve_member_ref(&asms, app, wrong).unwrap_err();
        assert_eq!(err.to_string(), "not implemented: method System.Console::Show(System.Console)");
    }

    #[test]
    fn missing_method_message() {
        let mut entry = app();
        let write = entry.member_ref(Cell::Ref(TableId::TypeRef, 2), "Write", VOID_STRING);
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(entry.build());
        let err = resolve_member_ref(&asms, app, write).unwrap_err();
        assert_eq!(err.to_string(), "not implemented: method System.Console::Write(string)");
        assert_eq!(err.exit_code(), 70);
    }

    #[test]
    fn field_references_are_not_implemented() {
        let mut entry = app();
        let field = entry.member_ref(Cell::Ref(TableId::TypeRef, 2), "Out", &[0x06, 0x1C]);
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(entry.build());
        let err = resolve_member_ref(&asms, app, field).unwrap_err();
        assert_eq!(err.to_string(), "not implemented: field reference Out");
    }

    #[test]
    fn malformed_signature_is_a_load_error() {
        let mut corlib = corlib_builder();
        corlib.method("Broken", &[0x00, 0x01], None);
        let asms = Assemblies::new(corlib.build()).unwrap();
        let err = method_sig(&asms, MethodId { asm: asms.corlib(), row: 2 }).unwrap_err();
        assert_eq!(err.exit_code(), 66);
    }
}
```
Add `pub mod methods;` to `lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p runtime methods`
Expected: compile errors — `MethodId`, `resolve_member_ref`, … not found.

- [ ] **Step 3: Implement `methods.rs`**

Insert above the test module:
```rust
use metadata::sig::{MethodSig, TypeDefOrRef, TypeSig, conv, decode_method_sig};
use metadata::tables::col;
use metadata::{MetadataModel, TableId};

use crate::error::{Result, RuntimeError};
use crate::loader::{Assemblies, AssemblyId, LoadedAssembly, qualified, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MethodId {
    pub asm: AssemblyId,
    pub row: u32,
}

/// TypeDef row owning MethodDef `method_row`: the last type whose MethodList
/// starts at or before it (§II.22.37). Returns 0 if no type owns it.
pub fn owner_type_row(model: &MetadataModel, method_row: u32) -> u32 {
    let mut owner = 0;
    for (i, r) in model.table(TableId::TypeDef).iter().enumerate() {
        match r[col::type_def::METHOD_LIST].as_index() {
            Some((_, start)) if start <= method_row => owner = i as u32 + 1,
            _ => break,
        }
    }
    owner
}

pub fn method_sig(asms: &Assemblies, method: MethodId) -> Result<MethodSig> {
    let assembly = asms.get(method.asm);
    let row = assembly.row(TableId::MethodDef, method.row)?;
    let blob = row[col::method_def::SIGNATURE].as_blob().unwrap_or(&[]);
    decode_method_sig(blob)
        .map_err(|e| assembly.bad_image(format!("signature of MethodDef row {}: {e}", method.row)))
}

/// `Namespace.Type::Method`
pub fn method_name(asms: &Assemblies, method: MethodId) -> Result<String> {
    let assembly = asms.get(method.asm);
    let name = text(assembly.row(TableId::MethodDef, method.row)?, col::method_def::NAME);
    let owner = match assembly.row(TableId::TypeDef, owner_type_row(&assembly.model, method.row)) {
        Ok(t) => qualified(text(t, col::type_def::NAMESPACE), text(t, col::type_def::NAME)),
        Err(_) => "<no type>".to_owned(),
    };
    Ok(format!("{owner}::{name}"))
}

/// Internal-call key, e.g. `System.Console::WriteLine(string)`.
pub fn method_key(asms: &Assemblies, method: MethodId) -> Result<String> {
    let sig = method_sig(asms, method)?;
    Ok(format!(
        "{}({})",
        method_name(asms, method)?,
        render_params(asms.get(method.asm), &sig)
    ))
}

fn render_params(assembly: &LoadedAssembly, sig: &MethodSig) -> String {
    sig.params.iter().map(|p| render_type(assembly, p)).collect::<Vec<_>>().join(",")
}

/// Name of a TypeDef/TypeRef from metadata alone, without resolving it.
fn type_name(assembly: &LoadedAssembly, t: TypeDefOrRef) -> String {
    let (ns_col, name_col) = match t.table {
        TableId::TypeDef => (col::type_def::NAMESPACE, col::type_def::NAME),
        TableId::TypeRef => (col::type_ref::NAMESPACE, col::type_ref::NAME),
        _ => return "?".to_owned(),
    };
    match assembly.row(t.table, t.row) {
        Ok(r) => qualified(text(r, ns_col), text(r, name_col)),
        Err(_) => "?".to_owned(),
    }
}

fn render_type(assembly: &LoadedAssembly, t: &TypeSig) -> String {
    use TypeSig as T;
    match t {
        T::Void => "void".into(),
        T::Boolean => "bool".into(),
        T::Char => "char".into(),
        T::I1 => "sbyte".into(),
        T::U1 => "byte".into(),
        T::I2 => "short".into(),
        T::U2 => "ushort".into(),
        T::I4 => "int".into(),
        T::U4 => "uint".into(),
        T::I8 => "long".into(),
        T::U8 => "ulong".into(),
        T::R4 => "float".into(),
        T::R8 => "double".into(),
        T::String => "string".into(),
        T::Object => "object".into(),
        T::I => "System.IntPtr".into(),
        T::U => "System.UIntPtr".into(),
        T::TypedByRef => "System.TypedReference".into(),
        T::Class(r) | T::ValueType(r) => type_name(assembly, *r),
        T::SzArray(e) => format!("{}[]", render_type(assembly, e)),
        T::Array { elem, rank, .. } => {
            format!("{}[{}]", render_type(assembly, elem), ",".repeat((*rank as usize).saturating_sub(1)))
        }
        T::Ptr(e) => format!("{}*", render_type(assembly, e)),
        T::ByRef(e) => format!("{}&", render_type(assembly, e)),
        T::Pinned(e) | T::Modified { ty: e, .. } => render_type(assembly, e),
        T::Var(n) => format!("!{n}"),
        T::MVar(n) => format!("!!{n}"),
        T::GenericInst { ty, args, .. } => format!(
            "{}<{}>",
            type_name(assembly, *ty),
            args.iter().map(|a| render_type(assembly, a)).collect::<Vec<_>>().join(",")
        ),
        T::FnPtr(_) => "method*".into(),
    }
}

/// Resolves MemberRef `row` of `asm` to the MethodDef it names: same name and
/// a structurally equal signature (§II.22.25).
pub fn resolve_member_ref(asms: &Assemblies, asm: AssemblyId, row: u32) -> Result<MethodId> {
    let assembly = asms.get(asm);
    let r = assembly.row(TableId::MemberRef, row)?;
    let name = text(r, col::member_ref::NAME);
    let blob = r[col::member_ref::SIGNATURE].as_blob().unwrap_or(&[]);
    if blob.first() == Some(&conv::FIELD) {
        return Err(RuntimeError::not_implemented(format!("field reference {name}")));
    }
    let wanted = decode_method_sig(blob)
        .map_err(|e| assembly.bad_image(format!("signature of MemberRef row {row}: {e}")))?;
    let (target, type_row) = match r[col::member_ref::CLASS].as_index() {
        Some((TableId::TypeRef, t)) => asms.resolve_type_ref(asm, t)?,
        Some((TableId::TypeDef, t)) => (asm, t),
        other => {
            return Err(RuntimeError::not_implemented(format!(
                "member reference with parent {other:?}"
            )));
        }
    };
    let target_assembly = asms.get(target);
    let type_def = target_assembly.row(TableId::TypeDef, type_row)?;
    let method_count = target_assembly.model.table(TableId::MethodDef).len() as u32;
    let first = type_def[col::type_def::METHOD_LIST].as_index().map_or(method_count + 1, |(_, s)| s);
    let end = target_assembly
        .model
        .table(TableId::TypeDef)
        .get(type_row as usize)
        .and_then(|next| next[col::type_def::METHOD_LIST].as_index())
        .map_or(method_count + 1, |(_, s)| s);
    for candidate_row in first..end.min(method_count + 1) {
        let candidate = MethodId { asm: target, row: candidate_row };
        if text(target_assembly.row(TableId::MethodDef, candidate_row)?, col::method_def::NAME) != name {
            continue;
        }
        let sig = method_sig(asms, candidate)?;
        if sig_matches(asms, (asm, &wanted), (target, &sig))? {
            return Ok(candidate);
        }
    }
    let owner = qualified(
        text(type_def, col::type_def::NAMESPACE),
        text(type_def, col::type_def::NAME),
    );
    Err(RuntimeError::not_implemented(format!(
        "method {owner}::{name}({})",
        render_params(assembly, &wanted)
    )))
}

/// (assembly, namespace, name) of a type token used in a signature.
fn type_identity(asms: &Assemblies, asm: AssemblyId, t: TypeDefOrRef) -> Result<(AssemblyId, String, String)> {
    match t.table {
        TableId::TypeDef => {
            let r = asms.get(asm).row(TableId::TypeDef, t.row)?;
            Ok((
                asm,
                text(r, col::type_def::NAMESPACE).to_owned(),
                text(r, col::type_def::NAME).to_owned(),
            ))
        }
        TableId::TypeRef => asms.type_ref_identity(asm, t.row),
        other => Err(RuntimeError::not_implemented(format!("{other:?} in a signature"))),
    }
}

fn same_type(asms: &Assemblies, a: (AssemblyId, TypeDefOrRef), b: (AssemblyId, TypeDefOrRef)) -> Result<bool> {
    Ok(type_identity(asms, a.0, a.1)? == type_identity(asms, b.0, b.1)?)
}

fn sig_matches(asms: &Assemblies, a: (AssemblyId, &MethodSig), b: (AssemblyId, &MethodSig)) -> Result<bool> {
    let (x, y) = (a.1, b.1);
    if x.conv != y.conv
        || x.generic_params != y.generic_params
        || x.sentinel != y.sentinel
        || x.params.len() != y.params.len()
    {
        return Ok(false);
    }
    if !type_matches(asms, (a.0, &x.ret), (b.0, &y.ret))? {
        return Ok(false);
    }
    for (p, q) in x.params.iter().zip(&y.params) {
        if !type_matches(asms, (a.0, p), (b.0, q))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Structural equality where type tokens are compared by identity, so a
/// TypeRef in one assembly equals the TypeDef it names in another.
fn type_matches(asms: &Assemblies, a: (AssemblyId, &TypeSig), b: (AssemblyId, &TypeSig)) -> Result<bool> {
    use TypeSig as T;
    let ((ia, x), (ib, y)) = (a, b);
    Ok(match (x, y) {
        (T::Class(p), T::Class(q)) | (T::ValueType(p), T::ValueType(q)) => same_type(asms, (ia, *p), (ib, *q))?,
        (T::Ptr(p), T::Ptr(q))
        | (T::ByRef(p), T::ByRef(q))
        | (T::Pinned(p), T::Pinned(q))
        | (T::SzArray(p), T::SzArray(q)) => type_matches(asms, (ia, p.as_ref()), (ib, q.as_ref()))?,
        (
            T::Array { elem: p, rank: r1, sizes: s1, lo_bounds: l1 },
            T::Array { elem: q, rank: r2, sizes: s2, lo_bounds: l2 },
        ) => r1 == r2 && s1 == s2 && l1 == l2 && type_matches(asms, (ia, p.as_ref()), (ib, q.as_ref()))?,
        (
            T::GenericInst { value_type: v1, ty: t1, args: a1 },
            T::GenericInst { value_type: v2, ty: t2, args: a2 },
        ) => {
            if v1 != v2 || a1.len() != a2.len() || !same_type(asms, (ia, *t1), (ib, *t2))? {
                return Ok(false);
            }
            for (p, q) in a1.iter().zip(a2) {
                if !type_matches(asms, (ia, p), (ib, q))? {
                    return Ok(false);
                }
            }
            true
        }
        (
            T::Modified { required: r1, modifier: m1, ty: p },
            T::Modified { required: r2, modifier: m2, ty: q },
        ) => {
            r1 == r2
                && same_type(asms, (ia, *m1), (ib, *m2))?
                && type_matches(asms, (ia, p.as_ref()), (ib, q.as_ref()))?
        }
        (T::FnPtr(p), T::FnPtr(q)) => sig_matches(asms, (ia, p.as_ref()), (ib, q.as_ref()))?,
        // Primitives, Var and MVar carry no tokens.
        _ => x == y,
    })
}
```
Note: the final `_ => x == y` arm is correct only because every token-carrying variant pair is handled above; a pair of different variants is unequal anyway.

- [ ] **Step 4: Run tests**

Run: `cargo test -p runtime`
Expected: all PASS (22 + 9 = 31), no warnings.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/runtime
git commit -m "feat(runtime): method resolution with structural signature matching

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Stack values, opcode table and internal calls

**Files:**
- Create: `crates/runtime/src/value.rs`, `crates/runtime/src/opcodes.rs`, `crates/runtime/src/internal_calls.rs`
- Modify: `crates/runtime/src/lib.rs` (add `pub mod internal_calls; pub mod opcodes; pub mod value;` and `pub use value::StackValue;`)

**Interfaces:**
- Consumes: `Heap`, `ObjRef`, `TypeId`, `RuntimeError`, `Result` (Task 2).
- Produces:
  - `value::StackValue { I32(i32), I64(i64), F64(f64), NativeInt(i64), Obj(Option<ObjRef>) }` (`Debug, Clone, Copy, PartialEq`).
  - `opcodes::{NOP, LDARG_0, LDARG_3, LDARG_S, LDNULL, LDC_I4_M1, LDC_I4_0, LDC_I4_8, LDC_I4_S, LDC_I4, CALL, RET, LDSTR, NEWOBJ, PREFIX_FE}: u8`; `opcodes::opcode_name(u8) -> &'static str` (`"unknown"` for unassigned bytes).
  - `internal_calls::CallContext<'a> { pub heap: &'a mut Heap, pub out: &'a mut dyn Write, pub string_type: TypeId }`; `type InternalCall = fn(&mut CallContext<'_>, &[StackValue]) -> Result<Option<StackValue>>`; `lookup(key: &str) -> Option<InternalCall>`.

- [ ] **Step 1: Write the failing tests**

`crates/runtime/src/opcodes.rs` — tests only for now:
```rust
//! CIL opcode bytes (ECMA-335 Partition III) used by the interpreter, and
//! names for error messages.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_of_common_opcodes() {
        assert_eq!(opcode_name(NOP), "nop");
        assert_eq!(opcode_name(LDSTR), "ldstr");
        assert_eq!(opcode_name(CALL), "call");
        assert_eq!(opcode_name(NEWOBJ), "newobj");
        assert_eq!(opcode_name(0x6F), "callvirt");
        assert_eq!(opcode_name(0x8D), "newarr");
        assert_eq!(opcode_name(0xDD), "leave");
        assert_eq!(opcode_name(0xE0), "conv.u");
    }

    #[test]
    fn unassigned_bytes_are_unknown() {
        for b in [0x24, 0x77, 0x78, 0xA6, 0xC0, 0xE1, 0xFF] {
            assert_eq!(opcode_name(b), "unknown", "{b:#04x}");
        }
    }

    #[test]
    fn ldc_i4_short_forms_are_contiguous() {
        assert_eq!(LDC_I4_0 - LDC_I4_M1, 1);
        assert_eq!(LDC_I4_8 - LDC_I4_0, 8);
        assert_eq!(LDARG_3 - LDARG_0, 3);
    }
}
```

`crates/runtime/src/internal_calls.rs` — tests only for now:
```rust
//! Rust implementations of corlib methods that have no IL body (spec §5.3
//! "Internal calls"), keyed like `methods::method_key`.

#[cfg(test)]
mod tests {
    use super::*;

    const STRING: TypeId = TypeId(1);
    const ARRAY: TypeId = TypeId(2);

    fn write_line(heap: &mut Heap, args: &[StackValue]) -> (Result<Option<StackValue>>, String) {
        let f = lookup("System.Console::WriteLine(string)").expect("registered");
        let mut out = Vec::new();
        let result = f(&mut CallContext { heap, out: &mut out, string_type: STRING }, args);
        (result, String::from_utf8(out).unwrap())
    }

    #[test]
    fn write_line_prints_utf8_and_newline() {
        let mut heap = Heap::new();
        let units: Vec<u16> = "Grüße, 世界 😀".encode_utf16().collect();
        let s = heap.alloc_string(STRING, &units).unwrap();
        let (result, out) = write_line(&mut heap, &[StackValue::Obj(Some(s))]);
        assert_eq!(result, Ok(None));
        assert_eq!(out, "Grüße, 世界 😀\n");
    }

    #[test]
    fn write_line_of_null_prints_an_empty_line() {
        let (result, out) = write_line(&mut Heap::new(), &[StackValue::Obj(None)]);
        assert_eq!(result, Ok(None));
        assert_eq!(out, "\n");
    }

    #[test]
    fn write_line_rejects_non_string_objects() {
        let mut heap = Heap::new();
        let array = heap.alloc_ref_array(ARRAY, &[None]).unwrap();
        let (result, out) = write_line(&mut heap, &[StackValue::Obj(Some(array))]);
        assert!(matches!(result, Err(RuntimeError::InvalidProgram { .. })), "{result:?}");
        assert_eq!(out, "");
        let (result, _) = write_line(&mut heap, &[StackValue::I32(1)]);
        assert!(matches!(result, Err(RuntimeError::InvalidProgram { .. })));
        let (result, _) = write_line(&mut heap, &[]);
        assert!(matches!(result, Err(RuntimeError::InvalidProgram { .. })));
    }

    #[test]
    fn write_failures_are_io_errors() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("pipe closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let f = lookup("System.Console::WriteLine(string)").unwrap();
        let mut heap = Heap::new();
        let result = f(&mut CallContext { heap: &mut heap, out: &mut Broken, string_type: STRING }, &[StackValue::Obj(None)]);
        assert!(matches!(result, Err(RuntimeError::Io(_))), "{result:?}");
    }

    #[test]
    fn unknown_keys_are_not_registered() {
        assert!(lookup("System.Console::Write(string)").is_none());
        assert!(lookup("System.Console::WriteLine(object)").is_none());
    }
}
```

`crates/runtime/src/value.rs` (complete — no logic to test on its own):
```rust
//! Evaluation-stack values (§III.1.1 stack types).

use crate::heap::ObjRef;

/// A typed evaluation-stack slot. A managed-pointer variant is added with the
/// first opcode that produces one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StackValue {
    I32(i32),
    I64(i64),
    F64(f64),
    NativeInt(i64),
    /// Object reference; `None` is null.
    Obj(Option<ObjRef>),
}
```

Update `crates/runtime/src/lib.rs` module list to:
```rust
pub mod error;
pub mod heap;
pub mod internal_calls;
pub mod loader;
pub mod methods;
pub mod opcodes;
pub mod types;
pub mod value;

#[cfg(test)]
mod testing;

pub use error::{Location, Result, RuntimeError};
pub use loader::{Assemblies, AssemblyId, LoadedAssembly};
pub use value::StackValue;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p runtime`
Expected: compile errors — `opcode_name`, `lookup`, `CallContext` not found.

- [ ] **Step 3: Implement `opcodes.rs`**

Insert above the test module:
```rust
pub const NOP: u8 = 0x00;
pub const LDARG_0: u8 = 0x02;
pub const LDARG_3: u8 = 0x05;
pub const LDARG_S: u8 = 0x0E;
pub const LDNULL: u8 = 0x14;
pub const LDC_I4_M1: u8 = 0x15;
pub const LDC_I4_0: u8 = 0x16;
pub const LDC_I4_8: u8 = 0x1E;
pub const LDC_I4_S: u8 = 0x1F;
pub const LDC_I4: u8 = 0x20;
pub const CALL: u8 = 0x28;
pub const RET: u8 = 0x2A;
pub const LDSTR: u8 = 0x72;
pub const NEWOBJ: u8 = 0x73;
/// First byte of two-byte opcodes.
pub const PREFIX_FE: u8 = 0xFE;

/// Mnemonic of a one-byte opcode (§III.1.2.1), `"unknown"` if unassigned.
pub fn opcode_name(opcode: u8) -> &'static str {
    match opcode {
        0x00 => "nop",
        0x01 => "break",
        0x02 => "ldarg.0",
        0x03 => "ldarg.1",
        0x04 => "ldarg.2",
        0x05 => "ldarg.3",
        0x06 => "ldloc.0",
        0x07 => "ldloc.1",
        0x08 => "ldloc.2",
        0x09 => "ldloc.3",
        0x0A => "stloc.0",
        0x0B => "stloc.1",
        0x0C => "stloc.2",
        0x0D => "stloc.3",
        0x0E => "ldarg.s",
        0x0F => "ldarga.s",
        0x10 => "starg.s",
        0x11 => "ldloc.s",
        0x12 => "ldloca.s",
        0x13 => "stloc.s",
        0x14 => "ldnull",
        0x15 => "ldc.i4.m1",
        0x16 => "ldc.i4.0",
        0x17 => "ldc.i4.1",
        0x18 => "ldc.i4.2",
        0x19 => "ldc.i4.3",
        0x1A => "ldc.i4.4",
        0x1B => "ldc.i4.5",
        0x1C => "ldc.i4.6",
        0x1D => "ldc.i4.7",
        0x1E => "ldc.i4.8",
        0x1F => "ldc.i4.s",
        0x20 => "ldc.i4",
        0x21 => "ldc.i8",
        0x22 => "ldc.r4",
        0x23 => "ldc.r8",
        0x25 => "dup",
        0x26 => "pop",
        0x27 => "jmp",
        0x28 => "call",
        0x29 => "calli",
        0x2A => "ret",
        0x2B => "br.s",
        0x2C => "brfalse.s",
        0x2D => "brtrue.s",
        0x2E => "beq.s",
        0x2F => "bge.s",
        0x30 => "bgt.s",
        0x31 => "ble.s",
        0x32 => "blt.s",
        0x33 => "bne.un.s",
        0x34 => "bge.un.s",
        0x35 => "bgt.un.s",
        0x36 => "ble.un.s",
        0x37 => "blt.un.s",
        0x38 => "br",
        0x39 => "brfalse",
        0x3A => "brtrue",
        0x3B => "beq",
        0x3C => "bge",
        0x3D => "bgt",
        0x3E => "ble",
        0x3F => "blt",
        0x40 => "bne.un",
        0x41 => "bge.un",
        0x42 => "bgt.un",
        0x43 => "ble.un",
        0x44 => "blt.un",
        0x45 => "switch",
        0x46 => "ldind.i1",
        0x47 => "ldind.u1",
        0x48 => "ldind.i2",
        0x49 => "ldind.u2",
        0x4A => "ldind.i4",
        0x4B => "ldind.u4",
        0x4C => "ldind.i8",
        0x4D => "ldind.i",
        0x4E => "ldind.r4",
        0x4F => "ldind.r8",
        0x50 => "ldind.ref",
        0x51 => "stind.ref",
        0x52 => "stind.i1",
        0x53 => "stind.i2",
        0x54 => "stind.i4",
        0x55 => "stind.i8",
        0x56 => "stind.r4",
        0x57 => "stind.r8",
        0x58 => "add",
        0x59 => "sub",
        0x5A => "mul",
        0x5B => "div",
        0x5C => "div.un",
        0x5D => "rem",
        0x5E => "rem.un",
        0x5F => "and",
        0x60 => "or",
        0x61 => "xor",
        0x62 => "shl",
        0x63 => "shr",
        0x64 => "shr.un",
        0x65 => "neg",
        0x66 => "not",
        0x67 => "conv.i1",
        0x68 => "conv.i2",
        0x69 => "conv.i4",
        0x6A => "conv.i8",
        0x6B => "conv.r4",
        0x6C => "conv.r8",
        0x6D => "conv.u4",
        0x6E => "conv.u8",
        0x6F => "callvirt",
        0x70 => "cpobj",
        0x71 => "ldobj",
        0x72 => "ldstr",
        0x73 => "newobj",
        0x74 => "castclass",
        0x75 => "isinst",
        0x76 => "conv.r.un",
        0x79 => "unbox",
        0x7A => "throw",
        0x7B => "ldfld",
        0x7C => "ldflda",
        0x7D => "stfld",
        0x7E => "ldsfld",
        0x7F => "ldsflda",
        0x80 => "stsfld",
        0x81 => "stobj",
        0x82 => "conv.ovf.i1.un",
        0x83 => "conv.ovf.i2.un",
        0x84 => "conv.ovf.i4.un",
        0x85 => "conv.ovf.i8.un",
        0x86 => "conv.ovf.u1.un",
        0x87 => "conv.ovf.u2.un",
        0x88 => "conv.ovf.u4.un",
        0x89 => "conv.ovf.u8.un",
        0x8A => "conv.ovf.i.un",
        0x8B => "conv.ovf.u.un",
        0x8C => "box",
        0x8D => "newarr",
        0x8E => "ldlen",
        0x8F => "ldelema",
        0x90 => "ldelem.i1",
        0x91 => "ldelem.u1",
        0x92 => "ldelem.i2",
        0x93 => "ldelem.u2",
        0x94 => "ldelem.i4",
        0x95 => "ldelem.u4",
        0x96 => "ldelem.i8",
        0x97 => "ldelem.i",
        0x98 => "ldelem.r4",
        0x99 => "ldelem.r8",
        0x9A => "ldelem.ref",
        0x9B => "stelem.i",
        0x9C => "stelem.i1",
        0x9D => "stelem.i2",
        0x9E => "stelem.i4",
        0x9F => "stelem.i8",
        0xA0 => "stelem.r4",
        0xA1 => "stelem.r8",
        0xA2 => "stelem.ref",
        0xA3 => "ldelem",
        0xA4 => "stelem",
        0xA5 => "unbox.any",
        0xB3 => "conv.ovf.i1",
        0xB4 => "conv.ovf.u1",
        0xB5 => "conv.ovf.i2",
        0xB6 => "conv.ovf.u2",
        0xB7 => "conv.ovf.i4",
        0xB8 => "conv.ovf.u4",
        0xB9 => "conv.ovf.i8",
        0xBA => "conv.ovf.u8",
        0xC2 => "refanyval",
        0xC3 => "ckfinite",
        0xC6 => "mkrefany",
        0xD0 => "ldtoken",
        0xD1 => "conv.u2",
        0xD2 => "conv.u1",
        0xD3 => "conv.i",
        0xD4 => "conv.ovf.i",
        0xD5 => "conv.ovf.u",
        0xD6 => "add.ovf",
        0xD7 => "add.ovf.un",
        0xD8 => "mul.ovf",
        0xD9 => "mul.ovf.un",
        0xDA => "sub.ovf",
        0xDB => "sub.ovf.un",
        0xDC => "endfinally",
        0xDD => "leave",
        0xDE => "leave.s",
        0xDF => "stind.i",
        0xE0 => "conv.u",
        _ => "unknown",
    }
}
```

- [ ] **Step 4: Implement `internal_calls.rs`**

Insert above the test module:
```rust
use std::io::Write;

use crate::error::{Result, RuntimeError};
use crate::heap::Heap;
use crate::types::TypeId;
use crate::value::StackValue;

/// What an internal call may touch.
pub struct CallContext<'a> {
    pub heap: &'a mut Heap,
    pub out: &'a mut dyn Write,
    /// `System.String`, to check that string arguments really are strings.
    pub string_type: TypeId,
}

pub type InternalCall = fn(&mut CallContext<'_>, &[StackValue]) -> Result<Option<StackValue>>;

pub fn lookup(key: &str) -> Option<InternalCall> {
    match key {
        "System.Console::WriteLine(string)" => Some(console_write_line),
        _ => None,
    }
}

/// Writes the UTF-16 string as UTF-8 plus `\n`; null writes an empty line.
/// Unpaired surrogates become U+FFFD.
fn console_write_line(ctx: &mut CallContext<'_>, args: &[StackValue]) -> Result<Option<StackValue>> {
    let text = match args {
        [StackValue::Obj(None)] => String::new(),
        [StackValue::Obj(Some(s))] if ctx.heap.type_of(*s) == ctx.string_type => {
            String::from_utf16_lossy(&ctx.heap.string_units(*s))
        }
        _ => {
            return Err(RuntimeError::invalid_program(
                "System.Console::WriteLine(string) expects one string argument",
            ));
        }
    };
    writeln!(ctx.out, "{text}").map_err(|e| RuntimeError::Io(e.to_string()))?;
    Ok(None)
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p runtime`
Expected: all PASS (31 + 3 + 5 = 39), no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/runtime
git commit -m "feat(runtime): stack values, opcode names and Console.WriteLine internal call

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: IL interpreter and `run` entry points

**Files:**
- Create: `crates/runtime/src/interp.rs`
- Modify: `crates/runtime/src/lib.rs` (add `pub mod interp;`, `run`, `run_files`), `crates/runtime/src/testing.rs` (add run helpers)

**Interfaces:**
- Consumes: everything from Tasks 2–5.
- Produces:
  - `interp::MAX_FRAMES: usize = 10_000`.
  - `interp::Interpreter<'o>`: `new(Assemblies, &'o mut dyn Write) -> Self`, `run_entry(&mut self, entry: AssemblyId, args: &[String]) -> Result<i32>`.
  - `runtime::run(corlib: LoadedAssembly, entry: LoadedAssembly, args: &[String], out: &mut dyn Write) -> Result<i32>`.
  - `runtime::run_files(entry: &Path, corlib: &Path, args: &[String], out: &mut dyn Write) -> Result<i32>` (loads the entry first, so a missing entry file is reported before a missing corlib).
  - `testing::{run, run_with, il}`.
- Implemented opcodes (spec §4): `nop`, `ldarg.0`–`ldarg.3`, `ldarg.s`, `ldnull`, `ldc.i4.m1`–`ldc.i4.8`, `ldc.i4.s`, `ldc.i4`, `ldstr`, `call`, `ret`. Everything else → `not implemented: opcode 0xNN (name)`; two-byte opcodes → `not implemented: opcode 0xFE 0xNN`.

- [ ] **Step 1: Add the run helpers to `testing.rs`**

Append to `crates/runtime/src/testing.rs`:
```rust
/// Runs `entry` against `corlib`; returns the outcome and captured stdout.
pub fn run_with(corlib: LoadedAssembly, entry: ModelBuilder, args: &[&str]) -> (crate::Result<i32>, String) {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let mut out = Vec::new();
    let result = crate::run(corlib, entry.build(), &args, &mut out);
    (result, String::from_utf8(out).expect("program output is UTF-8"))
}

/// Runs `entry` against `test_corlib()`.
pub fn run(entry: ModelBuilder, args: &[&str]) -> (crate::Result<i32>, String) {
    run_with(test_corlib(), entry, args)
}

/// Concatenates IL fragments, e.g. `il(&[&[LDSTR], &token.to_le_bytes()])`.
pub fn il(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}
```

- [ ] **Step 2: Write the failing tests**

`crates/runtime/src/interp.rs` — tests only for now:
```rust
//! IL interpreter (spec §5.3 "Interpreter"): explicit frame stack, typed
//! evaluation stack, lazy resolution of call targets.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcodes::*;
    use crate::testing::{INT_NO_ARGS, VOID_NO_ARGS, VOID_STRING, WRITE_LINE, app, corlib_builder, il, run, run_with, test_corlib};
    use metadata::Cell;

    fn print(token: u32) -> Vec<u8> {
        il(&[&[LDSTR], &token.to_le_bytes(), &[CALL], &WRITE_LINE.to_le_bytes()])
    }

    fn error_of(result: Result<i32>) -> RuntimeError {
        result.expect_err("program should fail")
    }

    #[test]
    fn prints_hello() {
        let mut b = app();
        let hello = b.user_string("Hello, World!");
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&print(hello), &[RET]])));
        b.entry_point(main);
        assert_eq!(run(b, &[]), (Ok(0), "Hello, World!\n".to_owned()));
    }

    #[test]
    fn int_main_returns_exit_code() {
        let cases: &[(&[u8], i32)] = &[
            (&[LDC_I4_M1, RET], -1),
            (&[LDC_I4_0, RET], 0),
            (&[LDC_I4_0 + 3, RET], 3),
            (&[LDC_I4_8, RET], 8),
            (&[LDC_I4_S, 0xF6, RET], -10),
            (&[LDC_I4, 0x40, 0x42, 0x0F, 0x00, RET], 1_000_000),
        ];
        for (code, expected) in cases {
            let mut b = app();
            let main = b.method("Main", INT_NO_ARGS, Some(code));
            b.entry_point(main);
            assert_eq!(run(b, &[]).0, Ok(*expected), "{code:02x?}");
        }
    }

    #[test]
    fn static_calls_pass_arguments_and_return_values() {
        let mut b = app();
        let ada = b.user_string("Ada");
        let hi = b.user_string("Hi");
        // MethodDef 1: string Name() => "Ada"
        b.method("Name", &[0x00, 0x00, 0x0E], Some(&il(&[&[LDSTR], &ada.to_le_bytes(), &[RET]])));
        // MethodDef 2: void Greet(string who, string greeting) prints greeting then who
        b.method(
            "Greet",
            &[0x00, 0x02, 0x01, 0x0E, 0x0E],
            Some(&il(&[&[LDARG_0 + 1, CALL], &WRITE_LINE.to_le_bytes(), &[LDARG_0, CALL], &WRITE_LINE.to_le_bytes(), &[RET]])),
        );
        // MethodDef 3: Main() { Greet(Name(), "Hi"); }
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&[CALL], &0x0600_0001u32.to_le_bytes(), &[LDSTR], &hi.to_le_bytes(), &[CALL], &0x0600_0002u32.to_le_bytes(), &[RET]])),
        );
        b.entry_point(main);
        assert_eq!(run(b, &[]), (Ok(0), "Hi\nAda\n".to_owned()));
    }

    #[test]
    fn ldnull_and_ldarg_s() {
        let mut b = app();
        b.method("Show", VOID_STRING, Some(&il(&[&[LDARG_S, 0, CALL], &WRITE_LINE.to_le_bytes(), &[RET]])));
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[NOP, LDNULL, CALL], &0x0600_0001u32.to_le_bytes(), &[RET]])));
        b.entry_point(main);
        assert_eq!(run(b, &[]), (Ok(0), "\n".to_owned()));
    }

    #[test]
    fn main_with_string_array_receives_arguments() {
        let mut b = app();
        let main = b.method("Main", &[0x00, 0x01, 0x01, 0x1D, 0x0E], Some(&[RET]));
        b.entry_point(main);
        assert_eq!(run(b, &["one", "two"]), (Ok(0), String::new()));
    }

    #[test]
    fn args_array_holds_the_strings() {
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        asms.add(app().build());
        let mut out = Vec::new();
        let mut interp = Interpreter::new(asms, &mut out);
        let array = interp.args_array(&["a".to_owned(), "bé".to_owned()]).unwrap();
        let array_type = interp.heap.type_of(array);
        assert_eq!(interp.types.get(array_type).full_name(), "System.String[]");
        let elements = interp.heap.ref_array_elements(array);
        let texts: Vec<String> = elements
            .iter()
            .map(|e| String::from_utf16(&interp.heap.string_units(e.unwrap())).unwrap())
            .collect();
        assert_eq!(texts, ["a", "bé"]);
    }

    #[test]
    fn ldstr_is_interned_per_token() {
        let mut b = app();
        let a = b.user_string("a");
        let other = b.user_string("b");
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&print(a), &print(a), &print(other), &[RET]])));
        b.entry_point(main);
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let entry = asms.add(b.build());
        let mut out = Vec::new();
        let mut interp = Interpreter::new(asms, &mut out);
        assert_eq!(interp.run_entry(entry, &[]), Ok(0));
        assert_eq!(interp.strings.len(), 2);
        drop(interp);
        assert_eq!(String::from_utf8(out).unwrap(), "a\na\nb\n");
    }

    #[test]
    fn unresolvable_references_are_ignored_until_used() {
        // Like mcs output: a TypeRef/MemberRef our corlib cannot satisfy.
        let mut b = app();
        let attr = b.type_ref(1, "System.Runtime.CompilerServices", "RuntimeCompatibilityAttribute");
        b.member_ref(Cell::Ref(TableId::TypeRef, attr), ".ctor", &[0x20, 0x00, 0x01]);
        let hello = b.user_string("still runs");
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&print(hello), &[RET]])));
        b.entry_point(main);
        assert_eq!(run(b, &[]), (Ok(0), "still runs\n".to_owned()));
    }

    #[test]
    fn unsupported_opcode_reports_spec_format() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[NOP, NEWOBJ, 1, 0, 0, 0x0A, RET]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(err.to_string(), "not implemented: opcode 0x73 (newobj) in App::Main @IL_0001");
        assert_eq!(err.exit_code(), 70);
    }

    #[test]
    fn two_byte_opcodes_are_not_implemented() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[PREFIX_FE, 0x01, RET]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(err.to_string(), "not implemented: opcode 0xFE 0x01 in App::Main @IL_0000");
    }

    #[test]
    fn missing_method_reports_spec_format() {
        let mut b = app();
        let write = b.member_ref(Cell::Ref(TableId::TypeRef, 2), "Write", VOID_STRING);
        let x = b.user_string("x");
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&[LDSTR], &x.to_le_bytes(), &[CALL], &TableId::MemberRef.token(write).to_le_bytes(), &[RET]])),
        );
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(err.to_string(), "not implemented: method System.Console::Write(string) in App::Main @IL_0005");
    }

    #[test]
    fn missing_internal_call_is_reported() {
        let mut corlib = corlib_builder();
        corlib.method("Beep", VOID_NO_ARGS, None);
        let mut b = app();
        let beep = b.member_ref(Cell::Ref(TableId::TypeRef, 2), "Beep", VOID_NO_ARGS);
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[CALL], &TableId::MemberRef.token(beep).to_le_bytes(), &[RET]])));
        b.entry_point(main);
        let err = error_of(run_with(corlib.build(), b, &[]).0);
        assert_eq!(err.to_string(), "not implemented: internal call System.Console::Beep() in App::Main @IL_0000");
    }

    #[test]
    fn instance_calls_are_not_implemented() {
        let mut corlib = corlib_builder();
        corlib.method("Clear", &[0x20, 0x00, 0x01], Some(&[RET]));
        let mut b = app();
        let clear = b.member_ref(Cell::Ref(TableId::TypeRef, 2), "Clear", &[0x20, 0x00, 0x01]);
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[LDNULL, CALL], &TableId::MemberRef.token(clear).to_le_bytes(), &[RET]])));
        b.entry_point(main);
        let err = error_of(run_with(corlib.build(), b, &[]).0);
        assert!(err.to_string().starts_with("not implemented: instance method call System.Console::Clear()"), "{err}");
    }

    #[test]
    fn unbounded_recursion_is_reported() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[CALL], &0x0600_0001u32.to_le_bytes(), &[RET]])));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(err.to_string().contains(&format!("call depth above {MAX_FRAMES}")), "{err}");
        assert_eq!(err.exit_code(), 70);
    }

    #[test]
    fn truncated_operand_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[LDSTR, 0x01, 0x00]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(err.to_string(), "invalid program: unexpected end of method body in App::Main @IL_0000");
    }

    #[test]
    fn falling_off_the_end_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[NOP]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(matches!(err, RuntimeError::InvalidProgram { .. }), "{err}");
    }

    #[test]
    fn call_to_missing_member_ref_is_a_load_error() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[CALL], &0x0A00_0063u32.to_le_bytes(), &[RET]])));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(err.exit_code(), 66, "{err}");
        assert!(err.to_string().contains("MemberRef row 99 does not exist"), "{err}");
    }

    #[test]
    fn call_with_non_method_token_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[CALL], &0x0200_0001u32.to_le_bytes(), &[RET]])));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(matches!(err, RuntimeError::InvalidProgram { .. }), "{err}");
    }

    #[test]
    fn ldstr_with_non_string_token_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[LDSTR], &0x0A00_0001u32.to_le_bytes(), &[RET]])));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(matches!(err, RuntimeError::InvalidProgram { .. }), "{err}");
    }

    #[test]
    fn ldstr_with_missing_us_entry_is_a_load_error() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[LDSTR], &0x7000_0042u32.to_le_bytes(), &[RET]])));
        b.entry_point(main);
        assert_eq!(error_of(run(b, &[]).0).exit_code(), 66);
    }

    #[test]
    fn stack_underflow_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[CALL], &WRITE_LINE.to_le_bytes(), &[RET]])));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(err.to_string().starts_with("invalid program: stack underflow"), "{err}");
    }

    #[test]
    fn ret_with_extra_values_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[LDNULL, RET]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(matches!(err, RuntimeError::InvalidProgram { .. }), "{err}");
    }

    #[test]
    fn missing_argument_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[LDARG_0 + 1, RET]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(err.to_string(), "invalid program: argument 1 does not exist in App::Main @IL_0000");
    }

    #[test]
    fn no_entry_point_is_a_load_error() {
        let b = app();
        assert_eq!(error_of(run(b, &[]).0).exit_code(), 66);
    }

    #[test]
    fn unsupported_entry_signature_is_not_implemented() {
        let mut b = app();
        let main = b.method("Main", &[0x00, 0x01, 0x01, 0x08], Some(&[RET])); // Main(int)
        b.entry_point(main);
        assert_eq!(error_of(run(b, &[]).0).exit_code(), 70);
    }

    #[test]
    fn corlib_without_string_is_a_load_error() {
        let mut corlib = crate::testing::ModelBuilder::new("mscorlib");
        corlib.type_def("System", "Object", Cell::Ref(TableId::TypeDef, 0));
        let mut b = app();
        let s = b.user_string("x");
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&[LDSTR], &s.to_le_bytes(), &[RET]])));
        b.entry_point(main);
        let err = error_of(run_with(corlib.build(), b, &[]).0);
        assert!(err.to_string().contains("corlib does not define System.String"), "{err}");
    }
}
```
Add `pub mod interp;` to `lib.rs`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p runtime interp`
Expected: compile errors — `Interpreter`, `MAX_FRAMES`, `crate::run` not found.

- [ ] **Step 4: Implement `interp.rs`**

Insert above the test module:
```rust
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

use metadata::TableId;
use metadata::sig::{TypeSig, conv};
use metadata::tables::col;

use crate::error::{Location, Result, RuntimeError};
use crate::heap::{Heap, ObjRef};
use crate::internal_calls::{self, CallContext};
use crate::loader::{Assemblies, AssemblyId};
use crate::methods::{self, MethodId};
use crate::opcodes::{self as op, opcode_name};
use crate::types::{TypeId, TypeSystem};
use crate::value::StackValue;

/// Managed call depth limit. Exceeding it ends the program (there are no
/// exceptions yet to raise StackOverflowException).
pub const MAX_FRAMES: usize = 10_000;

struct Frame {
    method: MethodId,
    code: Rc<[u8]>,
    ip: usize,
    args: Vec<StackValue>,
    stack: Vec<StackValue>,
    returns_value: bool,
}

impl Frame {
    fn operand(&mut self, len: usize) -> Result<&[u8]> {
        let end = self
            .ip
            .checked_add(len)
            .filter(|&end| end <= self.code.len())
            .ok_or_else(|| RuntimeError::invalid_program("unexpected end of method body"))?;
        let bytes = &self.code[self.ip..end];
        self.ip = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.operand(1)?[0])
    }

    fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.operand(4)?.try_into().expect("4-byte operand")))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn push(&mut self, value: StackValue) {
        self.stack.push(value);
    }

    fn arg(&self, index: usize) -> Result<StackValue> {
        self.args
            .get(index)
            .copied()
            .ok_or_else(|| RuntimeError::invalid_program(format!("argument {index} does not exist")))
    }
}

enum Flow {
    Continue,
    /// The outermost frame returned this value.
    Exit(Option<StackValue>),
}

pub struct Interpreter<'o> {
    asms: Assemblies,
    types: TypeSystem,
    heap: Heap,
    /// `ldstr` results, interned per (module, token).
    strings: HashMap<(AssemblyId, u32), ObjRef>,
    frames: Vec<Frame>,
    out: &'o mut dyn Write,
}

impl<'o> Interpreter<'o> {
    pub fn new(asms: Assemblies, out: &'o mut dyn Write) -> Self {
        Self {
            asms,
            types: TypeSystem::new(),
            heap: Heap::new(),
            strings: HashMap::new(),
            frames: Vec::new(),
            out,
        }
    }

    /// Runs the entry point of `entry` and returns the process exit code:
    /// 0 for `void Main`, the return value for `int Main`.
    pub fn run_entry(&mut self, entry: AssemblyId, args: &[String]) -> Result<i32> {
        let assembly = self.asms.get(entry);
        let token = assembly.model.entry_point_token;
        if token == 0 {
            return Err(assembly.bad_image("assembly has no entry point"));
        }
        let main = MethodId { asm: entry, row: token & 0x00FF_FFFF };
        let sig = methods::method_sig(&self.asms, main)?;
        let supported_return = matches!(sig.ret, TypeSig::Void | TypeSig::I4);
        let main_args = match sig.params.as_slice() {
            [] => Vec::new(),
            [TypeSig::SzArray(element)] if **element == TypeSig::String => {
                vec![StackValue::Obj(Some(self.args_array(args)?))]
            }
            _ => Vec::new(),
        };
        if sig.conv != conv::DEFAULT || !supported_return || main_args.len() != sig.params.len() {
            return Err(RuntimeError::not_implemented(format!(
                "entry point signature of {}",
                methods::method_key(&self.asms, main)?
            )));
        }
        self.push_frame(main, main_args, sig.ret != TypeSig::Void)?;
        let result = self.execute()?;
        self.out.flush().map_err(|e| RuntimeError::Io(e.to_string()))?;
        Ok(match result {
            Some(StackValue::I32(code)) => code,
            _ => 0,
        })
    }

    fn execute(&mut self) -> Result<Option<StackValue>> {
        loop {
            let frame = self.frames.last().expect("execute runs with a frame");
            let (method, at) = (frame.method, frame.ip as u32);
            match self.step() {
                Ok(Flow::Continue) => {}
                Ok(Flow::Exit(value)) => return Ok(value),
                Err(e) => {
                    let name = methods::method_name(&self.asms, method)
                        .unwrap_or_else(|_| "<unknown method>".to_owned());
                    return Err(e.at(Location { method: name, il_offset: at }));
                }
            }
        }
    }

    fn top(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a frame is executing")
    }

    fn step(&mut self) -> Result<Flow> {
        let frame = self.top();
        let opcode = frame.u8()?;
        match opcode {
            op::NOP => {}
            op::LDARG_0..=op::LDARG_3 => {
                let value = frame.arg(usize::from(opcode - op::LDARG_0))?;
                frame.push(value);
            }
            op::LDARG_S => {
                let index = frame.u8()?;
                let value = frame.arg(usize::from(index))?;
                frame.push(value);
            }
            op::LDNULL => frame.push(StackValue::Obj(None)),
            op::LDC_I4_M1..=op::LDC_I4_8 => {
                frame.push(StackValue::I32(i32::from(opcode) - i32::from(op::LDC_I4_0)));
            }
            op::LDC_I4_S => {
                let value = frame.i8()?;
                frame.push(StackValue::I32(i32::from(value)));
            }
            op::LDC_I4 => {
                let value = frame.i32()?;
                frame.push(StackValue::I32(value));
            }
            op::LDSTR => {
                let token = frame.u32()?;
                let asm = frame.method.asm;
                let s = self.ldstr(asm, token)?;
                self.top().push(StackValue::Obj(Some(s)));
            }
            op::CALL => {
                let token = frame.u32()?;
                let asm = frame.method.asm;
                let callee = self.resolve_call(asm, token)?;
                self.call(callee)?;
            }
            op::RET => return self.ret(),
            op::PREFIX_FE => {
                let second = frame.u8()?;
                return Err(RuntimeError::not_implemented(format!("opcode 0xFE 0x{second:02X}")));
            }
            other => {
                return Err(RuntimeError::not_implemented(format!(
                    "opcode 0x{other:02X} ({})",
                    opcode_name(other)
                )));
            }
        }
        Ok(Flow::Continue)
    }

    fn string_type(&mut self) -> Result<TypeId> {
        self.types.corlib_type(&self.asms, "System", "String")
    }

    /// §III.4.16: a string object for a #US token, interned per (module, token).
    fn ldstr(&mut self, asm: AssemblyId, token: u32) -> Result<ObjRef> {
        if token >> 24 != 0x70 {
            return Err(RuntimeError::invalid_program(format!(
                "ldstr operand {token:#010x} is not a string token"
            )));
        }
        if let Some(&s) = self.strings.get(&(asm, token)) {
            return Ok(s);
        }
        let assembly = self.asms.get(asm);
        let units = assembly
            .model
            .user_string(token)
            .ok_or_else(|| assembly.bad_image(format!("ldstr token {token:#010x} has no #US entry")))?
            .to_vec();
        let string_type = self.string_type()?;
        let s = self.heap.alloc_string(string_type, &units)?;
        self.strings.insert((asm, token), s);
        Ok(s)
    }

    /// The `string[]` passed to `Main(string[] args)`.
    fn args_array(&mut self, args: &[String]) -> Result<ObjRef> {
        let string_type = self.string_type()?;
        let array_base = self.types.corlib_type(&self.asms, "System", "Array")?;
        let array_type = self.types.szarray(string_type, array_base);
        let mut elements = Vec::with_capacity(args.len());
        for arg in args {
            let units: Vec<u16> = arg.encode_utf16().collect();
            elements.push(Some(self.heap.alloc_string(string_type, &units)?));
        }
        self.heap.alloc_ref_array(array_type, &elements)
    }

    fn resolve_call(&self, asm: AssemblyId, token: u32) -> Result<MethodId> {
        let row = token & 0x00FF_FFFF;
        match token >> 24 {
            0x06 => {
                self.asms.get(asm).row(TableId::MethodDef, row)?;
                Ok(MethodId { asm, row })
            }
            0x0A => methods::resolve_member_ref(&self.asms, asm, row),
            0x2B => Err(RuntimeError::not_implemented("generic method calls (MethodSpec)")),
            _ => Err(RuntimeError::invalid_program(format!(
                "call operand {token:#010x} is not a method token"
            ))),
        }
    }

    /// §III.3.19: pops the arguments, then runs the callee's IL in a new
    /// frame or, for a body-less corlib method, its internal call.
    fn call(&mut self, callee: MethodId) -> Result<()> {
        let sig = methods::method_sig(&self.asms, callee)?;
        if sig.conv & conv::HAS_THIS != 0 {
            return Err(RuntimeError::not_implemented(format!(
                "instance method call {}",
                methods::method_key(&self.asms, callee)?
            )));
        }
        if sig.conv != conv::DEFAULT {
            return Err(RuntimeError::not_implemented(format!(
                "calling convention {:#04x} of {}",
                sig.conv,
                methods::method_key(&self.asms, callee)?
            )));
        }
        let argc = sig.params.len();
        let available = self.top().stack.len();
        if available < argc {
            return Err(RuntimeError::invalid_program(format!(
                "stack underflow: {} takes {argc} arguments, {available} on the stack",
                methods::method_name(&self.asms, callee)?
            )));
        }
        let args = self.top().stack.split_off(available - argc);
        let has_body = self.asms.get(callee.asm).row(TableId::MethodDef, callee.row)?
            [col::method_def::BODY]
            .as_body()
            .is_some();
        if has_body {
            return self.push_frame(callee, args, sig.ret != TypeSig::Void);
        }
        let key = methods::method_key(&self.asms, callee)?;
        if callee.asm != self.asms.corlib() {
            return Err(RuntimeError::not_implemented(format!("method without IL body {key}")));
        }
        let internal_call = internal_calls::lookup(&key)
            .ok_or_else(|| RuntimeError::not_implemented(format!("internal call {key}")))?;
        let string_type = self.string_type()?;
        let mut ctx = CallContext { heap: &mut self.heap, out: &mut *self.out, string_type };
        if let Some(value) = internal_call(&mut ctx, &args)? {
            self.top().push(value);
        }
        Ok(())
    }

    fn push_frame(&mut self, method: MethodId, args: Vec<StackValue>, returns_value: bool) -> Result<()> {
        if self.frames.len() >= MAX_FRAMES {
            return Err(RuntimeError::not_implemented(format!(
                "call depth above {MAX_FRAMES} (stack overflow; exceptions are not supported yet)"
            )));
        }
        let row = self.asms.get(method.asm).row(TableId::MethodDef, method.row)?;
        let Some(body) = row[col::method_def::BODY].as_body() else {
            return Err(RuntimeError::not_implemented(format!(
                "method without IL body {}",
                methods::method_name(&self.asms, method)?
            )));
        };
        let code: Rc<[u8]> = Rc::from(body.code.as_slice());
        self.frames.push(Frame { method, code, ip: 0, args, stack: Vec::new(), returns_value });
        Ok(())
    }

    /// §III.3.56: the evaluation stack must hold exactly the return value.
    fn ret(&mut self) -> Result<Flow> {
        let frame = self.top();
        let expected = usize::from(frame.returns_value);
        if frame.stack.len() != expected {
            return Err(RuntimeError::invalid_program(format!(
                "ret with {} values on the stack, expected {expected}",
                frame.stack.len()
            )));
        }
        let value = frame.stack.pop();
        self.frames.pop();
        match self.frames.last_mut() {
            None => Ok(Flow::Exit(value)),
            Some(caller) => {
                if let Some(v) = value {
                    caller.push(v);
                }
                Ok(Flow::Continue)
            }
        }
    }
}
```

- [ ] **Step 5: Add `run` and `run_files` to `lib.rs`**

`crates/runtime/src/lib.rs` becomes:
```rust
//! Clean-room CLI runtime: loads assemblies with the `metadata` crate and
//! interprets their IL (spec §5.3). Section references such as §III.3.38
//! point into ECMA-335, 6th edition.

pub mod error;
pub mod heap;
pub mod internal_calls;
pub mod interp;
pub mod loader;
pub mod methods;
pub mod opcodes;
pub mod types;
pub mod value;

#[cfg(test)]
mod testing;

use std::io::Write;
use std::path::Path;

pub use error::{Location, Result, RuntimeError};
pub use loader::{Assemblies, AssemblyId, LoadedAssembly};
pub use value::StackValue;

/// Runs the entry point of `entry` with `corlib` as `mscorlib`; returns the
/// exit code. Program output goes to `out`.
pub fn run(corlib: LoadedAssembly, entry: LoadedAssembly, args: &[String], out: &mut dyn Write) -> Result<i32> {
    let mut assemblies = Assemblies::new(corlib)?;
    let entry = assemblies.add(entry);
    interp::Interpreter::new(assemblies, out).run_entry(entry, args)
}

/// Loads `entry` (first, so its errors win) and `corlib` from disk and runs them.
pub fn run_files(entry: &Path, corlib: &Path, args: &[String], out: &mut dyn Write) -> Result<i32> {
    let entry = LoadedAssembly::from_file(entry)?;
    let corlib = LoadedAssembly::from_file(corlib)?;
    run(corlib, entry, args, out)
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p runtime`
Expected: all PASS (39 + 26 = 65), no warnings. `unbounded_recursion_is_reported` builds 10 000 frames; it should take well under a second.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/runtime
git commit -m "feat(runtime): IL interpreter with explicit frames and typed stack

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `clr` binary

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Create: `crates/clr/Cargo.toml`, `crates/clr/build.rs`, `crates/clr/src/main.rs`, `crates/clr/tests/cli.rs`

**Interfaces:**
- Consumes: `runtime::run_files`, `RuntimeError::exit_code` (Task 6).
- Produces:
  - Binary `clr`: `clr [-corlib <path>] <assembly.exe> [args...]`. Without `-corlib`, uses `mscorlib.dll` next to the binary. Success → exit code of `Main` (truncated to 0–255 by the OS); runtime error → `clr: <error>` on stderr and `RuntimeError::exit_code()`; usage error → `clr: <problem>` + usage line, exit 64.
  - cfgs `has_mcs` and `has_mono` for the `clr` crate's tests (used by Task 8).

- [ ] **Step 1: Scaffold the crate**

Workspace `Cargo.toml`:
```toml
[workspace]
resolver = "3"
members = ["crates/metadata", "crates/runtime", "crates/clr"]
```

`crates/clr/Cargo.toml`:
```toml
[package]
name = "clr"
version = "0.1.0"
edition = "2024"
description = "Runs CLI assemblies on the clean-room runtime"

[dependencies]
runtime = { path = "../runtime" }
```

`crates/clr/build.rs`:
```rust
//! Detects the Mono tools used by the program tests, so tests needing them
//! are ignored (visibly, with a reason) instead of silently passing.

use std::process::Command;

fn detect(tool: &str, cfg: &str) {
    let runs = Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if runs {
        println!("cargo::rustc-cfg={cfg}");
    } else {
        println!("cargo::warning={tool} not found on PATH: tests needing it will be ignored");
    }
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_mcs)");
    println!("cargo::rustc-check-cfg=cfg(has_mono)");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=PATH");
    detect("mcs", "has_mcs");
    detect("mono", "has_mono");
}
```

- [ ] **Step 2: Write the failing tests**

`crates/clr/src/main.rs` — tests and a stub `main` for now:
```rust
//! `clr [-corlib <path>] <assembly.exe> [args...]` — runs a CLI assembly on
//! the clean-room runtime (spec §5.3, exit codes §5.4).

fn main() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn assembly_and_trailing_args() {
        let o = parse_args(&strings(&["app.exe", "one", "-two"])).unwrap();
        assert_eq!(
            o,
            Options { corlib: None, assembly: PathBuf::from("app.exe"), args: strings(&["one", "-two"]) }
        );
    }

    #[test]
    fn corlib_option() {
        let o = parse_args(&strings(&["-corlib", "lib/mscorlib.dll", "app.exe"])).unwrap();
        assert_eq!(o.corlib, Some(PathBuf::from("lib/mscorlib.dll")));
        assert_eq!(o.assembly, PathBuf::from("app.exe"));
        assert!(o.args.is_empty());
    }

    #[test]
    fn usage_errors() {
        assert_eq!(parse_args(&[]), Err("missing assembly path".to_owned()));
        assert_eq!(parse_args(&strings(&["-corlib"])), Err("-corlib needs a path".to_owned()));
        assert_eq!(parse_args(&strings(&["-verbose", "app.exe"])), Err("unknown option -verbose".to_owned()));
    }

    #[test]
    fn default_corlib_sits_next_to_the_binary() {
        assert_eq!(default_corlib(Path::new("/opt/clr/bin/clr")), PathBuf::from("/opt/clr/bin/mscorlib.dll"));
    }
}
```

`crates/clr/tests/cli.rs`:
```rust
//! Error exits of the `clr` binary that need no Mono tools (spec §5.4).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn clr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_clr")).args(args).output().expect("run clr")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../metadata/tests/fixtures").join(name)
}

#[test]
fn no_arguments_is_a_usage_error() {
    let out = clr(&[]);
    assert_eq!(out.status.code(), Some(64));
    assert!(stderr(&out).contains("usage: clr [-corlib <path>] <assembly.exe> [args...]"), "{}", stderr(&out));
}

#[test]
fn missing_assembly_exits_66() {
    let out = clr(&["-corlib", "/nonexistent/mscorlib.dll", "/nonexistent/app.exe"]);
    assert_eq!(out.status.code(), Some(66));
    assert!(stderr(&out).starts_with("clr: /nonexistent/app.exe: "), "{}", stderr(&out));
}

#[test]
fn non_pe_assembly_exits_66() {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("not-a-pe.exe");
    std::fs::write(&path, b"hello").unwrap();
    let out = clr(&["-corlib", "/nonexistent/mscorlib.dll", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(66));
    assert!(
        stderr(&out).starts_with(&format!("clr: {}: bad image: ", path.display())),
        "{}",
        stderr(&out)
    );
}

#[test]
fn missing_corlib_exits_66() {
    let hello = fixture("hello.exe");
    let out = clr(&["-corlib", "/nonexistent/mscorlib.dll", hello.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(66));
    assert!(stderr(&out).starts_with("clr: /nonexistent/mscorlib.dll: "), "{}", stderr(&out));
}

#[test]
fn corlib_with_another_name_exits_66() {
    let hello = fixture("hello.exe");
    let lib = fixture("lib.dll");
    let out = clr(&["-corlib", lib.to_str().unwrap(), hello.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(66));
    assert!(stderr(&out).contains("expected assembly 'mscorlib', found 'lib'"), "{}", stderr(&out));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p clr`
Expected: compile errors in `main.rs` tests (`parse_args`, `Options`, `default_corlib` not found).

- [ ] **Step 4: Implement `main.rs`**

Replace `fn main() {}` with:
```rust
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const EXIT_USAGE: u8 = 64;
const USAGE: &str = "usage: clr [-corlib <path>] <assembly.exe> [args...]";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    corlib: Option<PathBuf>,
    assembly: PathBuf,
    /// Passed to `Main(string[] args)` verbatim, even if they start with `-`.
    args: Vec<String>,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut corlib = None;
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "-corlib" => {
                let path = args.get(i + 1).ok_or("-corlib needs a path")?;
                corlib = Some(PathBuf::from(path));
                i += 2;
            }
            option if option.starts_with('-') => return Err(format!("unknown option {option}")),
            _ => break,
        }
    }
    let assembly = args.get(i).ok_or("missing assembly path")?;
    Ok(Options { corlib, assembly: PathBuf::from(assembly), args: args[i + 1..].to_vec() })
}

/// `mscorlib.dll` next to the `clr` binary.
fn default_corlib(exe: &Path) -> PathBuf {
    exe.with_file_name("mscorlib.dll")
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_args(&argv) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("clr: {problem}\n{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let corlib = match options.corlib {
        Some(path) => path,
        None => match std::env::current_exe() {
            Ok(exe) => default_corlib(&exe),
            Err(e) => {
                eprintln!("clr: cannot locate mscorlib.dll next to clr: {e}");
                return ExitCode::from(runtime::error::EXIT_LOAD as u8);
            }
        },
    };
    let mut out = std::io::stdout().lock();
    match runtime::run_files(&options.assembly, &corlib, &options.args, &mut out) {
        // Process exit codes are 0-255 on Unix; `int Main` values wrap like on .NET.
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            let _ = std::io::Write::flush(&mut out);
            eprintln!("clr: {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p clr`
Expected: 4 unit tests + 5 cli tests PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add Cargo.toml Cargo.lock crates/clr
git commit -m "feat(clr): command-line runner with spec exit codes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Corlib sources, test programs and the config R harness

**Files:**
- Create: `corlib/README.md`, `corlib/AssemblyInfo.cs`, `corlib/System.cs`, `corlib/System.Collections.cs`, `corlib/System.Reflection.cs`, `corlib/System.Runtime.InteropServices.cs`
- Create: `tests/programs/` — `hello.cs`, `hello.expected`; `static_calls.cs`, `static_calls.expected`; `exit_code.cs`, `exit_code.expected`, `exit_code.exitcode`; `main_args.cs`, `main_args.expected`; `null_and_unicode.cs`, `null_and_unicode.expected`; `unsupported_opcode.cs`, `unsupported_opcode.expected`, `unsupported_opcode.exitcode`, `unsupported_opcode.stderr`; `missing_method.cs`, `missing_method.expected`, `missing_method.exitcode`, `missing_method.stderr`
- Create: `crates/clr/tests/programs.rs`
- Modify: `docs/superpowers/specs/2026-10-07-hello-world-toolchain-design.md` (§4 corlib line), `.github/workflows/ci.yml` (verify step)

**Interfaces:**
- Consumes: the `clr` binary and cfgs `has_mcs`/`has_mono` (Task 7).
- Produces: `corlib/` compiled by `mcs -nostdlib -target:library -nowarn:626 -out:mscorlib.dll corlib/AssemblyInfo.cs <other corlib/*.cs sorted>`; program sidecar convention: `<name>.expected` (exact stdout, required), `<name>.exitcode` (default 0), `<name>.stderr` (substring of stderr; marks a program as runtime-specific — skipped by the mono cross-check).

- [ ] **Step 1: Write the corlib sources**

`corlib/AssemblyInfo.cs`:
```csharp
// Must be the first file on the mcs command line: assembly attributes have
// to precede every other declaration (mcs error CS1730).
[assembly: System.Reflection.AssemblyVersion("4.0.0.0")]
```

`corlib/System.cs`:
```csharp
namespace System {
    public class Object { }

    public abstract class ValueType { }
    public abstract class Enum : ValueType { }

    public struct Void { }
    public struct Boolean { }
    public struct Char { }
    public struct SByte { }
    public struct Byte { }
    public struct Int16 { }
    public struct UInt16 { }
    public struct Int32 { }
    public struct UInt32 { }
    public struct Int64 { }
    public struct UInt64 { }
    public struct Single { }
    public struct Double { }
    public struct Decimal { }
    public struct IntPtr { }
    public struct UIntPtr { }
    public struct RuntimeFieldHandle { }
    public struct RuntimeTypeHandle { }

    public sealed class String { }
    public abstract class Array { }
    public abstract class Type { }
    public abstract class Delegate { }
    public abstract class MulticastDelegate : Delegate { }
    public class Exception { }

    public abstract class Attribute { }
    public sealed class ParamArrayAttribute : Attribute { }

    public interface IDisposable { }

    public static class Console {
        // No body: the runtime implements it as an internal call.
        public static extern void WriteLine(string value);
    }
}
```

`corlib/System.Collections.cs`:
```csharp
namespace System.Collections {
    public interface IEnumerator { }
    public interface IEnumerable { }
}
```

`corlib/System.Reflection.cs`:
```csharp
namespace System.Reflection {
    public sealed class AssemblyVersionAttribute : System.Attribute {
        public AssemblyVersionAttribute(string version) { }
    }
}
```

`corlib/System.Runtime.InteropServices.cs`:
```csharp
namespace System.Runtime.InteropServices {
    public sealed class OutAttribute : System.Attribute { }
}
```

`corlib/README.md`:
````markdown
# corlib — our `mscorlib` 4.0.0.0

C# source of the runtime's core library. Until our own compiler can build it
(stage 4), it is compiled with Mono's `mcs`:

```sh
mcs -nostdlib -target:library -nowarn:626 -out:mscorlib.dll \
    corlib/AssemblyInfo.cs corlib/System.cs corlib/System.Collections.cs \
    corlib/System.Reflection.cs corlib/System.Runtime.InteropServices.cs
```

- `AssemblyInfo.cs` must come first (mcs error CS1730).
- `-nowarn:626`: body-less `extern` methods are intentional — a method of
  `mscorlib` without IL is an **internal call** implemented in Rust
  (`crates/runtime/src/internal_calls.rs`). mcs emits them with RVA 0 and no
  `InternalCall` flag; RVA 0 is the only marker the runtime relies on.

## Why these types (measured in stage 2)

`mcs -nostdlib` refuses to compile anything until the corlib defines these
predefined types (error CS0518 otherwise):

- `System`: `Object`, `ValueType`, `Enum`, `Void`, `Boolean`, `Char`,
  `SByte`, `Byte`, `Int16`, `UInt16`, `Int32`, `UInt32`, `Int64`, `UInt64`,
  `Single`, `Double`, `Decimal`, `IntPtr`, `UIntPtr`, `String`, `Array`,
  `Type`, `Attribute`, `ParamArrayAttribute`, `Delegate`,
  `MulticastDelegate`, `Exception`, `RuntimeFieldHandle`,
  `RuntimeTypeHandle`, `IDisposable`
- `System.Collections`: `IEnumerator`, `IEnumerable` (empty interfaces suffice)
- `System.Runtime.InteropServices`: `OutAttribute`

Added on purpose:

- `System.Reflection.AssemblyVersionAttribute` — gives the assembly its
  identity `mscorlib, Version=4.0.0.0`.
- `System.Console.WriteLine(string)` — the only internal call so far.

Keep the source inside the subset our own compiler will handle: classes,
structs, empty interfaces, `abstract`/`sealed`/`static`, `extern` methods,
one attribute class with a string constructor.
````

- [ ] **Step 2: Record the measured list in the spec**

In `docs/superpowers/specs/2026-10-07-hello-world-toolchain-design.md` §4 "In scope", replace the whole "**Corlib types:**" bullet with:
```markdown
- **Corlib types:** measured in stage 2 — the 33 predefined types
  `mcs -nostdlib` requires (primitives, `Object`, `ValueType`, `Enum`,
  `String`, `Array`, `Type`, `Attribute`, `ParamArrayAttribute`,
  `Delegate`, `MulticastDelegate`, `Exception`, runtime handles,
  `IDisposable`, `IEnumerator`, `IEnumerable`, `OutAttribute`) plus
  `AssemblyVersionAttribute` and `System.Console.WriteLine(string)`. The
  authoritative list and the reasons live in `corlib/README.md`.
```

- [ ] **Step 3: Write the test programs**

`tests/programs/hello.cs`:
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
`tests/programs/hello.expected` — exactly the line `Hello, World!` followed by one newline.

`tests/programs/static_calls.cs`:
```csharp
using System;

static class Program {
    static string Name() {
        return "Ada";
    }

    static void Greet(string who, string greeting) {
        Console.WriteLine(greeting);
        Console.WriteLine(who);
    }

    static void Main() {
        Greet(Name(), "Hi");
    }
}
```
`tests/programs/static_calls.expected` — two lines: `Hi`, `Ada` (each followed by a newline).

`tests/programs/exit_code.cs`:
```csharp
using System;

static class Program {
    static int Main() {
        Console.WriteLine("exiting with 3");
        return 3;
    }
}
```
`tests/programs/exit_code.expected` — `exiting with 3` + newline. `tests/programs/exit_code.exitcode` — `3` + newline.

`tests/programs/main_args.cs`:
```csharp
using System;

static class Program {
    static void Main(string[] args) {
        Console.WriteLine("args accepted");
    }
}
```
`tests/programs/main_args.expected` — `args accepted` + newline.

`tests/programs/null_and_unicode.cs`:
```csharp
using System;

static class Program {
    static void Main() {
        Console.WriteLine((string)null);
        Console.WriteLine("Grüße, 世界 😀");
    }
}
```
`tests/programs/null_and_unicode.expected` — an empty line, then `Grüße, 世界 😀` + newline (file bytes: `\nGrüße, 世界 😀\n`, UTF-8).

`tests/programs/unsupported_opcode.cs`:
```csharp
using System;

static class Program {
    static void Main() {
        Console.WriteLine("before");
        object o = new object();
        Console.WriteLine("unreachable");
    }
}
```
`tests/programs/unsupported_opcode.expected` — `before` + newline.
`tests/programs/unsupported_opcode.exitcode` — `70` + newline.
`tests/programs/unsupported_opcode.stderr` — the line
`clr: not implemented: opcode 0x73 (newobj) in Program::Main @IL_000a`
(mcs emits `ldstr` (5 bytes) + `call` (5 bytes) first, so `newobj` is at offset 10.)

`tests/programs/missing_method.cs`:
```csharp
using System;

static class Program {
    static void Main() {
        Console.Write("x");
    }
}
```
`tests/programs/missing_method.expected` — empty file (0 bytes).
`tests/programs/missing_method.exitcode` — `70` + newline.
`tests/programs/missing_method.stderr` — the line
`clr: not implemented: method System.Console::Write(string) in Program::Main @IL_0005`

Write the `.expected` files with `printf` to control newlines exactly, e.g.:
```bash
printf 'Hello, World!\n' > tests/programs/hello.expected
printf 'Hi\nAda\n' > tests/programs/static_calls.expected
printf 'exiting with 3\n' > tests/programs/exit_code.expected
printf '3\n' > tests/programs/exit_code.exitcode
printf 'args accepted\n' > tests/programs/main_args.expected
printf '\nGrüße, 世界 😀\n' > tests/programs/null_and_unicode.expected
printf 'before\n' > tests/programs/unsupported_opcode.expected
printf '70\n' > tests/programs/unsupported_opcode.exitcode
printf 'clr: not implemented: opcode 0x73 (newobj) in Program::Main @IL_000a\n' > tests/programs/unsupported_opcode.stderr
: > tests/programs/missing_method.expected
printf '70\n' > tests/programs/missing_method.exitcode
printf 'clr: not implemented: method System.Console::Write(string) in Program::Main @IL_0005\n' > tests/programs/missing_method.stderr
```

- [ ] **Step 4: Write the harness**

`crates/clr/tests/programs.rs`:
```rust
//! Program tests, spec §6 config R: programs compiled by `mcs` against
//! Mono's mscorlib, corlib compiled by `mcs -nostdlib`, run on our `clr`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Passed to every program, so `Main(string[] args)` gets a real array.
const PROGRAM_ARGS: [&str; 2] = ["one", "two"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Program {
    name: String,
    source: PathBuf,
    expected_stdout: String,
    expected_exit: i32,
    /// Substring of stderr; only programs that fail on our runtime have one.
    expected_stderr: Option<String>,
}

fn programs() -> Vec<Program> {
    let dir = repo_root().join("tests/programs");
    let mut list: Vec<Program> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "cs"))
        .map(|source| {
            let read = |ext: &str| std::fs::read_to_string(source.with_extension(ext)).ok();
            Program {
                name: source.file_stem().unwrap().to_string_lossy().into_owned(),
                expected_stdout: read("expected")
                    .unwrap_or_else(|| panic!("{} has no .expected file", source.display())),
                expected_exit: read("exitcode").map_or(0, |s| s.trim().parse().unwrap()),
                expected_stderr: read("stderr").map(|s| s.trim_end().to_owned()),
                source,
            }
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    assert!(!list.is_empty(), "no programs in {}", dir.display());
    list
}

fn mcs(args: &[String]) {
    let out = Command::new("mcs").args(args).output().expect("run mcs");
    assert!(
        out.status.success(),
        "mcs {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Builds `corlib/*.cs` into `<dir>/mscorlib.dll`; AssemblyInfo.cs goes first (CS1730).
fn build_corlib(dir: &Path) -> PathBuf {
    let corlib_dir = repo_root().join("corlib");
    let first = corlib_dir.join("AssemblyInfo.cs");
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&corlib_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "cs") && *path != first)
        .collect();
    sources.sort();
    sources.insert(0, first);
    let out = dir.join("mscorlib.dll");
    let mut args: Vec<String> = ["-nostdlib", "-target:library", "-nowarn:626"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    args.push(format!("-out:{}", out.display()));
    args.extend(sources.iter().map(|s| s.display().to_string()));
    mcs(&args);
    out
}

/// Compiles a program against Mono's own mscorlib (that is config R).
fn compile(program: &Program, dir: &Path) -> PathBuf {
    let exe = dir.join(format!("{}.exe", program.name));
    mcs(&[format!("-out:{}", exe.display()), program.source.display().to_string()]);
    exe
}

#[cfg_attr(not(has_mcs), ignore = "mcs not installed: config R skipped")]
#[test]
fn programs_run_on_clr() {
    let dir = scratch("config-r");
    let corlib = build_corlib(&dir);
    let mut failures = Vec::new();
    for program in programs() {
        let exe = compile(&program, &dir);
        let out = Command::new(env!("CARGO_BIN_EXE_clr"))
            .arg("-corlib")
            .arg(&corlib)
            .arg(&exe)
            .args(PROGRAM_ARGS)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let name = &program.name;
        if stdout != program.expected_stdout {
            failures.push(format!("{name}: stdout {stdout:?}, expected {:?}", program.expected_stdout));
        }
        if out.status.code() != Some(program.expected_exit) {
            failures.push(format!(
                "{name}: exit {:?}, expected {} (stderr: {stderr})",
                out.status.code(),
                program.expected_exit
            ));
        }
        match &program.expected_stderr {
            Some(want) if !stderr.contains(want.as_str()) => {
                failures.push(format!("{name}: stderr {stderr:?} lacks {want:?}"));
            }
            None if !stderr.is_empty() => failures.push(format!("{name}: unexpected stderr {stderr:?}")),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Validates the .expected/.exitcode files themselves on Mono, for every
/// program that is meant to succeed.
#[cfg_attr(not(all(has_mcs, has_mono)), ignore = "mcs/mono not installed: expected-file check skipped")]
#[test]
fn expected_files_match_mono() {
    let dir = scratch("mono-check");
    for program in programs().into_iter().filter(|p| p.expected_stderr.is_none()) {
        let exe = compile(&program, &dir);
        let out = Command::new("mono")
            .arg(&exe)
            .args(PROGRAM_ARGS)
            .env("LANG", "C.UTF-8")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), program.expected_stdout, "{} on mono", program.name);
        assert_eq!(out.status.code(), Some(program.expected_exit), "{} exit code on mono", program.name);
    }
}
```

- [ ] **Step 5: Make the CI oracle check cover `mcs`**

In `.github/workflows/ci.yml`, change the "Verify Mono oracle is available" step's `run` line to:
```yaml
        run: test -f /usr/lib/mono/4.5/mscorlib.dll && mono --version && mcs --version
```

- [ ] **Step 6: Run the harness**

Run: `cargo test -p clr --test programs`
Expected: `programs_run_on_clr` and `expected_files_match_mono` PASS (not ignored). If a program fails, the assertion lists every mismatch; fix the runtime (with a unit test reproducing it in `crates/runtime`), never the `.expected` file — unless `expected_files_match_mono` shows the expected file itself is wrong.

- [ ] **Step 7: Run everything**

Run: `cargo fmt --all --check && cargo test --workspace`
Expected: all PASS, **0 ignored**, no warnings.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add corlib tests/programs crates/clr/tests/programs.rs docs/superpowers/specs/2026-10-07-hello-world-toolchain-design.md .github/workflows/ci.yml
git commit -m "feat: corlib sources and config R program tests

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Stage 2 completion check

Stage 2 of the spec (§7.2) is done when, on this machine:

- `cargo test --workspace` passes with **0 ignored** tests,
- `programs_run_on_clr` passes — in particular `hello` prints `Hello, World!` on our `clr` with our corlib,
- `expected_files_match_mono` passes.

The next plan (stage 3: compiler) builds on `metadata::MetadataModel` + `write_assembly`, the `col` constants, `MethodBody::normalized()` (stage 1) and the `tests/programs` harness, which gains config C.
