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

pub fn interpret(program: &Program, interner: &Interner) -> Result<(), InterpError> {
    let mut interp = Interpreter::new(program, interner);
    interp.run()
}
