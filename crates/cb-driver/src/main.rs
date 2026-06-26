//! `cb` — CoolBasic compiler driver (interpreter + LLVM-stub binary).
//!
//! Thin shell over [`cb_driver`]'s shared compile pipeline: parse CLI flags,
//! select a backend from the features compiled in (and the `--backend` flag),
//! run the pipeline, and hand the lowered IR to the chosen backend. The
//! frontend pipeline and the exit-code contract live in the `cb_driver` library
//! so a future second binary can reuse them (FD-044). Selecting the LLVM backend
//! reports "not yet implemented" (exit 3) until codegen lands (FD-025).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cb_backend_api::{BackendErrorKind, BackendOutcome};
use cb_driver::{Compilation, PipelineOptions, clamp_exit, compile, exit};
use clap::Parser;

#[cfg(feature = "interp")]
const HAS_INTERP: bool = true;
#[cfg(not(feature = "interp"))]
const HAS_INTERP: bool = false;
#[cfg(feature = "llvm")]
const HAS_LLVM: bool = true;
#[cfg(not(feature = "llvm"))]
const HAS_LLVM: bool = false;

/// Compile and run a single CoolBasic source file.
#[derive(Parser, Debug)]
#[command(name = "cb", version, about, long_about = None)]
struct Cli {
    /// Backend used to run the program: `interp` or `llvm`. Defaults to
    /// `interp`; availability depends on the features compiled in.
    #[arg(long, value_name = "NAME")]
    backend: Option<String>,

    /// Print the parsed AST to stdout (still reports diagnostics; skips
    /// execution).
    #[arg(long)]
    dump_ast: bool,

    /// Print the lowered IR to stdout (only when the program is error-free;
    /// skips execution).
    #[arg(long)]
    dump_ir: bool,

    /// Output path for the compiled artifact (AOT backends only). Defaults to
    /// the source file's stem next to it, plus the platform exe suffix. Accepted
    /// but ignored by the interpreter backend.
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    output: Option<PathBuf>,

    /// CoolBasic source file to compile.
    #[arg(value_name = "FILE")]
    file: PathBuf,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum Backend {
    #[cfg(feature = "interp")]
    Interp,
    #[cfg(feature = "llvm")]
    Llvm,
}

fn available_backends() -> &'static str {
    match (HAS_INTERP, HAS_LLVM) {
        (true, true) => "interp, llvm",
        (true, false) => "interp",
        (false, true) => "llvm",
        (false, false) => "(none)",
    }
}

fn default_backend() -> Option<Backend> {
    #[cfg(feature = "interp")]
    {
        Some(Backend::Interp)
    }
    #[cfg(all(not(feature = "interp"), feature = "llvm"))]
    {
        Some(Backend::Llvm)
    }
    #[cfg(not(any(feature = "interp", feature = "llvm")))]
    {
        None
    }
}

/// Instantiate the selected backend as a `Box<dyn Backend>` (FD-044). The
/// single feature-gated dispatch point: the run site below calls
/// `backend.execute(...)` with no backend-specific `match`. In a no-backend
/// build `Backend` is uninhabited, so the body is an empty (diverging) match.
///
/// `source`/`output` are injected here (FD-048 decision 3): the AOT (llvm)
/// backend needs the artifact path, which the `Backend::execute` signature does
/// not carry. The interpreter ignores them.
fn make_backend(
    sel: Backend,
    source: &Path,
    output: Option<PathBuf>,
) -> Box<dyn cb_backend_api::Backend> {
    // Bind both so the interp arm and a no-backend build (uninhabited `Backend`,
    // empty match) don't warn on the unused injected paths; the llvm arm
    // consumes them below.
    let _ = (source, &output);
    match sel {
        #[cfg(feature = "interp")]
        Backend::Interp => Box::new(cb_backend_interp::InterpBackend),
        #[cfg(feature = "llvm")]
        Backend::Llvm => Box::new(cb_backend_llvm::LlvmBackend::new(source.to_owned(), output)),
    }
}

fn parse_backend(name: &str) -> Result<Backend, String> {
    match name {
        #[cfg(feature = "interp")]
        "interp" => Ok(Backend::Interp),
        #[cfg(feature = "llvm")]
        "llvm" => Ok(Backend::Llvm),
        #[cfg(not(feature = "interp"))]
        "interp" => Err(format!(
            "backend 'interp' not compiled in (rebuild with --features interp); \
             available backends in this build: {}",
            available_backends()
        )),
        #[cfg(not(feature = "llvm"))]
        "llvm" => Err(format!(
            "backend 'llvm' not compiled in (rebuild with --features llvm); \
             available backends in this build: {}",
            available_backends()
        )),
        other => Err(format!(
            "unknown backend '{other}'; available backends in this build: {}",
            available_backends()
        )),
    }
}

fn main() -> ExitCode {
    let Cli {
        backend: backend_arg,
        dump_ast,
        dump_ir,
        output,
        file: path,
    } = Cli::parse();

    // Resolve the requested backend up front so an explicitly named but
    // invalid/unavailable backend fails fast — but don't *require* one: a
    // dump-only build (no backend compiled in) must still be able to
    // `--dump-ast` / `--dump-ir`. The "no backend compiled in" error is
    // deferred to the point a program would actually run (below).
    let backend: Option<Box<dyn cb_backend_api::Backend>> = match backend_arg {
        Some(name) => match parse_backend(&name) {
            Ok(b) => Some(make_backend(b, &path, output.clone())),
            Err(msg) => {
                eprintln!("cb: {msg}");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => default_backend().map(|b| make_backend(b, &path, output.clone())),
    };

    // Run the shared front-end pipeline. It prints diagnostics and any AST/IR
    // dump itself; we only get IR back when there is something to run.
    let opts = PipelineOptions { dump_ast, dump_ir };
    let (ir_program, interner) = match compile(&path, &opts) {
        Compilation::Ready { program, interner } => (program, interner),
        Compilation::Finished { exit_code } => return ExitCode::from(exit_code),
    };

    // Backend-agnostic dispatch (FD-044): the backend either ran the program —
    // returning its own exit code, which we clamp to an OS code — or produced an
    // artifact. On `Err`, the `kind` selects the exit code, keeping all OS-exit
    // policy here in the driver (FD-025): `Unimplemented` (e.g. the llvm stub)
    // → 3, any other failure (an interpreter trap / internal error) → 1.
    match backend {
        Some(backend) => match backend.execute(&ir_program, &interner) {
            Ok(BackendOutcome::Ran { exit_code }) => ExitCode::from(clamp_exit(exit_code)),
            Ok(BackendOutcome::Produced { artifact }) => {
                println!("cb: wrote {}", artifact.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("cb: {}", e.message);
                ExitCode::from(match e.kind {
                    BackendErrorKind::Unimplemented => exit::BACKEND_UNIMPLEMENTED,
                    BackendErrorKind::Failed => 1,
                })
            }
        },
        None => {
            eprintln!(
                "cb: no backend compiled in; rebuild with --features interp or --features llvm"
            );
            ExitCode::from(exit::USAGE)
        }
    }
}
