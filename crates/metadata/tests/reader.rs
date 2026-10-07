use metadata::cli::COMIMAGE_FLAGS_ILONLY;
use metadata::{Cell, MetadataModel, Row, TableId, read_assembly};

const HELLO: &[u8] = include_bytes!("fixtures/hello.exe");
const LIB: &[u8] = include_bytes!("fixtures/lib.dll");

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn type_def<'a>(m: &'a MetadataModel, ns: &str, name: &str) -> Option<&'a Row> {
    m.table(TableId::TypeDef)
        .iter()
        .find(|r| r[1].as_str() == Some(name) && r[2].as_str() == Some(ns))
}

fn method<'a>(m: &'a MetadataModel, name: &str) -> &'a Row {
    m.table(TableId::MethodDef)
        .iter()
        .find(|r| r[3].as_str() == Some(name))
        .unwrap_or_else(|| panic!("method {name} not found"))
}

#[test]
fn reads_hello_header_values() {
    let m = read_assembly(HELLO).unwrap();
    assert_eq!(m.version, "v4.0.30319");
    assert_eq!(m.entry_point_token, 0x0600_0001);
    assert_eq!(m.cli_flags & COMIMAGE_FLAGS_ILONLY, COMIMAGE_FLAGS_ILONLY);
    assert_eq!(m.table(TableId::Module)[0][1].as_str(), Some("hello.exe"));
}

#[test]
fn reads_hello_types_and_main_body() {
    let m = read_assembly(HELLO).unwrap();
    assert!(type_def(&m, "HelloApp", "Program").is_some());
    let main = method(&m, "Main");
    let body = main[0].as_body().expect("Main has IL");
    // ldstr 0x70000001; call 0x0A000001; ret
    assert_eq!(
        body.code,
        [
            0x72, 0x01, 0x00, 0x00, 0x70, 0x28, 0x01, 0x00, 0x00, 0x0A, 0x2A
        ]
    );
}

#[test]
fn reads_hello_references() {
    let m = read_assembly(HELLO).unwrap();
    let write_line = &m.table(TableId::MemberRef)[0];
    assert_eq!(write_line[1].as_str(), Some("WriteLine"));
    let (parent_table, parent_row) = write_line[0].as_index().unwrap();
    assert_eq!(parent_table, TableId::TypeRef);
    let console = &m.table(TableId::TypeRef)[parent_row as usize - 1];
    assert_eq!(console[1].as_str(), Some("Console"));
    assert_eq!(console[2].as_str(), Some("System"));

    let mscorlib = m
        .table(TableId::AssemblyRef)
        .iter()
        .find(|r| r[6].as_str() == Some("mscorlib"))
        .expect("mscorlib reference");
    let version: Vec<u32> = mscorlib[..4].iter().map(|c| c.as_u32().unwrap()).collect();
    assert_eq!(version, [4, 0, 0, 0]);

    assert_eq!(
        m.user_string(0x7000_0001),
        Some(&utf16("Hello, World!")[..])
    );
}

#[test]
fn reads_lib_tables() {
    let m = read_assembly(LIB).unwrap();
    for name in ["IShape", "Point", "Circle", "Color"] {
        assert!(type_def(&m, "Fixtures", name).is_some(), "{name}");
    }
    assert!(
        type_def(&m, "", "Nested").is_some(),
        "nested types have no namespace"
    );
    for t in [
        TableId::Field,
        TableId::Param,
        TableId::InterfaceImpl,
        TableId::Constant,
        TableId::CustomAttribute,
        TableId::StandAloneSig,
        TableId::EventMap,
        TableId::Event,
        TableId::PropertyMap,
        TableId::Property,
        TableId::MethodSemantics,
        TableId::TypeSpec,
        TableId::NestedClass,
        TableId::GenericParam,
        TableId::MethodSpec,
    ] {
        assert!(!m.table(t).is_empty(), "{t:?} should not be empty");
    }
    let safe_divide = method(&m, "SafeDivide")[0].as_body().unwrap();
    assert!(
        !safe_divide.extra_sections.is_empty(),
        "try/catch/finally emits an EH section"
    );
    // `Area` is declared by IShape (abstract, no body) and implemented by Circle.
    let areas: Vec<_> = m
        .table(TableId::MethodDef)
        .iter()
        .filter(|r| r[3].as_str() == Some("Area"))
        .collect();
    assert!(
        areas.iter().any(|r| r[0].as_body().is_some()),
        "Circle.Area has IL"
    );
    assert!(
        m.table(TableId::MethodDef)
            .iter()
            .any(|r| matches!(r[0], Cell::Body(None))),
        "interface methods have no body"
    );
}

#[test]
fn model_user_strings() {
    let mut m = MetadataModel::new();
    assert_eq!(m.tables.len(), metadata::tables::TABLE_COUNT);
    assert_eq!(m.add_user_string("Hello").unwrap(), 0x7000_0001);
    assert_eq!(m.add_user_string("x").unwrap(), 0x7000_000D);
    assert_eq!(m.user_string(0x7000_000D), Some(&utf16("x")[..]));
    assert_eq!(m.user_string(0x7000_0002), None, "not an entry start");
    assert_eq!(m.push_row(TableId::TypeRef, vec![]), 1);
    assert_eq!(m.push_row(TableId::TypeRef, vec![]), 2);
}

#[test]
fn truncated_inputs_never_panic() {
    for len in (0..HELLO.len()).step_by(7) {
        let _ = read_assembly(&HELLO[..len]);
    }
    assert!(read_assembly(&HELLO[..0x200]).is_err());
}

#[test]
fn bit_flipped_inputs_never_panic() {
    for i in 0..HELLO.len() {
        let mut corrupted = HELLO.to_vec();
        corrupted[i] ^= 0xFF;
        let _ = read_assembly(&corrupted);
    }
}

#[cfg_attr(
    not(has_mono_corlib),
    ignore = "Mono mscorlib.dll not found (set MONO_CORLIB)"
)]
#[test]
fn reads_mono_mscorlib() {
    let path = option_env!("MONO_CORLIB").unwrap();
    let m = read_assembly(&std::fs::read(path).unwrap()).unwrap();
    assert!(type_def(&m, "System", "Object").is_some());
    assert!(type_def(&m, "System", "String").is_some());
    assert!(type_def(&m, "System", "Console").is_some());
    assert!(m.table(TableId::TypeDef).len() > 1000);
}
