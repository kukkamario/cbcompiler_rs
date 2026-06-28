//! `cb` — CoolBasic compiler driver (interpreter + LLVM-stub binary).
//!
//! Thin shell over [`cb_driver`]'s shared compile pipeline: parse CLI flags,
//! select a backend from the features compiled in (and the `--backend` flag),
//! run the pipeline, and hand the lowered IR to the chosen backend. The
//! frontend pipeline and the exit-code contract live in the `cb_driver` library
//! so a future second binary can reuse them. Selecting the LLVM backend
//! reports "not yet implemented" (exit 3) until codegen lands.

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

    /// Optimization level for the compiled artifact (AOT backends only):
    /// `-O0`/`-O1`/`-O2`/`-O3`, or `-Os`/`-Oz` to optimize for size. Defaults to
    /// `-O2`. Accepted but ignored by the interpreter backend.
    #[arg(short = 'O', value_name = "LEVEL", default_value = "2", value_parser = parse_opt_level)]
    opt: OptLevelArg,

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

/// Optimization level parsed from `-O`. Driver-local so the CLI surface exists
/// in every build; converted to `cb_backend_llvm::OptLevel` only where the llvm
/// backend is constructed ([`to_backend_opt`], gated with that arm).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum OptLevelArg {
    O0,
    O1,
    O2,
    O3,
    Os,
    Oz,
}

/// Parse a `-O` value: `0`/`1`/`2`/`3`/`s`/`z` (the leading `O` is the flag, so a
/// level is required — `-O` alone is an error, not an alias).
fn parse_opt_level(s: &str) -> Result<OptLevelArg, String> {
    match s {
        "0" => Ok(OptLevelArg::O0),
        "1" => Ok(OptLevelArg::O1),
        "2" => Ok(OptLevelArg::O2),
        "3" => Ok(OptLevelArg::O3),
        "s" => Ok(OptLevelArg::Os),
        "z" => Ok(OptLevelArg::Oz),
        other => Err(format!(
            "invalid optimization level '{other}'; expected one of 0, 1, 2, 3, s, z"
        )),
    }
}

/// Map the driver's CLI opt level onto the llvm backend's. Gated with the arm
/// that uses it so non-llvm builds neither reference the backend type nor warn
/// on an unused fn.
#[cfg(feature = "llvm")]
fn to_backend_opt(opt: OptLevelArg) -> cb_backend_llvm::OptLevel {
    use cb_backend_llvm::OptLevel as B;
    match opt {
        OptLevelArg::O0 => B::O0,
        OptLevelArg::O1 => B::O1,
        OptLevelArg::O2 => B::O2,
        OptLevelArg::O3 => B::O3,
        OptLevelArg::Os => B::Os,
        OptLevelArg::Oz => B::Oz,
    }
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

/// Instantiate the selected backend as a `Box<dyn Backend>`. The
/// single feature-gated dispatch point: the run site below calls
/// `backend.execute(...)` with no backend-specific `match`. In a no-backend
/// build `Backend` is uninhabited, so the body is an empty (diverging) match.
///
/// `source`/`output`/`opt` are injected here: the AOT (llvm)
/// backend needs the artifact path and optimization level, which the
/// `Backend::execute` signature does not carry. The interpreter ignores them.
fn make_backend(
    sel: Backend,
    source: &Path,
    output: Option<PathBuf>,
    opt: OptLevelArg,
) -> Box<dyn cb_backend_api::Backend> {
    // Bind all so the interp arm and a no-backend build (uninhabited `Backend`,
    // empty match) don't warn on the unused injected settings; the llvm arm
    // consumes them below.
    let _ = (source, &output, opt);
    match sel {
        #[cfg(feature = "interp")]
        Backend::Interp => Box::new(cb_backend_interp::InterpBackend),
        #[cfg(feature = "llvm")]
        Backend::Llvm => Box::new(cb_backend_llvm::LlvmBackend::new(
            source.to_owned(),
            output,
            to_backend_opt(opt),
        )),
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
        opt,
        file: path,
    } = Cli::parse();

    // Resolve the requested backend up front so an explicitly named but
    // invalid/unavailable backend fails fast — but don't *require* one: a
    // dump-only build (no backend compiled in) must still be able to
    // `--dump-ast` / `--dump-ir`. The "no backend compiled in" error is
    // deferred to the point a program would actually run (below).
    let backend: Option<Box<dyn cb_backend_api::Backend>> = match backend_arg {
        Some(name) => match parse_backend(&name) {
            Ok(b) => Some(make_backend(b, &path, output.clone(), opt)),
            Err(msg) => {
                eprintln!("cb: {msg}");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => default_backend().map(|b| make_backend(b, &path, output.clone(), opt)),
    };

    // Run the shared front-end pipeline. It prints diagnostics and any AST/IR
    // dump itself; we only get IR back when there is something to run.
    let opts = PipelineOptions { dump_ast, dump_ir };
    let (ir_program, interner) = match compile(&path, &opts) {
        Compilation::Ready { program, interner } => (program, interner),
        Compilation::Finished { exit_code } => return ExitCode::from(exit_code),
    };

    // Backend-agnostic dispatch: the backend either ran the program —
    // returning its own exit code, which we clamp to an OS code — or produced an
    // artifact. On `Err`, the `kind` selects the exit code, keeping all OS-exit
    // policy here in the driver: `Unimplemented` (e.g. the llvm stub)
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
