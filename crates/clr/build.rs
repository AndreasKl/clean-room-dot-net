//! Detects the Mono tools used by the program tests, so tests needing them
//! are ignored (visibly, with a reason) instead of silently passing.

use std::process::Command;

fn detect(tool: &str, cfg: &str) {
    let runs = Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if runs {
        println!("cargo::rustc-cfg={cfg}");
    } else {
        println!("cargo::warning={tool} not found on PATH: tests needing it will be ignored");
    }
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_mcs)");
    println!("cargo::rustc-check-cfg=cfg(has_mono)");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=PATH");
    detect("mcs", "has_mcs");
    detect("mono", "has_mono");
}
