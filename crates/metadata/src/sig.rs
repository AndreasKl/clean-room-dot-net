//! Signature blobs (§II.23.2).

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
    Array {
        elem: Box<TypeSig>,
        rank: u32,
        sizes: Vec<u32>,
        lo_bounds: Vec<i32>,
    },
    GenericInst {
        value_type: bool,
        ty: TypeDefOrRef,
        args: Vec<TypeSig>,
    },
    FnPtr(Box<MethodSig>),
    Modified {
        required: bool,
        modifier: TypeDefOrRef,
        ty: Box<TypeSig>,
    },
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
        Error::BadImage(format!(
            "invalid TypeDefOrRef tag in signature value {raw:#x}"
        ))
    })?;
    Ok(TypeDefOrRef {
        table,
        row: raw >> 2,
    })
}

fn write_tdor(w: &mut ByteWriter, t: TypeDefOrRef) -> Result<()> {
    let tag = TDOR_TABLES
        .iter()
        .position(|x| *x == t.table)
        .ok_or_else(|| Error::Malformed(format!("{:?} is not a TypeDefOrRef table", t.table)))?;
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
            let sizes = (0..num_sizes)
                .map(|_| r.compressed_u32())
                .collect::<Result<_>>()?;
            let num_lo = r.compressed_u32()?;
            let lo_bounds = (0..num_lo)
                .map(|_| r.compressed_i32())
                .collect::<Result<_>>()?;
            TypeSig::Array {
                elem,
                rank,
                sizes,
                lo_bounds,
            }
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
            let args = (0..count)
                .map(|_| read_type(r, depth + 1))
                .collect::<Result<_>>()?;
            TypeSig::GenericInst {
                value_type,
                ty,
                args,
            }
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
        TypeSig::Array {
            elem,
            rank,
            sizes,
            lo_bounds,
        } => {
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
        TypeSig::GenericInst {
            value_type,
            ty,
            args,
        } => {
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
        TypeSig::Modified {
            required,
            modifier,
            ty,
        } => {
            w.u8(if *required { e::CMOD_REQD } else { e::CMOD_OPT });
            write_tdor(w, *modifier)?;
            write_type(w, ty)?;
        }
    }
    Ok(())
}

fn read_method(r: &mut ByteReader, depth: usize) -> Result<MethodSig> {
    let conv = r.u8()?;
    let generic_params = if conv & conv::GENERIC != 0 {
        r.compressed_u32()?
    } else {
        0
    };
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
    Ok(MethodSig {
        conv,
        generic_params,
        ret,
        params,
        sentinel,
    })
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
        return Err(Error::BadImage(format!(
            "field signature starts with {lead:#04x}"
        )));
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
        return Err(Error::BadImage(format!(
            "local signature starts with {lead:#04x}"
        )));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn type_ref(row: u32) -> TypeDefOrRef {
        TypeDefOrRef {
            table: TableId::TypeRef,
            row,
        }
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
            MethodSig {
                conv: 0,
                generic_params: 0,
                ret: TypeSig::Void,
                params: vec![TypeSig::String],
                sentinel: None,
            },
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
            MethodSig {
                conv: conv::VARARG,
                generic_params: 0,
                ret: TypeSig::Void,
                params: vec![TypeSig::I4, TypeSig::String],
                sentinel: Some(1),
            },
        );
    }

    #[test]
    fn type_specs() {
        // List<int32> where List is TypeRef row 3: tag 1 → (3 << 2) | 1 = 0x0D.
        let generic = [0x15, 0x12, 0x0D, 0x01, 0x08];
        let ty = decode_type_spec(&generic).unwrap();
        assert_eq!(
            ty,
            TypeSig::GenericInst {
                value_type: false,
                ty: type_ref(3),
                args: vec![TypeSig::I4]
            }
        );
        assert_eq!(encode_type_spec(&ty).unwrap(), generic);

        // int32[,] with rank 2, no sizes, two zero lower bounds.
        let array = [0x14, 0x08, 0x02, 0x00, 0x02, 0x00, 0x00];
        let ty = decode_type_spec(&array).unwrap();
        assert_eq!(
            ty,
            TypeSig::Array {
                elem: Box::new(TypeSig::I4),
                rank: 2,
                sizes: vec![],
                lo_bounds: vec![0, 0]
            }
        );
        assert_eq!(encode_type_spec(&ty).unwrap(), array);

        // Negative lower bound uses the signed compressed form.
        let negative = [0x14, 0x08, 0x01, 0x01, 0x05, 0x01, 0x7B];
        let ty = decode_type_spec(&negative).unwrap();
        assert_eq!(
            ty,
            TypeSig::Array {
                elem: Box::new(TypeSig::I4),
                rank: 1,
                sizes: vec![5],
                lo_bounds: vec![-3]
            }
        );
        assert_eq!(encode_type_spec(&ty).unwrap(), negative);
    }

    #[test]
    fn field_with_required_modifier() {
        let blob = [0x06, 0x1F, 0x05, 0x08];
        let ty = decode_field_sig(&blob).unwrap();
        assert_eq!(
            ty,
            TypeSig::Modified {
                required: true,
                modifier: type_ref(1),
                ty: Box::new(TypeSig::I4)
            }
        );
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
                TypeSig::ValueType(TypeDefOrRef {
                    table: TableId::TypeDef,
                    row: 2
                }),
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
            TypeSig::FnPtr(Box::new(MethodSig {
                conv: 0,
                generic_params: 0,
                ret: TypeSig::I4,
                params: vec![],
                sentinel: None
            }))
        );
        assert_eq!(encode_type_spec(&ty).unwrap(), blob);
    }

    #[test]
    fn malformed_signatures_are_errors() {
        assert!(decode_type_spec(&[0x50]).is_err(), "unknown element type");
        assert!(decode_type_spec(&[0x08, 0x08]).is_err(), "trailing bytes");
        assert!(decode_field_sig(&[0x07, 0x08]).is_err(), "wrong lead byte");
        assert!(decode_local_sig(&[0x06, 0x00]).is_err(), "wrong lead byte");
        assert!(
            decode_type_spec(&[0x12, 0x03]).is_err(),
            "TypeDefOrRef tag 3"
        );
        assert!(
            decode_method_sig(&[0x00, 0x05, 0x01]).is_err(),
            "truncated params"
        );
    }

    #[test]
    fn deeply_nested_signature_is_an_error() {
        let mut blob = vec![0x0F; 10_000]; // Ptr(Ptr(Ptr(...)))
        blob.push(0x08);
        assert!(decode_type_spec(&blob).is_err());
    }
}
