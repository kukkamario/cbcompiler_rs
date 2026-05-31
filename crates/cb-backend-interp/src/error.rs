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
    /// Clean program exit requested by the runtime via the trap channel
    /// (FD-015 `request_exit`). Carried as an error so it propagates up the
    /// `?` chain like a trap, but `run` intercepts it and converts it to
    /// `Ok(code)` — it should never reach the driver's error path.
    Exit(i32),
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
            // Should be intercepted by `run` and never displayed; defensive.
            InterpErrorKind::Exit(code) => write!(f, "exit {code}"),
        }
    }
}

impl std::error::Error for InterpError {}
