//! Program tests, spec §6 config R: programs compiled by `mcs` against
//! Mono's mscorlib, corlib compiled by `mcs -nostdlib`, run on our `clr`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Passed to every program, so `Main(string[] args)` gets a real array.
const PROGRAM_ARGS: [&str; 2] = ["one", "two"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Program {
    name: String,
    source: PathBuf,
    expected_stdout: String,
    expected_exit: i32,
    /// Substring of stderr; only programs that fail on our runtime have one.
    expected_stderr: Option<String>,
}

fn programs() -> Vec<Program> {
    let dir = repo_root().join("tests/programs");
    let mut list: Vec<Program> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "cs"))
        .map(|source| {
            let read = |ext: &str| std::fs::read_to_string(source.with_extension(ext)).ok();
            Program {
                name: source.file_stem().unwrap().to_string_lossy().into_owned(),
                expected_stdout: read("expected")
                    .unwrap_or_else(|| panic!("{} has no .expected file", source.display())),
                expected_exit: read("exitcode").map_or(0, |s| s.trim().parse().unwrap()),
                expected_stderr: read("stderr").map(|s| s.trim_end().to_owned()),
                source,
            }
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    assert!(!list.is_empty(), "no programs in {}", dir.display());
    list
}

fn mcs(args: &[String]) {
    let out = Command::new("mcs").args(args).output().expect("run mcs");
    assert!(
        out.status.success(),
        "mcs {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Builds `corlib/*.cs` into `<dir>/mscorlib.dll`; AssemblyInfo.cs goes first (CS1730).
fn build_corlib(dir: &Path) -> PathBuf {
    let corlib_dir = repo_root().join("corlib");
    let first = corlib_dir.join("AssemblyInfo.cs");
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&corlib_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "cs") && *path != first)
        .collect();
    sources.sort();
    sources.insert(0, first);
    let out = dir.join("mscorlib.dll");
    let mut args: Vec<String> = ["-nostdlib", "-target:library", "-nowarn:626"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    args.push(format!("-out:{}", out.display()));
    args.extend(sources.iter().map(|s| s.display().to_string()));
    mcs(&args);
    out
}

/// Compiles a program against Mono's own mscorlib (that is config R).
fn compile(program: &Program, dir: &Path) -> PathBuf {
    let exe = dir.join(format!("{}.exe", program.name));
    mcs(&[
        format!("-out:{}", exe.display()),
        program.source.display().to_string(),
    ]);
    exe
}

#[cfg_attr(not(has_mcs), ignore = "mcs not installed: config R skipped")]
#[test]
fn programs_run_on_clr() {
    let dir = scratch("config-r");
    let corlib = build_corlib(&dir);
    let mut failures = Vec::new();
    for program in programs() {
        let exe = compile(&program, &dir);
        let out = Command::new(env!("CARGO_BIN_EXE_clr"))
            .arg("-corlib")
            .arg(&corlib)
            .arg(&exe)
            .args(PROGRAM_ARGS)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let name = &program.name;
        if stdout != program.expected_stdout {
            failures.push(format!(
                "{name}: stdout {stdout:?}, expected {:?}",
                program.expected_stdout
            ));
        }
        if out.status.code() != Some(program.expected_exit) {
            failures.push(format!(
                "{name}: exit {:?}, expected {} (stderr: {stderr})",
                out.status.code(),
                program.expected_exit
            ));
        }
        match &program.expected_stderr {
            Some(want) if !stderr.contains(want.as_str()) => {
                failures.push(format!("{name}: stderr {stderr:?} lacks {want:?}"));
            }
            None if !stderr.is_empty() => {
                failures.push(format!("{name}: unexpected stderr {stderr:?}"));
            }
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Validates the .expected/.exitcode files themselves on Mono, for every
/// program that is meant to succeed.
#[cfg_attr(
    not(all(has_mcs, has_mono)),
    ignore = "mcs/mono not installed: expected-file check skipped"
)]
#[test]
fn expected_files_match_mono() {
    let dir = scratch("mono-check");
    for program in programs()
        .into_iter()
        .filter(|p| p.expected_stderr.is_none())
    {
        let exe = compile(&program, &dir);
        let out = Command::new("mono")
            .arg(&exe)
            .args(PROGRAM_ARGS)
            .env("LANG", "C.UTF-8")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            program.expected_stdout,
            "{} on mono",
            program.name
        );
        assert_eq!(
            out.status.code(),
            Some(program.expected_exit),
            "{} exit code on mono",
            program.name
        );
    }
}
