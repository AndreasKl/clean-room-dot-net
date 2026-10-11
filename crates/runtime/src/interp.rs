//! IL interpreter (spec §5.3 "Interpreter"): explicit frame stack, typed
//! evaluation stack, lazy resolution of call targets.

use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

use metadata::TableId;
use metadata::sig::{TypeSig, conv};
use metadata::tables::col;

use crate::error::{Location, Result, RuntimeError};
use crate::heap::{Heap, ObjRef};
use crate::internal_calls::{self, CallContext};
use crate::loader::{Assemblies, AssemblyId};
use crate::methods::{self, MethodId};
use crate::opcodes::{self as op, opcode_name};
use crate::types::{TypeId, TypeSystem};
use crate::value::StackValue;

/// Managed call depth limit. Exceeding it ends the program (there are no
/// exceptions yet to raise StackOverflowException).
pub const MAX_FRAMES: usize = 10_000;

struct Frame {
    method: MethodId,
    code: Rc<[u8]>,
    ip: usize,
    args: Vec<StackValue>,
    stack: Vec<StackValue>,
    returns_value: bool,
}

impl Frame {
    fn operand(&mut self, len: usize) -> Result<&[u8]> {
        let end = self
            .ip
            .checked_add(len)
            .filter(|&end| end <= self.code.len())
            .ok_or_else(|| RuntimeError::invalid_program("unexpected end of method body"))?;
        let bytes = &self.code[self.ip..end];
        self.ip = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.operand(1)?[0])
    }

    fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.operand(4)?.try_into().expect("4-byte operand"),
        ))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn push(&mut self, value: StackValue) {
        self.stack.push(value);
    }

    fn arg(&self, index: usize) -> Result<StackValue> {
        self.args.get(index).copied().ok_or_else(|| {
            RuntimeError::invalid_program(format!("argument {index} does not exist"))
        })
    }
}

enum Flow {
    Continue,
    /// The outermost frame returned this value.
    Exit(Option<StackValue>),
}

pub struct Interpreter<'o> {
    asms: Assemblies,
    types: TypeSystem,
    heap: Heap,
    /// `ldstr` results, interned per (module, token).
    strings: HashMap<(AssemblyId, u32), ObjRef>,
    frames: Vec<Frame>,
    out: &'o mut dyn Write,
}

impl<'o> Interpreter<'o> {
    pub fn new(asms: Assemblies, out: &'o mut dyn Write) -> Self {
        Self {
            asms,
            types: TypeSystem::new(),
            heap: Heap::new(),
            strings: HashMap::new(),
            frames: Vec::new(),
            out,
        }
    }

    /// Runs the entry point of `entry` and returns the process exit code:
    /// 0 for `void Main`, the return value for `int Main`.
    pub fn run_entry(&mut self, entry: AssemblyId, args: &[String]) -> Result<i32> {
        let assembly = self.asms.get(entry);
        let token = assembly.model.entry_point_token;
        if token == 0 {
            return Err(assembly.bad_image("assembly has no entry point"));
        }
        let main = MethodId {
            asm: entry,
            row: token & 0x00FF_FFFF,
        };
        let sig = methods::method_sig(&self.asms, main)?;
        let supported_return = matches!(sig.ret, TypeSig::Void | TypeSig::I4);
        let main_args = match sig.params.as_slice() {
            [] => Vec::new(),
            [TypeSig::SzArray(element)] if **element == TypeSig::String => {
                vec![StackValue::Obj(Some(self.args_array(args)?))]
            }
            _ => Vec::new(),
        };
        if sig.conv != conv::DEFAULT || !supported_return || main_args.len() != sig.params.len() {
            return Err(RuntimeError::not_implemented(format!(
                "entry point signature of {}",
                methods::method_key(&self.asms, main)?
            )));
        }
        self.push_frame(main, main_args, sig.ret != TypeSig::Void)?;
        let result = self.execute()?;
        self.out
            .flush()
            .map_err(|e| RuntimeError::Io(e.to_string()))?;
        Ok(match result {
            Some(StackValue::I32(code)) => code,
            _ => 0,
        })
    }

    fn execute(&mut self) -> Result<Option<StackValue>> {
        loop {
            let frame = self.frames.last().expect("execute runs with a frame");
            let (method, at) = (frame.method, frame.ip as u32);
            match self.step() {
                Ok(Flow::Continue) => {}
                Ok(Flow::Exit(value)) => return Ok(value),
                Err(e) => {
                    let name = methods::method_name(&self.asms, method)
                        .unwrap_or_else(|_| "<unknown method>".to_owned());
                    return Err(e.at(Location {
                        method: name,
                        il_offset: at,
                    }));
                }
            }
        }
    }

    fn top(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a frame is executing")
    }

    fn step(&mut self) -> Result<Flow> {
        let frame = self.top();
        let opcode = frame.u8()?;
        match opcode {
            op::NOP => {}
            op::LDARG_0..=op::LDARG_3 => {
                let value = frame.arg(usize::from(opcode - op::LDARG_0))?;
                frame.push(value);
            }
            op::LDARG_S => {
                let index = frame.u8()?;
                let value = frame.arg(usize::from(index))?;
                frame.push(value);
            }
            op::LDNULL => frame.push(StackValue::Obj(None)),
            op::LDC_I4_M1..=op::LDC_I4_8 => {
                frame.push(StackValue::I32(i32::from(opcode) - i32::from(op::LDC_I4_0)));
            }
            op::LDC_I4_S => {
                let value = frame.i8()?;
                frame.push(StackValue::I32(i32::from(value)));
            }
            op::LDC_I4 => {
                let value = frame.i32()?;
                frame.push(StackValue::I32(value));
            }
            op::LDSTR => {
                let token = frame.u32()?;
                let asm = frame.method.asm;
                let s = self.ldstr(asm, token)?;
                self.top().push(StackValue::Obj(Some(s)));
            }
            op::CALL => {
                let token = frame.u32()?;
                let asm = frame.method.asm;
                let callee = self.resolve_call(asm, token)?;
                self.call(callee)?;
            }
            op::RET => return self.ret(),
            op::PREFIX_FE => {
                let second = frame.u8()?;
                return Err(RuntimeError::not_implemented(format!(
                    "opcode 0xFE 0x{second:02X}"
                )));
            }
            other => {
                return Err(RuntimeError::not_implemented(format!(
                    "opcode 0x{other:02X} ({})",
                    opcode_name(other)
                )));
            }
        }
        Ok(Flow::Continue)
    }

    fn string_type(&mut self) -> Result<TypeId> {
        self.types.corlib_type(&self.asms, "System", "String")
    }

    /// §III.4.16: a string object for a #US token, interned per (module, token).
    fn ldstr(&mut self, asm: AssemblyId, token: u32) -> Result<ObjRef> {
        if token >> 24 != 0x70 {
            return Err(RuntimeError::invalid_program(format!(
                "ldstr operand {token:#010x} is not a string token"
            )));
        }
        if let Some(&s) = self.strings.get(&(asm, token)) {
            return Ok(s);
        }
        let assembly = self.asms.get(asm);
        let units = assembly
            .model
            .user_string(token)
            .ok_or_else(|| {
                assembly.bad_image(format!("ldstr token {token:#010x} has no #US entry"))
            })?
            .to_vec();
        let string_type = self.string_type()?;
        let s = self.heap.alloc_string(string_type, &units)?;
        self.strings.insert((asm, token), s);
        Ok(s)
    }

    /// The `string[]` passed to `Main(string[] args)`.
    fn args_array(&mut self, args: &[String]) -> Result<ObjRef> {
        let string_type = self.string_type()?;
        let array_base = self.types.corlib_type(&self.asms, "System", "Array")?;
        let array_type = self.types.szarray(string_type, array_base);
        let mut elements = Vec::with_capacity(args.len());
        for arg in args {
            let units: Vec<u16> = arg.encode_utf16().collect();
            elements.push(Some(self.heap.alloc_string(string_type, &units)?));
        }
        self.heap.alloc_ref_array(array_type, &elements)
    }

    fn resolve_call(&self, asm: AssemblyId, token: u32) -> Result<MethodId> {
        let row = token & 0x00FF_FFFF;
        match token >> 24 {
            0x06 => {
                self.asms.get(asm).row(TableId::MethodDef, row)?;
                Ok(MethodId { asm, row })
            }
            0x0A => methods::resolve_member_ref(&self.asms, asm, row),
            0x2B => Err(RuntimeError::not_implemented(
                "generic method calls (MethodSpec)",
            )),
            _ => Err(RuntimeError::invalid_program(format!(
                "call operand {token:#010x} is not a method token"
            ))),
        }
    }

    /// §III.3.19: pops the arguments, then runs the callee's IL in a new
    /// frame or, for a body-less corlib method, its internal call.
    fn call(&mut self, callee: MethodId) -> Result<()> {
        let sig = methods::method_sig(&self.asms, callee)?;
        if sig.conv & conv::HAS_THIS != 0 {
            return Err(RuntimeError::not_implemented(format!(
                "instance method call {}",
                methods::method_key(&self.asms, callee)?
            )));
        }
        if sig.conv != conv::DEFAULT {
            return Err(RuntimeError::not_implemented(format!(
                "calling convention {:#04x} of {}",
                sig.conv,
                methods::method_key(&self.asms, callee)?
            )));
        }
        let argc = sig.params.len();
        let available = self.top().stack.len();
        if available < argc {
            return Err(RuntimeError::invalid_program(format!(
                "stack underflow: {} takes {argc} arguments, {available} on the stack",
                methods::method_name(&self.asms, callee)?
            )));
        }
        let args = self.top().stack.split_off(available - argc);
        let has_body = self
            .asms
            .get(callee.asm)
            .row(TableId::MethodDef, callee.row)?[col::method_def::BODY]
            .as_body()
            .is_some();
        if has_body {
            return self.push_frame(callee, args, sig.ret != TypeSig::Void);
        }
        let key = methods::method_key(&self.asms, callee)?;
        if callee.asm != self.asms.corlib() {
            return Err(RuntimeError::not_implemented(format!(
                "method without IL body {key}"
            )));
        }
        let internal_call = internal_calls::lookup(&key)
            .ok_or_else(|| RuntimeError::not_implemented(format!("internal call {key}")))?;
        let string_type = self.string_type()?;
        let mut ctx = CallContext {
            heap: &mut self.heap,
            out: &mut *self.out,
            string_type,
        };
        if let Some(value) = internal_call(&mut ctx, &args)? {
            self.top().push(value);
        }
        Ok(())
    }

    fn push_frame(
        &mut self,
        method: MethodId,
        args: Vec<StackValue>,
        returns_value: bool,
    ) -> Result<()> {
        if self.frames.len() >= MAX_FRAMES {
            return Err(RuntimeError::not_implemented(format!(
                "call depth above {MAX_FRAMES} (stack overflow; exceptions are not supported yet)"
            )));
        }
        let row = self
            .asms
            .get(method.asm)
            .row(TableId::MethodDef, method.row)?;
        let Some(body) = row[col::method_def::BODY].as_body() else {
            return Err(RuntimeError::not_implemented(format!(
                "method without IL body {}",
                methods::method_name(&self.asms, method)?
            )));
        };
        let code: Rc<[u8]> = Rc::from(body.code.as_slice());
        self.frames.push(Frame {
            method,
            code,
            ip: 0,
            args,
            stack: Vec::new(),
            returns_value,
        });
        Ok(())
    }

    /// §III.3.56: the evaluation stack must hold exactly the return value.
    fn ret(&mut self) -> Result<Flow> {
        let frame = self.top();
        let expected = usize::from(frame.returns_value);
        if frame.stack.len() != expected {
            return Err(RuntimeError::invalid_program(format!(
                "ret with {} values on the stack, expected {expected}",
                frame.stack.len()
            )));
        }
        let value = frame.stack.pop();
        self.frames.pop();
        match self.frames.last_mut() {
            None => Ok(Flow::Exit(value)),
            Some(caller) => {
                if let Some(v) = value {
                    caller.push(v);
                }
                Ok(Flow::Continue)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcodes::*;
    use crate::testing::{
        INT_NO_ARGS, VOID_NO_ARGS, VOID_STRING, WRITE_LINE, app, corlib_builder, il, run, run_with,
        test_corlib,
    };
    use metadata::Cell;

    fn print(token: u32) -> Vec<u8> {
        il(&[
            &[LDSTR],
            &token.to_le_bytes(),
            &[CALL],
            &WRITE_LINE.to_le_bytes(),
        ])
    }

    fn error_of(result: Result<i32>) -> RuntimeError {
        result.expect_err("program should fail")
    }

    #[test]
    fn prints_hello() {
        let mut b = app();
        let hello = b.user_string("Hello, World!");
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&print(hello), &[RET]])));
        b.entry_point(main);
        assert_eq!(run(b, &[]), (Ok(0), "Hello, World!\n".to_owned()));
    }

    #[test]
    fn int_main_returns_exit_code() {
        let cases: &[(&[u8], i32)] = &[
            (&[LDC_I4_M1, RET], -1),
            (&[LDC_I4_0, RET], 0),
            (&[LDC_I4_0 + 3, RET], 3),
            (&[LDC_I4_8, RET], 8),
            (&[LDC_I4_S, 0xF6, RET], -10),
            (&[LDC_I4, 0x40, 0x42, 0x0F, 0x00, RET], 1_000_000),
        ];
        for (code, expected) in cases {
            let mut b = app();
            let main = b.method("Main", INT_NO_ARGS, Some(code));
            b.entry_point(main);
            assert_eq!(run(b, &[]).0, Ok(*expected), "{code:02x?}");
        }
    }

    #[test]
    fn static_calls_pass_arguments_and_return_values() {
        let mut b = app();
        let ada = b.user_string("Ada");
        let hi = b.user_string("Hi");
        // MethodDef 1: string Name() => "Ada"
        b.method(
            "Name",
            &[0x00, 0x00, 0x0E],
            Some(&il(&[&[LDSTR], &ada.to_le_bytes(), &[RET]])),
        );
        // MethodDef 2: void Greet(string who, string greeting) prints greeting then who
        b.method(
            "Greet",
            &[0x00, 0x02, 0x01, 0x0E, 0x0E],
            Some(&il(&[
                &[LDARG_0 + 1, CALL],
                &WRITE_LINE.to_le_bytes(),
                &[LDARG_0, CALL],
                &WRITE_LINE.to_le_bytes(),
                &[RET],
            ])),
        );
        // MethodDef 3: Main() { Greet(Name(), "Hi"); }
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[
                &[CALL],
                &0x0600_0001u32.to_le_bytes(),
                &[LDSTR],
                &hi.to_le_bytes(),
                &[CALL],
                &0x0600_0002u32.to_le_bytes(),
                &[RET],
            ])),
        );
        b.entry_point(main);
        assert_eq!(run(b, &[]), (Ok(0), "Hi\nAda\n".to_owned()));
    }

    #[test]
    fn ldnull_and_ldarg_s() {
        let mut b = app();
        b.method(
            "Show",
            VOID_STRING,
            Some(&il(&[
                &[LDARG_S, 0, CALL],
                &WRITE_LINE.to_le_bytes(),
                &[RET],
            ])),
        );
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[
                &[NOP, LDNULL, CALL],
                &0x0600_0001u32.to_le_bytes(),
                &[RET],
            ])),
        );
        b.entry_point(main);
        assert_eq!(run(b, &[]), (Ok(0), "\n".to_owned()));
    }

    #[test]
    fn main_with_string_array_receives_arguments() {
        let mut b = app();
        let main = b.method("Main", &[0x00, 0x01, 0x01, 0x1D, 0x0E], Some(&[RET]));
        b.entry_point(main);
        assert_eq!(run(b, &["one", "two"]), (Ok(0), String::new()));
    }

    #[test]
    fn args_array_holds_the_strings() {
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        asms.add(app().build());
        let mut out = Vec::new();
        let mut interp = Interpreter::new(asms, &mut out);
        let array = interp
            .args_array(&["a".to_owned(), "bé".to_owned()])
            .unwrap();
        let array_type = interp.heap.type_of(array);
        assert_eq!(interp.types.get(array_type).full_name(), "System.String[]");
        let elements = interp.heap.ref_array_elements(array);
        let texts: Vec<String> = elements
            .iter()
            .map(|e| String::from_utf16(&interp.heap.string_units(e.unwrap())).unwrap())
            .collect();
        assert_eq!(texts, ["a", "bé"]);
    }

    #[test]
    fn ldstr_is_interned_per_token() {
        let mut b = app();
        let a = b.user_string("a");
        let other = b.user_string("b");
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&print(a), &print(a), &print(other), &[RET]])),
        );
        b.entry_point(main);
        let mut asms = Assemblies::new(test_corlib()).unwrap();
        let entry = asms.add(b.build());
        let mut out = Vec::new();
        let mut interp = Interpreter::new(asms, &mut out);
        assert_eq!(interp.run_entry(entry, &[]), Ok(0));
        assert_eq!(interp.strings.len(), 2);
        drop(interp);
        assert_eq!(String::from_utf8(out).unwrap(), "a\na\nb\n");
    }

    #[test]
    fn unresolvable_references_are_ignored_until_used() {
        // Like mcs output: a TypeRef/MemberRef our corlib cannot satisfy.
        let mut b = app();
        let attr = b.type_ref(
            1,
            "System.Runtime.CompilerServices",
            "RuntimeCompatibilityAttribute",
        );
        b.member_ref(
            Cell::Ref(TableId::TypeRef, attr),
            ".ctor",
            &[0x20, 0x00, 0x01],
        );
        let hello = b.user_string("still runs");
        let main = b.method("Main", VOID_NO_ARGS, Some(&il(&[&print(hello), &[RET]])));
        b.entry_point(main);
        assert_eq!(run(b, &[]), (Ok(0), "still runs\n".to_owned()));
    }

    #[test]
    fn unsupported_opcode_reports_spec_format() {
        let mut b = app();
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&[NOP, NEWOBJ, 1, 0, 0, 0x0A, RET]),
        );
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(
            err.to_string(),
            "not implemented: opcode 0x73 (newobj) in App::Main @IL_0001"
        );
        assert_eq!(err.exit_code(), 70);
    }

    #[test]
    fn two_byte_opcodes_are_not_implemented() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[PREFIX_FE, 0x01, RET]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(
            err.to_string(),
            "not implemented: opcode 0xFE 0x01 in App::Main @IL_0000"
        );
    }

    #[test]
    fn missing_method_reports_spec_format() {
        let mut b = app();
        let write = b.member_ref(Cell::Ref(TableId::TypeRef, 2), "Write", VOID_STRING);
        let x = b.user_string("x");
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[
                &[LDSTR],
                &x.to_le_bytes(),
                &[CALL],
                &TableId::MemberRef.token(write).to_le_bytes(),
                &[RET],
            ])),
        );
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(
            err.to_string(),
            "not implemented: method System.Console::Write(string) in App::Main @IL_0005"
        );
    }

    #[test]
    fn missing_internal_call_is_reported() {
        let mut corlib = corlib_builder();
        corlib.method("Beep", VOID_NO_ARGS, None);
        let mut b = app();
        let beep = b.member_ref(Cell::Ref(TableId::TypeRef, 2), "Beep", VOID_NO_ARGS);
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[
                &[CALL],
                &TableId::MemberRef.token(beep).to_le_bytes(),
                &[RET],
            ])),
        );
        b.entry_point(main);
        let err = error_of(run_with(corlib.build(), b, &[]).0);
        assert_eq!(
            err.to_string(),
            "not implemented: internal call System.Console::Beep() in App::Main @IL_0000"
        );
    }

    #[test]
    fn instance_calls_are_not_implemented() {
        let mut corlib = corlib_builder();
        corlib.method("Clear", &[0x20, 0x00, 0x01], Some(&[RET]));
        let mut b = app();
        let clear = b.member_ref(Cell::Ref(TableId::TypeRef, 2), "Clear", &[0x20, 0x00, 0x01]);
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[
                &[LDNULL, CALL],
                &TableId::MemberRef.token(clear).to_le_bytes(),
                &[RET],
            ])),
        );
        b.entry_point(main);
        let err = error_of(run_with(corlib.build(), b, &[]).0);
        assert!(
            err.to_string()
                .starts_with("not implemented: instance method call System.Console::Clear()"),
            "{err}"
        );
    }

    #[test]
    fn unbounded_recursion_is_reported() {
        let mut b = app();
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&[CALL], &0x0600_0001u32.to_le_bytes(), &[RET]])),
        );
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(
            err.to_string()
                .contains(&format!("call depth above {MAX_FRAMES}")),
            "{err}"
        );
        assert_eq!(err.exit_code(), 70);
    }

    #[test]
    fn truncated_operand_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[LDSTR, 0x01, 0x00]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(
            err.to_string(),
            "invalid program: unexpected end of method body in App::Main @IL_0000"
        );
    }

    #[test]
    fn falling_off_the_end_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[NOP]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(matches!(err, RuntimeError::InvalidProgram { .. }), "{err}");
    }

    #[test]
    fn call_to_missing_member_ref_is_a_load_error() {
        let mut b = app();
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&[CALL], &0x0A00_0063u32.to_le_bytes(), &[RET]])),
        );
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(err.exit_code(), 66, "{err}");
        assert!(
            err.to_string().contains("MemberRef row 99 does not exist"),
            "{err}"
        );
    }

    #[test]
    fn call_with_non_method_token_is_invalid_program() {
        let mut b = app();
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&[CALL], &0x0200_0001u32.to_le_bytes(), &[RET]])),
        );
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(matches!(err, RuntimeError::InvalidProgram { .. }), "{err}");
    }

    #[test]
    fn ldstr_with_non_string_token_is_invalid_program() {
        let mut b = app();
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&[LDSTR], &0x0A00_0001u32.to_le_bytes(), &[RET]])),
        );
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(matches!(err, RuntimeError::InvalidProgram { .. }), "{err}");
    }

    #[test]
    fn ldstr_with_missing_us_entry_is_a_load_error() {
        let mut b = app();
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&[LDSTR], &0x7000_0042u32.to_le_bytes(), &[RET]])),
        );
        b.entry_point(main);
        assert_eq!(error_of(run(b, &[]).0).exit_code(), 66);
    }

    #[test]
    fn stack_underflow_is_invalid_program() {
        let mut b = app();
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&[CALL], &WRITE_LINE.to_le_bytes(), &[RET]])),
        );
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(
            err.to_string()
                .starts_with("invalid program: stack underflow"),
            "{err}"
        );
    }

    #[test]
    fn ret_with_extra_values_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[LDNULL, RET]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert!(matches!(err, RuntimeError::InvalidProgram { .. }), "{err}");
    }

    #[test]
    fn missing_argument_is_invalid_program() {
        let mut b = app();
        let main = b.method("Main", VOID_NO_ARGS, Some(&[LDARG_0 + 1, RET]));
        b.entry_point(main);
        let err = error_of(run(b, &[]).0);
        assert_eq!(
            err.to_string(),
            "invalid program: argument 1 does not exist in App::Main @IL_0000"
        );
    }

    #[test]
    fn no_entry_point_is_a_load_error() {
        let b = app();
        assert_eq!(error_of(run(b, &[]).0).exit_code(), 66);
    }

    #[test]
    fn unsupported_entry_signature_is_not_implemented() {
        let mut b = app();
        let main = b.method("Main", &[0x00, 0x01, 0x01, 0x08], Some(&[RET])); // Main(int)
        b.entry_point(main);
        assert_eq!(error_of(run(b, &[]).0).exit_code(), 70);
    }

    #[test]
    fn corlib_without_string_is_a_load_error() {
        let mut corlib = crate::testing::ModelBuilder::new("mscorlib");
        corlib.type_def("System", "Object", Cell::Ref(TableId::TypeDef, 0));
        let mut b = app();
        let s = b.user_string("x");
        let main = b.method(
            "Main",
            VOID_NO_ARGS,
            Some(&il(&[&[LDSTR], &s.to_le_bytes(), &[RET]])),
        );
        b.entry_point(main);
        let err = error_of(run_with(corlib.build(), b, &[]).0);
        assert!(
            err.to_string()
                .contains("corlib does not define System.String"),
            "{err}"
        );
    }
}
