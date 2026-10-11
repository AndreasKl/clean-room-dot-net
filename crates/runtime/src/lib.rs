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
pub fn run(
    corlib: LoadedAssembly,
    entry: LoadedAssembly,
    args: &[String],
    out: &mut dyn Write,
) -> Result<i32> {
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
