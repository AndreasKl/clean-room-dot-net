//! Error exits of the `clr` binary that need no Mono tools (spec §5.4).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn clr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_clr"))
        .args(args)
        .output()
        .expect("run clr")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../metadata/tests/fixtures")
        .join(name)
}

#[test]
fn no_arguments_is_a_usage_error() {
    let out = clr(&[]);
    assert_eq!(out.status.code(), Some(64));
    assert!(
        stderr(&out).contains("usage: clr [-corlib <path>] <assembly.exe> [args...]"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn missing_assembly_exits_66() {
    let out = clr(&[
        "-corlib",
        "/nonexistent/mscorlib.dll",
        "/nonexistent/app.exe",
    ]);
    assert_eq!(out.status.code(), Some(66));
    assert!(
        stderr(&out).starts_with("clr: /nonexistent/app.exe: "),
        "{}",
        stderr(&out)
    );
}

#[test]
fn non_pe_assembly_exits_66() {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("not-a-pe.exe");
    std::fs::write(&path, b"hello").unwrap();
    let out = clr(&[
        "-corlib",
        "/nonexistent/mscorlib.dll",
        path.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(66));
    assert!(
        stderr(&out).starts_with(&format!("clr: {}: bad image: ", path.display())),
        "{}",
        stderr(&out)
    );
}

#[test]
fn missing_corlib_exits_66() {
    let hello = fixture("hello.exe");
    let out = clr(&[
        "-corlib",
        "/nonexistent/mscorlib.dll",
        hello.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(66));
    assert!(
        stderr(&out).starts_with("clr: /nonexistent/mscorlib.dll: "),
        "{}",
        stderr(&out)
    );
}

#[test]
fn corlib_with_another_name_exits_66() {
    let hello = fixture("hello.exe");
    let lib = fixture("lib.dll");
    let out = clr(&["-corlib", lib.to_str().unwrap(), hello.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(66));
    assert!(
        stderr(&out).contains("expected assembly 'mscorlib', found 'lib'"),
        "{}",
        stderr(&out)
    );
}
