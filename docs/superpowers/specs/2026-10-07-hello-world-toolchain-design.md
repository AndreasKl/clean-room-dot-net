# Hello-World Toolchain — Design

Date: 2026-10-07
Status: Approved in brainstorming, pending written-spec review

## 1. Goal

First milestone of a clean-room implementation of the .NET Framework for Linux:
compile a C# "Hello, World!" console program with **our own C# compiler** and run
it on **our own runtime** against **our own corlib**.

```
hello.cs ──(our csc)──▶ hello.exe (ECMA-335 PE/IL) ──(our clr + our mscorlib.dll)──▶ "Hello, World!"
```

### Success criterion

Without `mcs`, `mono`, or `dotnet` involved:

1. `csc -nostdlib -t:library corlib/**/*.cs -out:mscorlib.dll` succeeds,
2. `csc hello.cs -r:mscorlib.dll -out:hello.exe` succeeds,
3. `clr -corlib mscorlib.dll hello.exe` prints `Hello, World!\n` and exits 0.

## 2. Constraints

- **Clean room.** Allowed: Microsoft code under permissive licenses (e.g. MIT
  dotnet/runtime, dotnet/roslyn), Mono and its libraries. Forbidden: Microsoft
  proprietary code, and decompiling any library or application. Disassembling
  assemblies *we* produced (our own output) is permitted for debugging.
- **Spec of record:** ECMA-335 (6th edition) is the authoritative reference for
  file format and execution semantics. MIT-licensed dotnet/runtime and Mono
  sources may be consulted for comparison.
- **Language:** Rust (stable, via rustup) for compiler and runtime.
- **Corlib identity:** `mscorlib`, version `4.0.0.0` (.NET Framework 4.x shape).
- **Corlib source:** C#, compiled by our own compiler (temporarily by `mcs`
  during bootstrap, see §7).

## 3. Repository layout

```
clean-room-dot-net/
├── Cargo.toml              # workspace
├── crates/
│   ├── metadata/           # ECMA-335: PE/COFF, CLI header, heaps, tables, signatures — read + write
│   ├── compiler/           # lib: lexer → parser → binder → IL emitter
│   ├── runtime/            # lib: loader, type system, IL interpreter, internal calls
│   ├── csc/                # bin: compiler CLI
│   └── clr/                # bin: runtime CLI
├── corlib/                 # C# source of mscorlib 4.0.0.0
├── tests/programs/         # *.cs programs + *.expected output
└── docs/
```

Dependency direction: `csc → compiler → metadata`, `clr → runtime → metadata`.
`compiler` and `runtime` never depend on each other.

## 4. Scope

### In scope

- **C# subset:** `using` directives, `namespace` declarations (block form),
  `class` declarations (incl. `static`), `public`/`private`/`internal` modifiers,
  `static void Main()` and `static void Main(string[] args)`, `static int Main(...)`
  with `return <int literal>;`, string literals (regular, with standard escapes),
  integer literals, static method calls with arguments, and `static extern`
  method declarations (no body).
- **Corlib subset (needed by stage 4):** to compile `corlib/`, the compiler additionally supports `struct`, empty `interface`s, `abstract`/`sealed` modifiers, a base-class clause, empty-bodied instance constructors with parameters, and the assembly attribute `[assembly: System.Reflection.AssemblyVersion("…")]` (which sets the Assembly table version; no CustomAttribute row is needed). These constructs are compiled to metadata only — no instance calls are executed.
- **IL opcodes:** `nop`, `ldstr`, `call`, `ret`, `ldarg.*`, `ldnull`,
  `ldc.i4.*`. Anything else is added only if `mcs` output for the test programs
  requires it.
- **Metadata tables:** Module, TypeRef, TypeDef, Field, MethodDef, Param,
  MemberRef, CustomAttribute, Assembly, AssemblyRef. Heaps: `#~`, `#Strings`,
  `#US`, `#Blob`, `#GUID`.
- **Corlib types:** measured in stage 2 — the 33 predefined types
  `mcs -nostdlib` requires (primitives, `Object`, `ValueType`, `Enum`,
  `String`, `Array`, `Type`, `Attribute`, `ParamArrayAttribute`,
  `Delegate`, `MulticastDelegate`, `Exception`, runtime handles,
  `IDisposable`, `IEnumerator`, `IEnumerable`, `OutAttribute`) plus
  `AssemblyVersionAttribute` and `System.Console.WriteLine(string)`. The
  authoritative list and the reasons live in `corlib/README.md`.

### Out of scope (this milestone)

JIT, real garbage collection, exceptions, generics, boxing, value-type
instances, instance methods/fields, virtual dispatch, P/Invoke, attributes (other than `AssemblyVersion`) and
enums in source, threading, strong-name verification.

### Forward-compatibility guardrails

Not implemented now, but the design must not preclude them:

- Managed references are an opaque `ObjRef` type; no raw pointers escape the
  heap module. (Precise GC later.)
- The evaluation stack is typed. (Stack maps for GC and Cranelift JIT later.)
- The managed call stack is an explicit `Vec<Frame>`, not Rust recursion.
  (Stack walking for GC and exceptions later.)
- Every object has a header pointing to its runtime type. (GC, virtual
  dispatch, `GetType` later.)

## 5. Components

### 5.1 `metadata` crate

Single home for all PE/metadata format knowledge.

- **Reader:** PE/COFF headers → CLI header → metadata root → stream headers →
  heaps → tables. Coded/simple index widths computed from row counts and heap
  size flags (ECMA-335 §II.24.2.6). Decodes method bodies (tiny and fat headers,
  ignoring exception clauses for now) and method/field/local signatures.
- **Writer:** takes a logical model (rows + method bodies), produces correctly
  sorted tables, deduplicated heaps, and a PE32 image with one `.text` section,
  CLI header, and entry-point token. No native entry stub or import table unless
  the `mono` oracle requires it.
- **Model:** one logical representation shared by reader and writer, so
  read → write → read is testable for equality.

### 5.2 Compiler (`compiler` + `csc`)

- **Lexer → parser → AST.** Hand-written recursive descent, no generator.
  Tokens and AST nodes carry source spans.
- **Binder.** Resolves names against
  (a) referenced assemblies, read via `metadata`, or
  (b) in `-nostdlib` mode, types declared in the source itself, which then
      supplies `System.Object` etc.
  Produces bound method bodies with resolved method/type references.
- **Emitter.** Bound methods → IL bytes + metadata rows → `metadata` writer.
  `static extern` methods are emitted with RVA 0 and ImplFlags
  `InternalCall`.
- **CLI:** `csc [-nostdlib] [-t:exe|library] [-r:<path>]... -out:<path> <files>...`
- **Diagnostics:** `file:line:col: error CSxxxx: message`, using Roslyn error
  codes where one applies. Never panics on invalid input; exit code 1 on
  errors.

### 5.3 Runtime (`runtime` + `clr`)

- **Loader.** Reads the entry assembly via `metadata`. Resolves
  `AssemblyRef mscorlib` to our corlib, located via `-corlib <path>` or next
  to the `clr` binary. Binding matches **name only** (version and public key
  token ignored), so `mcs`-compiled programs bind to our corlib.
- **Type system.** Types loaded lazily. TypeRef → TypeDef and MemberRef →
  MethodDef resolved across assemblies by name and signature. Each loaded type
  has a runtime type descriptor (name, base type, methods).
- **Interpreter.** Starts at the CLI header's entry-point token. Frame = method,
  IP, args, locals, typed eval stack
  (`I32 | I64 | F64 | NativeInt | ObjRef | ManagedPtr`). Managed calls push a
  new `Frame` onto an explicit stack.
- **Heap.** Bump allocator, never frees. Object header → runtime type.
  Strings use CLR layout: header, `length: i32`, UTF-16 code units. `ldstr`
  results are interned per (module, token).
- **Internal calls.** Any method in `mscorlib` with RVA 0 is an internal call,
  looked up by full name and signature in a static Rust table, e.g.
  `System.Console::WriteLine(string)` → writes UTF-16 → UTF-8 to stdout plus
  `\n`. This works for both our compiler's output (InternalCall flag set) and
  `mcs -nostdlib` output (body-less `extern`, warning CS0626).
- **CLI:** `clr [-corlib <path>] <assembly.exe> [args...]`

### 5.4 Error handling (runtime)

| Situation | Behaviour | Exit code |
|---|---|---|
| `void Main` completes | — | 0 |
| `int Main` completes | — | return value |
| File missing / not a valid PE / bad metadata | `clr: <path>: <reason>` on stderr | 66 |
| Unsupported opcode, missing method, missing internal call | `clr: not implemented: <what> in <Type::Method> @IL_xxxx` on stderr | 70 |
| IL that violates ECMA-335 (stack underflow, wrong value kind, bad token table, …) | clr: invalid program: <what> in <Type::Method> @IL_xxxx on stderr | 70 |

## 6. Testing

- **Unit tests per crate** (`cargo test`): lexer, parser, binder; heap and
  signature encoding; table index-width computation; individual opcodes with
  hand-built method bodies.
- **Metadata round-trip:** `mcs`-compiled assemblies read → write → read yield
  equal logical models. Mono's installed `mscorlib.dll` is a large read-only
  fixture for the reader.
- **Program tests:** a Rust test harness runs every `tests/programs/*.cs` in
  these configurations and compares stdout byte-for-byte with `*.expected`:

| Config | Compiler | Corlib built by | Runtime | From stage |
|---|---|---|---|---|
| R (runtime check) | `mcs` | `mcs -nostdlib` | ours | 2 |
| C (compiler check) | ours | Mono's mscorlib | `mono` | 3 |
| E2E | ours | ours | ours | 4 |

  If `mcs`/`mono` are not installed, configs R and C are **skipped with a
  visible message**, never silently counted as passed.

## 7. Stages

Each stage has an objective completion criterion.

1. **metadata** — reader parses `mcs`-compiled `hello.exe` and Mono's
   `mscorlib.dll`; round-trip tests pass.
2. **runtime** — config R prints `Hello, World!`.
3. **compiler** — config C prints `Hello, World!`.
4. **loop closed** — success criterion of §1 met; `mcs` no longer needed to
   build corlib or test programs (it remains an optional oracle).

CI (GitHub Actions, Ubuntu + `mono-devel`, running `cargo test` incl. configs
R and C) is part of the implementation plan.

## 8. Environment (verified 2026-10-07)

- Rust 1.99.0 / cargo 1.99.0 (rustup)
- Mono 6.14.1 with `mcs`
- .NET SDK 10.0.112 (available, not required)
