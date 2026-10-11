//! Rust implementations of corlib methods that have no IL body (spec §5.3
//! "Internal calls"), keyed like `methods::method_key`.

use std::io::Write;

use crate::error::{Result, RuntimeError};
use crate::heap::Heap;
use crate::types::TypeId;
use crate::value::StackValue;

/// What an internal call may touch.
pub struct CallContext<'a> {
    pub heap: &'a mut Heap,
    pub out: &'a mut dyn Write,
    /// `System.String`, to check that string arguments really are strings.
    pub string_type: TypeId,
}

pub type InternalCall = fn(&mut CallContext<'_>, &[StackValue]) -> Result<Option<StackValue>>;

pub fn lookup(key: &str) -> Option<InternalCall> {
    match key {
        "System.Console::WriteLine(string)" => Some(console_write_line),
        _ => None,
    }
}

/// Writes the UTF-16 string as UTF-8 plus `\n`; null writes an empty line.
/// Unpaired surrogates become U+FFFD.
fn console_write_line(
    ctx: &mut CallContext<'_>,
    args: &[StackValue],
) -> Result<Option<StackValue>> {
    let text = match args {
        [StackValue::Obj(None)] => String::new(),
        [StackValue::Obj(Some(s))] if ctx.heap.type_of(*s) == ctx.string_type => {
            String::from_utf16_lossy(&ctx.heap.string_units(*s))
        }
        _ => {
            return Err(RuntimeError::invalid_program(
                "System.Console::WriteLine(string) expects one string argument",
            ));
        }
    };
    writeln!(ctx.out, "{text}").map_err(|e| RuntimeError::Io(e.to_string()))?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Result, RuntimeError};
    use crate::heap::Heap;
    use crate::types::TypeId;
    use crate::value::StackValue;
    use std::io::Write;

    const STRING: TypeId = TypeId(1);
    const ARRAY: TypeId = TypeId(2);

    fn write_line(heap: &mut Heap, args: &[StackValue]) -> (Result<Option<StackValue>>, String) {
        let f = lookup("System.Console::WriteLine(string)").expect("registered");
        let mut out = Vec::new();
        let result = f(
            &mut CallContext {
                heap,
                out: &mut out,
                string_type: STRING,
            },
            args,
        );
        (result, String::from_utf8(out).unwrap())
    }

    #[test]
    fn write_line_prints_utf8_and_newline() {
        let mut heap = Heap::new();
        let units: Vec<u16> = "Grüße, 世界 😀".encode_utf16().collect();
        let s = heap.alloc_string(STRING, &units).unwrap();
        let (result, out) = write_line(&mut heap, &[StackValue::Obj(Some(s))]);
        assert_eq!(result, Ok(None));
        assert_eq!(out, "Grüße, 世界 😀\n");
    }

    #[test]
    fn write_line_of_null_prints_an_empty_line() {
        let (result, out) = write_line(&mut Heap::new(), &[StackValue::Obj(None)]);
        assert_eq!(result, Ok(None));
        assert_eq!(out, "\n");
    }

    #[test]
    fn write_line_rejects_non_string_objects() {
        let mut heap = Heap::new();
        let array = heap.alloc_ref_array(ARRAY, &[None]).unwrap();
        let (result, out) = write_line(&mut heap, &[StackValue::Obj(Some(array))]);
        assert!(
            matches!(result, Err(RuntimeError::InvalidProgram { .. })),
            "{result:?}"
        );
        assert_eq!(out, "");
        let (result, _) = write_line(&mut heap, &[StackValue::I32(1)]);
        assert!(matches!(result, Err(RuntimeError::InvalidProgram { .. })));
        let (result, _) = write_line(&mut heap, &[]);
        assert!(matches!(result, Err(RuntimeError::InvalidProgram { .. })));
    }

    #[test]
    fn write_failures_are_io_errors() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("pipe closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let f = lookup("System.Console::WriteLine(string)").unwrap();
        let mut heap = Heap::new();
        let result = f(
            &mut CallContext {
                heap: &mut heap,
                out: &mut Broken,
                string_type: STRING,
            },
            &[StackValue::Obj(None)],
        );
        assert!(matches!(result, Err(RuntimeError::Io(_))), "{result:?}");
    }

    #[test]
    fn unknown_keys_are_not_registered() {
        assert!(lookup("System.Console::Write(string)").is_none());
        assert!(lookup("System.Console::WriteLine(object)").is_none());
    }
}
