//! Backend-agnostic contract shared by every CoolBasic backend.
//!
//! This crate is the seam the project's "pluggable backend" rule hangs on: the
//! frontend lowers to [`cb_ir::Program`], and a backend is anything that can
//! consume that IR and either *run* it or *emit an artifact* from it. The
//! driver selects a backend at runtime and dispatches through [`Backend`]
//! behind a `Box<dyn Backend>`, so its run site stays backend-agnostic.
//!
//! Deliberately holds **no** process/exit-code policy of its own — the driver
//! maps a [`BackendOutcome`] / [`BackendError`] onto an OS exit code.
//! Do not leak backend-specific types (LLVM, interpreter internals) into here.

use std::path::PathBuf;

use cb_diagnostics::Interner;
use cb_ir::Program;

/// A pluggable code-generation/execution backend for lowered CoolBasic IR.
///
/// Object-safe by design so the driver can hold a `Box<dyn Backend>` chosen at
/// runtime from the `--backend` flag.
pub trait Backend {
    /// Stable backend identifier, matching the `--backend <name>` value
    /// (e.g. `"interp"`, `"llvm"`).
    fn name(&self) -> &'static str;

    /// Consume the lowered IR and either run it or emit an artifact.
    ///
    /// `interner` resolves the [`cb_diagnostics::Symbol`]s the IR refers to
    /// (names, string data). Returns a [`BackendOutcome`] on success or a
    /// [`BackendError`] the driver renders and maps to an exit code.
    fn execute(
        &self,
        program: &Program,
        interner: &Interner,
    ) -> Result<BackendOutcome, BackendError>;
}

/// What a backend did with the program.
pub enum BackendOutcome {
    /// The program executed to completion (the interpreter, or a future JIT).
    /// `exit_code` is the program's own code (`End` → 0, `MakeError` → 1,
    /// `request_exit(n)` → n); the driver clamps it to an OS exit code.
    Ran { exit_code: i32 },
    /// The backend produced a build artifact rather than running it (AOT, e.g.
    /// the LLVM backend emitting an object/executable).
    Produced { artifact: PathBuf },
}

/// A backend-side failure. `kind` selects the driver's process exit code and
/// `message` is rendered to stderr.
pub struct BackendError {
    pub kind: BackendErrorKind,
    pub message: String,
}

impl BackendError {
    /// A recognised backend whose codegen does not exist yet (today: `llvm`).
    /// The driver maps this to exit code 3.
    pub fn unimplemented(message: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::Unimplemented,
            message: message.into(),
        }
    }

    /// A genuine trap / internal error while running or compiling. The driver
    /// maps this to exit code 1.
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::Failed,
            message: message.into(),
        }
    }
}

/// Classifies a [`BackendError`] so the driver can pick the right exit code
/// while keeping all OS-exit policy in one place.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BackendErrorKind {
    /// Recognised backend with no codegen yet → driver exit 3.
    Unimplemented,
    /// A genuine trap / internal error while running or compiling → exit 1.
    Failed,
}
