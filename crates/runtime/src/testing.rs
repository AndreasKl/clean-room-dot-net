//! In-memory metadata models for unit tests. Rows follow the column layout
//! of `metadata::tables::columns()`.
#![allow(dead_code)] // shared helpers; not every test module uses every item

use metadata::body::MethodBody;
use metadata::{Cell, MetadataModel, TableId};

use crate::loader::LoadedAssembly;

/// `Public | Static | HideBySig`
pub const PUBLIC_STATIC: u16 = 0x0096;
pub const VOID_NO_ARGS: &[u8] = &[0x00, 0x00, 0x01];
pub const INT_NO_ARGS: &[u8] = &[0x00, 0x00, 0x08];
pub const VOID_STRING: &[u8] = &[0x00, 0x01, 0x01, 0x0E];
/// MemberRef 1 of `app()`: `System.Console::WriteLine(string)`.
pub const WRITE_LINE: u32 = 0x0A00_0001;

pub struct ModelBuilder {
    name: String,
    pub model: MetadataModel,
}

impl ModelBuilder {
    /// A module and assembly named `name`, version 4.0.0.0, with `<Module>` as TypeDef 1.
    pub fn new(name: &str) -> Self {
        let mut model = MetadataModel::new();
        model.push_row(
            TableId::Module,
            vec![
                Cell::U16(0),
                Cell::Str(format!("{name}.dll")),
                Cell::Guid(Some([1; 16])),
                Cell::Guid(None),
                Cell::Guid(None),
            ],
        );
        model.push_row(
            TableId::Assembly,
            vec![
                Cell::U32(0x8004),
                Cell::U16(4),
                Cell::U16(0),
                Cell::U16(0),
                Cell::U16(0),
                Cell::U32(0),
                Cell::Blob(Vec::new()),
                Cell::Str(name.to_owned()),
                Cell::Str(String::new()),
            ],
        );
        let mut builder = Self {
            name: name.to_owned(),
            model,
        };
        builder.type_def("", "<Module>", Cell::Ref(TableId::TypeDef, 0));
        builder
    }

    /// Appends a type; methods added afterwards belong to it until the next type.
    pub fn type_def(&mut self, namespace: &str, name: &str, extends: Cell) -> u32 {
        let field_list = self.model.table(TableId::Field).len() as u32 + 1;
        let method_list = self.model.table(TableId::MethodDef).len() as u32 + 1;
        self.model.push_row(
            TableId::TypeDef,
            vec![
                Cell::U32(0),
                Cell::Str(name.to_owned()),
                Cell::Str(namespace.to_owned()),
                extends,
                Cell::Ref(TableId::Field, field_list),
                Cell::Ref(TableId::MethodDef, method_list),
            ],
        )
    }

    /// Appends a public static method; `code: None` makes it body-less.
    pub fn method(&mut self, name: &str, signature: &[u8], code: Option<&[u8]>) -> u32 {
        let param_list = self.model.table(TableId::Param).len() as u32 + 1;
        let body = code.map(|code| MethodBody {
            max_stack: 8,
            init_locals: false,
            local_var_sig_token: 0,
            code: code.to_vec(),
            extra_sections: Vec::new(),
        });
        self.model.push_row(
            TableId::MethodDef,
            vec![
                Cell::Body(body),
                Cell::U16(0),
                Cell::U16(PUBLIC_STATIC),
                Cell::Str(name.to_owned()),
                Cell::Blob(signature.to_vec()),
                Cell::Ref(TableId::Param, param_list),
            ],
        )
    }

    pub fn assembly_ref(&mut self, name: &str) -> u32 {
        self.model.push_row(
            TableId::AssemblyRef,
            vec![
                Cell::U16(4),
                Cell::U16(0),
                Cell::U16(0),
                Cell::U16(0),
                Cell::U32(0),
                Cell::Blob(Vec::new()),
                Cell::Str(name.to_owned()),
                Cell::Str(String::new()),
                Cell::Blob(Vec::new()),
            ],
        )
    }

    pub fn type_ref(&mut self, assembly_ref: u32, namespace: &str, name: &str) -> u32 {
        self.model.push_row(
            TableId::TypeRef,
            vec![
                Cell::Ref(TableId::AssemblyRef, assembly_ref),
                Cell::Str(name.to_owned()),
                Cell::Str(namespace.to_owned()),
            ],
        )
    }

    pub fn member_ref(&mut self, parent: Cell, name: &str, signature: &[u8]) -> u32 {
        self.model.push_row(
            TableId::MemberRef,
            vec![
                parent,
                Cell::Str(name.to_owned()),
                Cell::Blob(signature.to_vec()),
            ],
        )
    }

    /// Returns the `ldstr` token.
    pub fn user_string(&mut self, s: &str) -> u32 {
        self.model.add_user_string(s).expect("small #US heap")
    }

    pub fn entry_point(&mut self, method_row: u32) {
        self.model.entry_point_token = TableId::MethodDef.token(method_row);
    }

    pub fn build(self) -> LoadedAssembly {
        LoadedAssembly::from_model(format!("<{}>", self.name), self.model)
            .expect("test models have an Assembly row")
    }
}

/// mscorlib with System.Object (TypeDef 2), System.String (3), System.Array (4)
/// and System.Console (5) declaring body-less `WriteLine(string)` (MethodDef 1).
/// Console is the last type, so tests may add more Console methods.
pub fn corlib_builder() -> ModelBuilder {
    let mut b = ModelBuilder::new("mscorlib");
    let object = b.type_def("System", "Object", Cell::Ref(TableId::TypeDef, 0));
    b.type_def("System", "String", Cell::Ref(TableId::TypeDef, object));
    b.type_def("System", "Array", Cell::Ref(TableId::TypeDef, object));
    b.type_def("System", "Console", Cell::Ref(TableId::TypeDef, object));
    b.method("WriteLine", VOID_STRING, None);
    b
}

pub fn test_corlib() -> LoadedAssembly {
    corlib_builder().build()
}

/// Entry assembly `app`: AssemblyRef 1 = mscorlib, TypeRef 1 = System.Object,
/// TypeRef 2 = System.Console, MemberRef 1 = Console::WriteLine(string)
/// (`WRITE_LINE`), TypeDef 2 = `App` extending Object. Methods added
/// afterwards belong to `App`.
pub fn app() -> ModelBuilder {
    let mut b = ModelBuilder::new("app");
    let mscorlib = b.assembly_ref("mscorlib");
    let object = b.type_ref(mscorlib, "System", "Object");
    let console = b.type_ref(mscorlib, "System", "Console");
    b.member_ref(
        Cell::Ref(TableId::TypeRef, console),
        "WriteLine",
        VOID_STRING,
    );
    b.type_def("", "App", Cell::Ref(TableId::TypeRef, object));
    b
}

/// Runs `entry` against `corlib`; returns the outcome and captured stdout.
pub fn run_with(
    corlib: LoadedAssembly,
    entry: ModelBuilder,
    args: &[&str],
) -> (crate::Result<i32>, String) {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let mut out = Vec::new();
    let result = crate::run(corlib, entry.build(), &args, &mut out);
    (
        result,
        String::from_utf8(out).expect("program output is UTF-8"),
    )
}

/// Runs `entry` against `test_corlib()`.
pub fn run(entry: ModelBuilder, args: &[&str]) -> (crate::Result<i32>, String) {
    run_with(test_corlib(), entry, args)
}

/// Concatenates IL fragments, e.g. `il(&[&[LDSTR], &token.to_le_bytes()])`.
pub fn il(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}
