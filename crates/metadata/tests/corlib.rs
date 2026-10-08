//! Decodes and re-encodes every signature in Mono's mscorlib.dll. The
//! re-encoded bytes must equal the originals, which pins both directions of
//! the signature codec against a large real-world assembly.

use metadata::sig::{
    decode_field_sig, decode_local_sig, decode_method_sig, decode_type_spec, encode_field_sig,
    encode_local_sig, encode_method_sig, encode_type_spec,
};
use metadata::{MetadataModel, Result, TableId, read_assembly};

fn reencode(blob: &[u8], kind: &str) -> Result<Vec<u8>> {
    match kind {
        "method" => encode_method_sig(&decode_method_sig(blob)?),
        "field" => encode_field_sig(&decode_field_sig(blob)?),
        "local" => encode_local_sig(&decode_local_sig(blob)?),
        "type" => encode_type_spec(&decode_type_spec(blob)?),
        _ => unreachable!(),
    }
}

fn member_kind(blob: &[u8]) -> &'static str {
    match blob.first() {
        Some(0x06) => "field",
        Some(0x07) => "local",
        _ => "method",
    }
}

fn check(
    m: &MetadataModel,
    table: TableId,
    col: usize,
    kind_of: impl Fn(&[u8]) -> &'static str,
) -> Vec<String> {
    let mut failures = Vec::new();
    for (i, row) in m.table(table).iter().enumerate() {
        let blob = row[col].as_blob().unwrap();
        let kind = kind_of(blob);
        match reencode(blob, kind) {
            Ok(bytes) if bytes == blob => {}
            Ok(bytes) => failures.push(format!(
                "{table:?} row {}: {kind} re-encoded as {bytes:02X?}, original {blob:02X?}",
                i + 1
            )),
            Err(e) => failures.push(format!("{table:?} row {}: {kind} {e}", i + 1)),
        }
    }
    failures
}

#[cfg_attr(
    not(has_mono_corlib),
    ignore = "Mono mscorlib.dll not found (set MONO_CORLIB)"
)]
#[test]
fn every_mscorlib_signature_round_trips() {
    let path = option_env!("MONO_CORLIB").unwrap();
    let m = read_assembly(&std::fs::read(path).unwrap()).unwrap();
    let mut failures = Vec::new();
    failures.extend(check(&m, TableId::MethodDef, 4, |_| "method"));
    failures.extend(check(&m, TableId::Field, 2, |_| "field"));
    failures.extend(check(&m, TableId::MemberRef, 2, member_kind));
    failures.extend(check(&m, TableId::StandAloneSig, 0, member_kind));
    failures.extend(check(&m, TableId::TypeSpec, 0, |_| "type"));
    assert!(
        failures.is_empty(),
        "{} signatures failed, first 10:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[cfg_attr(
    not(has_mono_corlib),
    ignore = "Mono mscorlib.dll not found (set MONO_CORLIB)"
)]
#[test]
fn every_mscorlib_method_body_was_decoded() {
    let path = option_env!("MONO_CORLIB").unwrap();
    let m = read_assembly(&std::fs::read(path).unwrap()).unwrap();
    let with_bodies = m
        .table(TableId::MethodDef)
        .iter()
        .filter(|r| r[0].as_body().is_some())
        .count();
    assert!(with_bodies > 10_000, "only {with_bodies} bodies decoded");
}
