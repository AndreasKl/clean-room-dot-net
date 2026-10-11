//! Runtime type descriptors (spec §5.3 "Type system").

/// Index of a loaded type in the `TypeSystem`; stored in every object header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);
