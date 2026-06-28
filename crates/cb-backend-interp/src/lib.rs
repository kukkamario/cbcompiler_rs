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

use cb_backend_api::{Backend, BackendError, BackendOutcome};
use cb_diagnostics::Interner;
use cb_ir::Program;

/// Interpret the program, returning the process exit code on success
/// (0 for normal completion / `End`, the `MakeError` code for an abort).
/// `Err` is a genuine interpreter trap or internal error.
pub fn interpret(program: &Program, interner: &Interner) -> Result<i32, InterpError> {
    let mut interp = Interpreter::new(program, interner);
    interp.run()
}

/// The interpreter exposed as a [`Backend`]. Runs the program
/// in-process via [`interpret`] and reports the program's own exit code; an
/// interpreter trap becomes a [`BackendError`] the driver maps to exit 1.
///
/// The observer/debuggability machinery ([`Observer`], [`Interpreter::with_observer`])
/// stays interpreter-specific and is intentionally not part of the cross-backend
/// trait — use [`interpret`] / [`Interpreter`] directly when you need it.
pub struct InterpBackend;

impl Backend for InterpBackend {
    fn name(&self) -> &'static str {
        "interp"
    }

    fn execute(
        &self,
        program: &Program,
        interner: &Interner,
    ) -> Result<BackendOutcome, BackendError> {
        match interpret(program, interner) {
            Ok(exit_code) => Ok(BackendOutcome::Ran { exit_code }),
            Err(e) => Err(BackendError::failed(e.to_string())),
        }
    }
}
