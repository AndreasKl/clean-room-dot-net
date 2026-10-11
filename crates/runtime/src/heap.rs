//! Bump-allocated managed heap (spec §5.3 "Heap"). Objects live in one
//! growable byte arena and are addressed by `ObjRef` offsets, so no raw
//! pointer escapes this module and a precise GC can replace the allocator.

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
        Self {
            arena: vec![0; ALIGN],
        }
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
        Ok(ObjRef(
            NonZeroU32::new(offset as u32).expect("offset 0 is reserved"),
        ))
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
        for s in [
            utf16(""),
            utf16("Hello, World!"),
            utf16("Grüße 😀"),
            vec![0xD800],
        ] {
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
        let arr = heap
            .alloc_ref_array(ARRAY, &[Some(s), None, Some(s)])
            .unwrap();
        assert_eq!(heap.type_of(arr), ARRAY);
        assert_eq!(heap.ref_array_elements(arr), vec![Some(s), None, Some(s)]);
        let empty = heap.alloc_ref_array(ARRAY, &[]).unwrap();
        assert!(heap.ref_array_elements(empty).is_empty());
    }
}
