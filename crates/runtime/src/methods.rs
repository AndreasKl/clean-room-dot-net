//! Methods: ownership, signatures, internal-call keys and MemberRef →
//! MethodDef resolution by name and signature (spec §5.3 "Type system").

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
    let name = text(
        assembly.row(TableId::MethodDef, method.row)?,
        col::method_def::NAME,
    );
    let owner = match assembly.row(
        TableId::TypeDef,
        owner_type_row(&assembly.model, method.row),
    ) {
        Ok(t) => qualified(
            text(t, col::type_def::NAMESPACE),
            text(t, col::type_def::NAME),
        ),
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
    sig.params
        .iter()
        .map(|p| render_type(assembly, p))
        .collect::<Vec<_>>()
        .join(",")
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
            format!(
                "{}[{}]",
                render_type(assembly, elem),
                ",".repeat((*rank as usize).saturating_sub(1))
            )
        }
        T::Ptr(e) => format!("{}*", render_type(assembly, e)),
        T::ByRef(e) => format!("{}&", render_type(assembly, e)),
        T::Pinned(e) | T::Modified { ty: e, .. } => render_type(assembly, e),
        T::Var(n) => format!("!{n}"),
        T::MVar(n) => format!("!!{n}"),
        T::GenericInst { ty, args, .. } => format!(
            "{}<{}>",
            type_name(assembly, *ty),
            args.iter()
                .map(|a| render_type(assembly, a))
                .collect::<Vec<_>>()
                .join(",")
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
        return Err(RuntimeError::not_implemented(format!(
            "field reference {name}"
        )));
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
    let first = type_def[col::type_def::METHOD_LIST]
        .as_index()
        .map_or(method_count + 1, |(_, s)| s);
    let end = target_assembly
        .model
        .table(TableId::TypeDef)
        .get(type_row as usize)
        .and_then(|next| next[col::type_def::METHOD_LIST].as_index())
        .map_or(method_count + 1, |(_, s)| s);
    for candidate_row in first..end.min(method_count + 1) {
        let candidate = MethodId {
            asm: target,
            row: candidate_row,
        };
        if text(
            target_assembly.row(TableId::MethodDef, candidate_row)?,
            col::method_def::NAME,
        ) != name
        {
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
fn type_identity(
    asms: &Assemblies,
    asm: AssemblyId,
    t: TypeDefOrRef,
) -> Result<(AssemblyId, String, String)> {
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
        other => Err(RuntimeError::not_implemented(format!(
            "{other:?} in a signature"
        ))),
    }
}

fn same_type(
    asms: &Assemblies,
    a: (AssemblyId, TypeDefOrRef),
    b: (AssemblyId, TypeDefOrRef),
) -> Result<bool> {
    Ok(type_identity(asms, a.0, a.1)? == type_identity(asms, b.0, b.1)?)
}

fn sig_matches(
    asms: &Assemblies,
    a: (AssemblyId, &MethodSig),
    b: (AssemblyId, &MethodSig),
) -> Result<bool> {
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
fn type_matches(
    asms: &Assemblies,
    a: (AssemblyId, &TypeSig),
    b: (AssemblyId, &TypeSig),
) -> Result<bool> {
    use TypeSig as T;
    let ((ia, x), (ib, y)) = (a, b);
    Ok(match (x, y) {
        (T::Class(p), T::Class(q)) | (T::ValueType(p), T::ValueType(q)) => {
            same_type(asms, (ia, *p), (ib, *q))?
        }
        (T::Ptr(p), T::Ptr(q))
        | (T::ByRef(p), T::ByRef(q))
        | (T::Pinned(p), T::Pinned(q))
        | (T::SzArray(p), T::SzArray(q)) => type_matches(asms, (ia, p.as_ref()), (ib, q.as_ref()))?,
        (
            T::Array {
                elem: p,
                rank: r1,
                sizes: s1,
                lo_bounds: l1,
            },
            T::Array {
                elem: q,
                rank: r2,
                sizes: s2,
                lo_bounds: l2,
            },
        ) => {
            r1 == r2
                && s1 == s2
                && l1 == l2
                && type_matches(asms, (ia, p.as_ref()), (ib, q.as_ref()))?
        }
        (
            T::GenericInst {
                value_type: v1,
                ty: t1,
                args: a1,
            },
            T::GenericInst {
                value_type: v2,
                ty: t2,
                args: a2,
            },
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
            T::Modified {
                required: r1,
                modifier: m1,
                ty: p,
            },
            T::Modified {
                required: r2,
                modifier: m2,
                ty: q,
            },
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
        let write_line = MethodId {
            asm: asms.corlib(),
            row: 1,
        };
        assert_eq!(
            method_name(&asms, write_line).unwrap(),
            "System.Console::WriteLine"
        );
        assert_eq!(
            method_key(&asms, write_line).unwrap(),
            "System.Console::WriteLine(string)"
        );
    }

    #[test]
    fn keys_render_parameter_types() {
        let mut b = corlib_builder();
        // (int, object, string[], System.String) — the last as Class(TypeDef 3).
        b.method(
            "Mixed",
            &[0x00, 0x04, 0x01, 0x08, 0x1C, 0x1D, 0x0E, 0x12, 0x0C],
            None,
        );
        let asms = Assemblies::new(b.build()).unwrap();
        let key = method_key(
            &asms,
            MethodId {
                asm: asms.corlib(),
                row: 2,
            },
        )
        .unwrap();
        assert_eq!(
            key,
            "System.Console::Mixed(int,object,string[],System.String)"
        );
    }

    #[test]
    fn member_ref_resolves_to_corlib_method_def() {
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(app().build());
        let target = resolve_member_ref(&asms, app, 1).unwrap();
        assert_eq!(
            target,
            MethodId {
                asm: asms.corlib(),
                row: 1
            }
        );
    }

    #[test]
    fn overloads_are_chosen_by_signature() {
        let mut corlib = corlib_builder();
        corlib.method("WriteLine", &[0x00, 0x01, 0x01, 0x1C], None); // WriteLine(object), row 2
        let mut entry = app();
        let by_object = entry.member_ref(
            Cell::Ref(TableId::TypeRef, 2),
            "WriteLine",
            &[0x00, 0x01, 0x01, 0x1C],
        );
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
        let show = entry.member_ref(
            Cell::Ref(TableId::TypeRef, 2),
            "Show",
            &[0x00, 0x01, 0x01, 0x12, 0x05],
        );
        // Same name, but the parameter is Class(TypeRef 2) = System.Console.
        let wrong = entry.member_ref(
            Cell::Ref(TableId::TypeRef, 2),
            "Show",
            &[0x00, 0x01, 0x01, 0x12, 0x09],
        );
        let mut asms = Assemblies::new(corlib.build()).unwrap();
        let app = asms.add(entry.build());
        assert_eq!(
            resolve_member_ref(&asms, app, show).unwrap(),
            MethodId {
                asm: asms.corlib(),
                row: 2
            }
        );
        let err = resolve_member_ref(&asms, app, wrong).unwrap_err();
        assert_eq!(
            err.to_string(),
            "not implemented: method System.Console::Show(System.Console)"
        );
    }

    #[test]
    fn missing_method_message() {
        let mut entry = app();
        let write = entry.member_ref(Cell::Ref(TableId::TypeRef, 2), "Write", VOID_STRING);
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(entry.build());
        let err = resolve_member_ref(&asms, app, write).unwrap_err();
        assert_eq!(
            err.to_string(),
            "not implemented: method System.Console::Write(string)"
        );
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
        let err = method_sig(
            &asms,
            MethodId {
                asm: asms.corlib(),
                row: 2,
            },
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 66);
    }
}
