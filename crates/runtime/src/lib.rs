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
