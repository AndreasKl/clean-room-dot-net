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

Keep the source inside the corlib subset defined in the spec (§4 "Corlib subset"), so our own compiler can build it in stage 4.
