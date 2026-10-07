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
