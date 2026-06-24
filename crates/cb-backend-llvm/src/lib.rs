//! LLVM backend for CoolBasic IR.
//!
//! Will use `inkwell` for safe LLVM bindings once codegen begins. Until then
//! the backend exists only as a [`Backend`] impl that reports the gap
//! explicitly, so the driver dispatches through the shared trait (FD-044)
//! ahead of codegen rather than carrying a special-cased `match` arm.

use cb_backend_api::{Backend, BackendError, BackendOutcome};
use cb_diagnostics::Interner;
use cb_ir::Program;

/// The LLVM / AOT backend. Codegen is not implemented yet, so
/// [`Backend::execute`] returns a `BackendError::unimplemented` (driver exit
/// code 3) rather than silently doing nothing (FD-025). Once codegen lands it
/// will emit an artifact and return [`BackendOutcome::Produced`].
pub struct LlvmBackend;

impl Backend for LlvmBackend {
    fn name(&self) -> &'static str {
        "llvm"
    }

    fn execute(
        &self,
        _program: &Program,
        _interner: &Interner,
    ) -> Result<BackendOutcome, BackendError> {
        Err(BackendError::unimplemented(
            "the llvm backend is not yet implemented; \
             run with --backend interp to execute programs",
        ))
    }
}
