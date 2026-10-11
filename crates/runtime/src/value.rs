//! Evaluation-stack values (§III.1.1 stack types).

use crate::heap::ObjRef;

/// A typed evaluation-stack slot. A managed-pointer variant is added with the
/// first opcode that produces one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StackValue {
    I32(i32),
    I64(i64),
    F64(f64),
    NativeInt(i64),
    /// Object reference; `None` is null.
    Obj(Option<ObjRef>),
}
