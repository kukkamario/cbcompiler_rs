//! Interpreter backend for CoolBasic IR.
//!
//! Prioritizes debuggability over speed. Acts as the reference implementation
//! when the LLVM backend disagrees.

pub mod error;
pub mod ffi;
pub mod heap;
pub mod interp;
pub mod observer;
pub mod string_handle;
pub mod value;

pub use error::InterpError;
pub use interp::Interpreter;
pub use observer::{NoopObserver, Observer};

use cb_diagnostics::Interner;
use cb_ir::Program;

/// Interpret the program, returning the process exit code on success
/// (0 for normal completion / `End`, the `MakeError` code for an abort).
/// `Err` is a genuine interpreter trap or internal error.
pub fn interpret(program: &Program, interner: &Interner) -> Result<i32, InterpError> {
    let mut interp = Interpreter::new(program, interner);
    interp.run()
}
