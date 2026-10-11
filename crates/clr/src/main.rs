//! `clr [-corlib <path>] <assembly.exe> [args...]` — runs a CLI assembly on
//! the clean-room runtime (spec §5.3, exit codes §5.4).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const EXIT_USAGE: u8 = 64;
const USAGE: &str = "usage: clr [-corlib <path>] <assembly.exe> [args...]";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    corlib: Option<PathBuf>,
    assembly: PathBuf,
    /// Passed to `Main(string[] args)` verbatim, even if they start with `-`.
    args: Vec<String>,
}

fn parse_args(args: &[OsString]) -> Result<Options, String> {
    let mut corlib = None;
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if arg == "-corlib" {
            let path = args.get(i + 1).ok_or("-corlib needs a path")?;
            corlib = Some(PathBuf::from(path));
            i += 2;
        } else if let Some(s) = arg.to_str() {
            if s.starts_with('-') {
                return Err(format!("unknown option {s}"));
            } else {
                break;
            }
        } else {
            // Non-UTF-8 option name (not a recognized option)
            break;
        }
    }
    let assembly = args.get(i).ok_or("missing assembly path")?;
    Ok(Options {
        corlib,
        assembly: PathBuf::from(assembly),
        args: args[i + 1..]
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
    })
}

/// `mscorlib.dll` next to the `clr` binary.
fn default_corlib(exe: &Path) -> PathBuf {
    exe.with_file_name("mscorlib.dll")
}

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let options = match parse_args(&argv) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("clr: {problem}\n{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let corlib = match options.corlib {
        Some(path) => path,
        None => match std::env::current_exe() {
            Ok(exe) => default_corlib(&exe),
            Err(e) => {
                eprintln!("clr: cannot locate mscorlib.dll next to clr: {e}");
                return ExitCode::from(runtime::error::EXIT_LOAD as u8);
            }
        },
    };
    let mut out = std::io::stdout().lock();
    match runtime::run_files(&options.assembly, &corlib, &options.args, &mut out) {
        // Process exit codes are 0-255 on Unix; `int Main` values wrap like on .NET.
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            let _ = std::io::Write::flush(&mut out);
            eprintln!("clr: {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<OsString> {
        args.iter().map(|a| OsString::from(*a)).collect()
    }

    #[test]
    fn assembly_and_trailing_args() {
        let o = parse_args(&strings(&["app.exe", "one", "-two"])).unwrap();
        assert_eq!(
            o,
            Options {
                corlib: None,
                assembly: PathBuf::from("app.exe"),
                args: vec!["one".to_owned(), "-two".to_owned()]
            }
        );
    }

    #[test]
    fn corlib_option() {
        let o = parse_args(&strings(&["-corlib", "lib/mscorlib.dll", "app.exe"])).unwrap();
        assert_eq!(o.corlib, Some(PathBuf::from("lib/mscorlib.dll")));
        assert_eq!(o.assembly, PathBuf::from("app.exe"));
        assert!(o.args.is_empty());
    }

    #[test]
    fn usage_errors() {
        assert_eq!(parse_args(&[]), Err("missing assembly path".to_owned()));
        assert_eq!(
            parse_args(&strings(&["-corlib"])),
            Err("-corlib needs a path".to_owned())
        );
        assert_eq!(
            parse_args(&strings(&["-verbose", "app.exe"])),
            Err("unknown option -verbose".to_owned())
        );
    }

    #[test]
    fn default_corlib_sits_next_to_the_binary() {
        assert_eq!(
            default_corlib(Path::new("/opt/clr/bin/clr")),
            PathBuf::from("/opt/clr/bin/mscorlib.dll")
        );
    }

    #[test]
    fn non_utf8_arguments() {
        use std::os::unix::ffi::OsStringExt;
        let args = vec![
            OsString::from_vec(vec![b'a', 0xFF, b'.', b'e', b'x', b'e']),
            OsString::from_vec(vec![0xFE]),
        ];
        let o = parse_args(&args).unwrap();
        assert_eq!(
            o.assembly,
            PathBuf::from(OsString::from_vec(vec![b'a', 0xFF, b'.', b'e', b'x', b'e']))
        );
        assert_eq!(o.args, vec!["\u{FFFD}".to_owned()]);
    }
}
