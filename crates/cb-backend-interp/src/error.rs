use std::fmt;

use cb_diagnostics::{Span, Symbol};
use cb_ir::TrapKind;

#[derive(Debug)]
pub struct InterpError {
    pub kind: InterpErrorKind,
    pub stack_trace: Vec<StackEntry>,
}

#[derive(Debug)]
pub enum InterpErrorKind {
    Trap(TrapKind),
    RuntimeError(String),
}

#[derive(Debug)]
pub struct StackEntry {
    pub func_name: Symbol,
    pub span: Span,
}

impl fmt::Display for InterpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            InterpErrorKind::Trap(kind) => {
                let msg = match kind {
                    TrapKind::NullDeref => "null pointer dereference",
                    TrapKind::DeletedAccess => "access to deleted object",
                    TrapKind::DivisionByZero => "division by zero",
                    TrapKind::IndexOutOfBounds => "array index out of bounds",
                    TrapKind::NullFnPtr => "null function pointer call",
                    TrapKind::DoubleDelete => "double delete",
                };
                write!(f, "runtime trap: {msg}")
            }
            InterpErrorKind::RuntimeError(msg) => write!(f, "runtime error: {msg}"),
        }
    }
}

impl std::error::Error for InterpError {}
