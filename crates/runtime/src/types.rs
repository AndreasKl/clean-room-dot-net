//! Runtime type descriptors (spec §5.3 "Type system").

use std::collections::HashMap;

use metadata::TableId;
use metadata::tables::col;

use crate::error::{Result, RuntimeError};
use crate::loader::{Assemblies, AssemblyId, qualified, text};

/// Index of a loaded type in the `TypeSystem`; stored in every object header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

/// Base-type chains longer than this are treated as cyclic metadata.
const MAX_INHERITANCE_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeType {
    pub namespace: String,
    pub name: String,
    /// Defining assembly and TypeDef row; `None` for synthesized array types.
    pub def: Option<(AssemblyId, u32)>,
    pub base: Option<TypeId>,
    /// Element type of a single-dimensional, zero-based array type.
    pub element: Option<TypeId>,
}

impl RuntimeType {
    pub fn full_name(&self) -> String {
        qualified(&self.namespace, &self.name)
    }
}

/// Types are loaded lazily, on first use, and cached.
#[derive(Default)]
pub struct TypeSystem {
    types: Vec<RuntimeType>,
    by_def: HashMap<(AssemblyId, u32), TypeId>,
    szarrays: HashMap<TypeId, TypeId>,
}

impl TypeSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: TypeId) -> &RuntimeType {
        &self.types[id.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    fn push(&mut self, t: RuntimeType) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        self.types.push(t);
        id
    }

    /// Loads TypeDef `row` of `asm` and its base chain.
    pub fn load_type_def(
        &mut self,
        asms: &Assemblies,
        asm: AssemblyId,
        row: u32,
    ) -> Result<TypeId> {
        self.load_at_depth(asms, asm, row, 0)
    }

    /// Resolves a TypeDef or TypeRef reference that appears in `asm`.
    pub fn resolve_type_token(
        &mut self,
        asms: &Assemblies,
        asm: AssemblyId,
        table: TableId,
        row: u32,
    ) -> Result<TypeId> {
        self.resolve_at_depth(asms, asm, table, row, 0)
    }

    fn resolve_at_depth(
        &mut self,
        asms: &Assemblies,
        asm: AssemblyId,
        table: TableId,
        row: u32,
        depth: usize,
    ) -> Result<TypeId> {
        match table {
            TableId::TypeDef => self.load_at_depth(asms, asm, row, depth),
            TableId::TypeRef => {
                let (target, target_row) = asms.resolve_type_ref(asm, row)?;
                self.load_at_depth(asms, target, target_row, depth)
            }
            other => Err(RuntimeError::not_implemented(format!(
                "{other:?} type references"
            ))),
        }
    }

    fn load_at_depth(
        &mut self,
        asms: &Assemblies,
        asm: AssemblyId,
        row: u32,
        depth: usize,
    ) -> Result<TypeId> {
        if let Some(&id) = self.by_def.get(&(asm, row)) {
            return Ok(id);
        }
        let assembly = asms.get(asm);
        if depth > MAX_INHERITANCE_DEPTH {
            return Err(assembly.bad_image(format!(
                "base-type chain at TypeDef row {row} is cyclic or deeper than {MAX_INHERITANCE_DEPTH}"
            )));
        }
        let def = assembly.row(TableId::TypeDef, row)?;
        let namespace = text(def, col::type_def::NAMESPACE).to_owned();
        let name = text(def, col::type_def::NAME).to_owned();
        let base = match def[col::type_def::EXTENDS].as_index() {
            Some((_, 0)) | None => None,
            Some((table, base_row)) => {
                Some(self.resolve_at_depth(asms, asm, table, base_row, depth + 1)?)
            }
        };
        let id = self.push(RuntimeType {
            namespace,
            name,
            def: Some((asm, row)),
            base,
            element: None,
        });
        self.by_def.insert((asm, row), id);
        Ok(id)
    }

    /// A well-known corlib type such as `System.String`.
    pub fn corlib_type(
        &mut self,
        asms: &Assemblies,
        namespace: &str,
        name: &str,
    ) -> Result<TypeId> {
        let corlib = asms.corlib();
        let row = asms.find_type_def(corlib, namespace, name).ok_or_else(|| {
            asms.get(corlib).bad_image(format!(
                "corlib does not define {}",
                qualified(namespace, name)
            ))
        })?;
        self.load_type_def(asms, corlib, row)
    }

    /// The single-dimensional, zero-based array type `element[]`.
    pub fn szarray(&mut self, element: TypeId, array_base: TypeId) -> TypeId {
        if let Some(&id) = self.szarrays.get(&element) {
            return id;
        }
        let e = self.get(element);
        let array = RuntimeType {
            namespace: e.namespace.clone(),
            name: format!("{}[]", e.name),
            def: None,
            base: Some(array_base),
            element: Some(element),
        };
        let id = self.push(array);
        self.szarrays.insert(element, id);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Assemblies;
    use crate::testing::{app, corlib_builder, test_corlib};
    use metadata::Cell;

    #[test]
    fn loads_types_with_their_base_chain_once() {
        let asms = Assemblies::new(test_corlib()).unwrap();
        let mut types = TypeSystem::new();
        let string = types.load_type_def(&asms, asms.corlib(), 3).unwrap();
        assert_eq!(types.get(string).full_name(), "System.String");
        let object = types.get(string).base.expect("String extends Object");
        assert_eq!(types.get(object).full_name(), "System.Object");
        assert_eq!(types.get(object).base, None);
        assert_eq!(types.len(), 2);
        assert_eq!(
            types.load_type_def(&asms, asms.corlib(), 3).unwrap(),
            string
        );
        assert_eq!(types.len(), 2, "cached");
    }

    #[test]
    fn resolves_type_refs_across_assemblies() {
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app_id = asms.add(app().build());
        let mut types = TypeSystem::new();
        let app_type = types.load_type_def(&asms, app_id, 2).unwrap();
        let base = types.get(app_type).base.unwrap();
        assert_eq!(types.get(base).def, Some((asms.corlib(), 2)));
        let console = types
            .resolve_type_token(&asms, app_id, metadata::TableId::TypeRef, 2)
            .unwrap();
        assert_eq!(types.get(console).full_name(), "System.Console");
    }

    #[test]
    fn cyclic_inheritance_is_a_load_error() {
        let mut b = corlib_builder();
        // TypeDef 6 extends 7 and 7 extends 6.
        b.type_def("Loop", "A", Cell::Ref(metadata::TableId::TypeDef, 7));
        b.type_def("Loop", "B", Cell::Ref(metadata::TableId::TypeDef, 6));
        let asms = Assemblies::new(b.build()).unwrap();
        let err = TypeSystem::new()
            .load_type_def(&asms, asms.corlib(), 6)
            .unwrap_err();
        assert!(err.to_string().contains("cyclic"), "{err}");
        assert_eq!(err.exit_code(), 66);
    }

    #[test]
    fn corlib_types_by_name() {
        let asms = Assemblies::new(test_corlib()).unwrap();
        let mut types = TypeSystem::new();
        let array = types.corlib_type(&asms, "System", "Array").unwrap();
        assert_eq!(types.get(array).def, Some((asms.corlib(), 4)));
        let err = types.corlib_type(&asms, "System", "Int32").unwrap_err();
        assert!(
            err.to_string()
                .contains("corlib does not define System.Int32"),
            "{err}"
        );
    }

    #[test]
    fn szarray_types_are_synthesized_once() {
        let asms = Assemblies::new(test_corlib()).unwrap();
        let mut types = TypeSystem::new();
        let string = types.corlib_type(&asms, "System", "String").unwrap();
        let array = types.corlib_type(&asms, "System", "Array").unwrap();
        let strings = types.szarray(string, array);
        assert_eq!(types.get(strings).full_name(), "System.String[]");
        assert_eq!(types.get(strings).element, Some(string));
        assert_eq!(types.get(strings).base, Some(array));
        assert_eq!(types.get(strings).def, None);
        assert_eq!(types.szarray(string, array), strings);
    }

    #[test]
    fn type_spec_references_are_not_implemented() {
        let asms = Assemblies::new(test_corlib()).unwrap();
        let err = TypeSystem::new()
            .resolve_type_token(&asms, asms.corlib(), metadata::TableId::TypeSpec, 1)
            .unwrap_err();
        assert_eq!(err.exit_code(), 70);
    }
}
