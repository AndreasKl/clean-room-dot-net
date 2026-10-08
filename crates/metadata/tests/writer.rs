use metadata::pe::PeImage;
use metadata::tables::CodedIndex;
use metadata::{Cell, Error, ImageKind, MetadataModel, TableId, read_assembly, write_assembly};

const HELLO: &[u8] = include_bytes!("fixtures/hello.exe");
const LIB: &[u8] = include_bytes!("fixtures/lib.dll");

fn round_trip(model: &MetadataModel, kind: ImageKind) -> MetadataModel {
    let bytes = write_assembly(model, kind).unwrap();
    read_assembly(&bytes).unwrap()
}

#[cfg(has_mono)]
fn run_on_mono(bytes: &[u8], name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, bytes).unwrap();
    let out = std::process::Command::new("mono")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "mono failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn with_wide_heaps() -> MetadataModel {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::TypeRef).push(vec![
        Cell::Ref(TableId::AssemblyRef, 1),
        Cell::Str("N".repeat(70_000)),
        Cell::Str(String::new()),
    ]);
    m.table_mut(TableId::StandAloneSig)
        .push(vec![Cell::Blob(vec![0x06; 70_000])]);
    m
}

fn with_wide_indexes() -> MetadataModel {
    let mut m = read_assembly(HELLO).unwrap();
    for i in 0..20_000 {
        m.table_mut(TableId::TypeRef).push(vec![
            Cell::Ref(TableId::AssemblyRef, 1),
            Cell::Str(format!("T{i}")),
            Cell::Str("Generated".into()),
        ]);
    }
    m
}

#[test]
fn hello_round_trips() {
    let m = read_assembly(HELLO).unwrap();
    assert_eq!(round_trip(&m, ImageKind::Exe), m);
}

#[test]
fn lib_round_trips() {
    let m = read_assembly(LIB).unwrap();
    assert_eq!(round_trip(&m, ImageKind::Dll), m);
}

#[test]
fn written_image_is_deterministic_and_parseable() {
    let m = read_assembly(HELLO).unwrap();
    let a = write_assembly(&m, ImageKind::Exe).unwrap();
    let b = write_assembly(&m, ImageKind::Exe).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len() % 0x200, 0);
    let pe = PeImage::parse(&a).unwrap();
    assert_eq!(pe.sections.len(), 2);
}

#[test]
fn wide_heaps_round_trip() {
    let m = with_wide_heaps();
    assert_eq!(round_trip(&m, ImageKind::Exe), m);
}

#[test]
fn wide_table_indexes_round_trip() {
    let m = with_wide_indexes();
    assert_eq!(round_trip(&m, ImageKind::Exe), m);
}

#[test]
fn writer_sorts_custom_attributes() {
    let original = read_assembly(LIB).unwrap();
    let mut shuffled = original.clone();
    shuffled.table_mut(TableId::CustomAttribute).reverse();
    let reread = round_trip(&shuffled, ImageKind::Dll);
    let attrs = reread.table(TableId::CustomAttribute);
    let keys: Vec<u32> = attrs
        .iter()
        .map(|r| {
            let (t, row) = r[0].as_index().unwrap();
            CodedIndex::HasCustomAttribute.encode(t, row).unwrap()
        })
        .collect();
    assert!(keys.windows(2).all(|w| w[0] <= w[1]), "sorted by parent");
    let as_strings = |rows: &[metadata::Row]| {
        let mut v: Vec<String> = rows.iter().map(|r| format!("{r:?}")).collect();
        v.sort();
        v
    };
    assert_eq!(
        as_strings(attrs),
        as_strings(original.table(TableId::CustomAttribute))
    );
}

#[test]
fn wrong_cell_kind_is_malformed() {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::TypeRef)[0][1] = Cell::U32(5);
    assert!(matches!(
        write_assembly(&m, ImageKind::Exe),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn wrong_column_count_is_malformed() {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::TypeRef)[0].pop();
    assert!(matches!(
        write_assembly(&m, ImageKind::Exe),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn out_of_range_reference_is_malformed() {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::MemberRef)[0][0] = Cell::Ref(TableId::TypeRef, 999);
    assert!(matches!(
        write_assembly(&m, ImageKind::Exe),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn exe_requires_entry_point() {
    let mut m = read_assembly(HELLO).unwrap();
    m.entry_point_token = 0;
    assert!(matches!(
        write_assembly(&m, ImageKind::Exe),
        Err(Error::Malformed(_))
    ));
    assert!(write_assembly(&m, ImageKind::Dll).is_ok());
}

#[test]
fn field_rva_is_unsupported() {
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::FieldRva)
        .push(vec![Cell::U32(0x2000), Cell::Ref(TableId::Field, 1)]);
    assert!(matches!(
        write_assembly(&m, ImageKind::Exe),
        Err(Error::Unsupported(_))
    ));
}

#[cfg_attr(not(has_mono), ignore = "mono not installed")]
#[test]
fn mono_runs_rewritten_hello() {
    #[cfg(has_mono)]
    {
        let bytes = write_assembly(&read_assembly(HELLO).unwrap(), ImageKind::Exe).unwrap();
        assert_eq!(
            run_on_mono(&bytes, "hello-rewritten.exe"),
            "Hello, World!\n"
        );
    }
}

#[cfg_attr(not(has_mono), ignore = "mono not installed")]
#[test]
fn mono_runs_hello_with_wide_heaps() {
    #[cfg(has_mono)]
    {
        let bytes = write_assembly(&with_wide_heaps(), ImageKind::Exe).unwrap();
        assert_eq!(
            run_on_mono(&bytes, "hello-wide-heaps.exe"),
            "Hello, World!\n"
        );
    }
}

#[cfg_attr(not(has_mono), ignore = "mono not installed")]
#[test]
fn mono_runs_hello_with_wide_indexes() {
    #[cfg(has_mono)]
    {
        let bytes = write_assembly(&with_wide_indexes(), ImageKind::Exe).unwrap();
        assert_eq!(
            run_on_mono(&bytes, "hello-wide-indexes.exe"),
            "Hello, World!\n"
        );
    }
}

#[test]
fn unsorted_decl_security_is_unsupported() {
    let row = |t: u32| {
        vec![
            Cell::U16(0),
            Cell::Ref(TableId::TypeDef, t),
            Cell::Blob(vec![1]),
        ]
    };
    let mut m = read_assembly(HELLO).unwrap();
    m.table_mut(TableId::DeclSecurity).push(row(2));
    m.table_mut(TableId::DeclSecurity).push(row(1));
    assert!(matches!(
        write_assembly(&m, ImageKind::Exe),
        Err(Error::Unsupported(_))
    ));

    let mut sorted = read_assembly(HELLO).unwrap();
    sorted.table_mut(TableId::DeclSecurity).push(row(1));
    sorted.table_mut(TableId::DeclSecurity).push(row(2));
    assert_eq!(round_trip(&sorted, ImageKind::Exe), sorted);
}
