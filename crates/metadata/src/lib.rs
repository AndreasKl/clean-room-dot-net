//! ECMA-335 metadata: PE/COFF images, CLI header, heaps, tables and signatures.
//!
//! Section references such as §II.23.2 point into ECMA-335, 6th edition,
//! which is the authoritative specification for everything in this crate.

pub mod bytes;
pub mod error;

pub use error::{Error, Result};
