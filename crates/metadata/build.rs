//! Detects the Mono oracle tools so tests depending on them are ignored
//! (visibly, with a reason) instead of silently passing.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_mono)");
    println!("cargo::rustc-check-cfg=cfg(has_mono_corlib)");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=MONO_CORLIB");

    let mono_runs = Command::new("mono")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if mono_runs {
        println!("cargo::rustc-cfg=has_mono");
    } else {
        println!("cargo::warning=mono not found on PATH: mono oracle tests will be ignored");
    }

    let corlib = std::env::var("MONO_CORLIB")
        .unwrap_or_else(|_| "/usr/lib/mono/4.5/mscorlib.dll".to_owned());
    if Path::new(&corlib).is_file() {
        println!("cargo::rustc-cfg=has_mono_corlib");
        println!("cargo::rustc-env=MONO_CORLIB={corlib}");
    } else {
        println!(
            "cargo::warning=Mono mscorlib.dll not found at {corlib} (set MONO_CORLIB): corlib tests will be ignored"
        );
    }
}
