//! Assembly loading and name-based binding (spec §5.3 "Loader").

use std::fmt;
use std::path::Path;

use metadata::tables::col;
use metadata::{MetadataModel, Row, TableId, read_assembly};

use crate::error::{Result, RuntimeError};

pub const CORLIB_NAME: &str = "mscorlib";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssemblyId(pub usize);

/// Text of a #Strings column. Rows produced by `read_assembly` always hold a
/// `Cell::Str` in such columns.
pub fn text(row: &Row, column: usize) -> &str {
    row[column].as_str().unwrap_or("")
}

/// `Namespace.Name`, or just `Name` in the global namespace.
pub fn qualified(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_owned()
    } else {
        format!("{namespace}.{name}")
    }
}

#[derive(Debug)]
pub struct LoadedAssembly {
    /// Simple name from the Assembly table, e.g. `mscorlib`.
    pub name: String,
    /// Where it came from; prefixes load errors.
    pub path: String,
    pub model: MetadataModel,
}

impl LoadedAssembly {
    pub fn from_file(path: &Path) -> Result<Self> {
        let shown = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|e| RuntimeError::load(&shown, e))?;
        let model = read_assembly(&bytes).map_err(|e| RuntimeError::load(&shown, e))?;
        Self::from_model(shown, model)
    }

    pub fn from_model(path: impl Into<String>, model: MetadataModel) -> Result<Self> {
        let path = path.into();
        let Some(assembly) = model.table(TableId::Assembly).first() else {
            return Err(RuntimeError::load(
                path,
                "not an assembly: the Assembly table is empty",
            ));
        };
        let name = text(assembly, col::assembly::NAME).to_owned();
        Ok(Self { name, path, model })
    }

    pub fn bad_image(&self, reason: impl fmt::Display) -> RuntimeError {
        RuntimeError::load(&self.path, reason)
    }

    /// Row `row` (1-based) of `table`, or a load error if it does not exist.
    /// Use this for tokens taken from IL, which the reader does not validate.
    pub fn row(&self, table: TableId, row: u32) -> Result<&Row> {
        row.checked_sub(1)
            .and_then(|i| self.model.table(table).get(i as usize))
            .ok_or_else(|| self.bad_image(format!("{table:?} row {row} does not exist")))
    }
}

/// All loaded assemblies; the corlib is always `AssemblyId(0)`.
pub struct Assemblies {
    list: Vec<LoadedAssembly>,
}

impl Assemblies {
    pub fn new(corlib: LoadedAssembly) -> Result<Self> {
        if corlib.name != CORLIB_NAME {
            return Err(corlib.bad_image(format!(
                "expected assembly '{CORLIB_NAME}', found '{}'",
                corlib.name
            )));
        }
        Ok(Self { list: vec![corlib] })
    }

    pub fn add(&mut self, assembly: LoadedAssembly) -> AssemblyId {
        self.list.push(assembly);
        AssemblyId(self.list.len() - 1)
    }

    pub fn get(&self, id: AssemblyId) -> &LoadedAssembly {
        &self.list[id.0]
    }

    pub fn corlib(&self) -> AssemblyId {
        AssemblyId(0)
    }

    /// Binds AssemblyRef `row` of `from` by simple name; version and public
    /// key token are ignored (spec §5.3).
    pub fn resolve_assembly_ref(&self, from: AssemblyId, row: u32) -> Result<AssemblyId> {
        let assembly = self.get(from);
        let name = text(
            assembly.row(TableId::AssemblyRef, row)?,
            col::assembly_ref::NAME,
        );
        self.list
            .iter()
            .position(|a| a.name == name)
            .map(AssemblyId)
            .ok_or_else(|| {
                assembly.bad_image(format!("cannot resolve assembly reference '{name}'"))
            })
    }

    pub fn find_type_def(&self, asm: AssemblyId, namespace: &str, name: &str) -> Option<u32> {
        self.get(asm)
            .model
            .table(TableId::TypeDef)
            .iter()
            .position(|r| {
                text(r, col::type_def::NAME) == name
                    && text(r, col::type_def::NAMESPACE) == namespace
            })
            .map(|i| i as u32 + 1)
    }

    /// The (assembly, namespace, name) TypeRef `row` of `asm` denotes. Does
    /// not require the type to exist.
    pub fn type_ref_identity(
        &self,
        asm: AssemblyId,
        row: u32,
    ) -> Result<(AssemblyId, String, String)> {
        let r = self.get(asm).row(TableId::TypeRef, row)?;
        let namespace = text(r, col::type_ref::NAMESPACE).to_owned();
        let name = text(r, col::type_ref::NAME).to_owned();
        let target = match r[col::type_ref::RESOLUTION_SCOPE].as_index() {
            Some((TableId::AssemblyRef, scope)) if scope != 0 => {
                self.resolve_assembly_ref(asm, scope)?
            }
            Some((TableId::Module, _)) => asm,
            Some((TableId::TypeRef, _)) => {
                return Err(RuntimeError::not_implemented(format!(
                    "nested type reference {}",
                    qualified(&namespace, &name)
                )));
            }
            _ => {
                return Err(RuntimeError::not_implemented(format!(
                    "resolution scope of type reference {}",
                    qualified(&namespace, &name)
                )));
            }
        };
        Ok((target, namespace, name))
    }

    /// Resolves TypeRef `row` of `asm` to its defining assembly and TypeDef
    /// row without loading the type.
    pub fn resolve_type_ref(&self, asm: AssemblyId, row: u32) -> Result<(AssemblyId, u32)> {
        let (target, namespace, name) = self.type_ref_identity(asm, row)?;
        let def = self
            .find_type_def(target, &namespace, &name)
            .ok_or_else(|| {
                RuntimeError::not_implemented(format!(
                    "type {} in assembly '{}'",
                    qualified(&namespace, &name),
                    self.get(target).name
                ))
            })?;
        Ok((target, def))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, test_corlib};
    use metadata::tables::col;
    use metadata::{Cell, TableId};

    #[test]
    fn loads_an_mcs_fixture_from_disk() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../metadata/tests/fixtures/hello.exe");
        let asm = LoadedAssembly::from_file(&path).unwrap();
        assert_eq!(asm.name, "hello");
        assert_eq!(asm.path, path.display().to_string());
    }

    #[test]
    fn missing_file_is_a_load_error_naming_the_path() {
        let err = LoadedAssembly::from_file(Path::new("/nonexistent/app.exe")).unwrap_err();
        assert!(
            matches!(&err, RuntimeError::Load { path, .. } if path == "/nonexistent/app.exe"),
            "{err:?}"
        );
        assert_eq!(err.exit_code(), 66);
    }

    #[test]
    fn non_pe_file_is_a_load_error() {
        let path =
            std::env::temp_dir().join(format!("runtime-loader-test-{}.exe", std::process::id()));
        std::fs::write(&path, b"not a PE file").unwrap();
        let err = LoadedAssembly::from_file(&path).unwrap_err();
        std::fs::remove_file(&path).unwrap();
        assert!(err.to_string().contains("bad image"), "{err}");
    }

    #[test]
    fn corlib_must_be_mscorlib() {
        let err = Assemblies::new(app().build())
            .err()
            .expect("app is not mscorlib");
        assert!(
            err.to_string()
                .contains("expected assembly 'mscorlib', found 'app'"),
            "{err}"
        );
    }

    #[test]
    fn assembly_refs_bind_by_name_only() {
        let mut entry = app();
        // Mono's mscorlib reference: version 4.0.0.0 plus a public key token.
        entry.model.table_mut(TableId::AssemblyRef)[0][col::assembly_ref::MAJOR_VERSION] =
            Cell::U16(9);
        entry.model.table_mut(TableId::AssemblyRef)[0][col::assembly_ref::PUBLIC_KEY_OR_TOKEN] =
            Cell::Blob(vec![0xb7, 0x7a, 0x5c, 0x56, 0x19, 0x34, 0xe0, 0x89]);
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(entry.build());
        assert_eq!(asms.resolve_assembly_ref(app, 1).unwrap(), asms.corlib());
    }

    #[test]
    fn unknown_assembly_ref_is_a_load_error() {
        let mut entry = app();
        let other = entry.assembly_ref("System.Core");
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(entry.build());
        let err = asms.resolve_assembly_ref(app, other).unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot resolve assembly reference 'System.Core'"),
            "{err}"
        );
        assert_eq!(err.exit_code(), 66);
    }

    #[test]
    fn type_refs_resolve_to_corlib_type_defs() {
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(app().build());
        assert_eq!(asms.resolve_type_ref(app, 2).unwrap(), (asms.corlib(), 5)); // System.Console
        assert_eq!(
            asms.find_type_def(asms.corlib(), "System", "String"),
            Some(3)
        );
        assert_eq!(asms.find_type_def(asms.corlib(), "System", "Nope"), None);
    }

    #[test]
    fn missing_type_is_not_implemented() {
        let mut entry = app();
        let attr = entry.type_ref(
            1,
            "System.Runtime.CompilerServices",
            "RuntimeCompatibilityAttribute",
        );
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let app = asms.add(entry.build());
        let identity = asms.type_ref_identity(app, attr).unwrap();
        assert_eq!(
            identity.0,
            asms.corlib(),
            "identity does not need the type to exist"
        );
        let err = asms.resolve_type_ref(app, attr).unwrap_err();
        assert_eq!(
            err.to_string(),
            "not implemented: type System.Runtime.CompilerServices.RuntimeCompatibilityAttribute in assembly 'mscorlib'"
        );
    }

    #[test]
    fn row_lookup_is_checked() {
        let corlib = test_corlib();
        assert!(corlib.row(TableId::TypeDef, 1).is_ok());
        assert!(corlib.row(TableId::TypeDef, 0).is_err());
        assert!(corlib.row(TableId::TypeDef, 99).is_err());
        assert!(corlib.row(TableId::MemberRef, 1).is_err());
    }
}
