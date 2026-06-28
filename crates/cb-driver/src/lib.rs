//! `cb-driver` shared compile pipeline.
//!
//! The front half of the driver, factored out of `main.rs` so multiple thin
//! binaries can share it: read a `.cb` file,
//! tokenize + parse it, render diagnostics, optionally dump the AST/IR, run
//! semantic analysis, and lower to backend-agnostic IR. The pipeline is
//! deliberately **backend-free** — it returns the lowered [`cb_ir::Program`]
//! for a caller to hand to whatever backend it selected, or a finished exit
//! code when there is nothing to run (dumps, errors, or a usage failure).
//!
//! Process exit codes are centralised in [`exit`] / [`clamp_exit`] so the
//! contract is documented once and shared by every binary built on this crate.

use std::path::Path;

use cb_diagnostics::{CliRenderer, Diagnostic, Interner, Renderer, Severity, SourceMap};
use cb_frontend::ast_print;
use cb_ir::Program;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};

mod include;

/// Process exit codes the driver returns. Centralised so the contract is
/// explicit and testable:
///
/// * `0` — success.
/// * `1` — compilation produced error diagnostics, or the program itself
///   failed at runtime (`MakeError`, an interpreter trap, a runtime
///   `raise_error`).
/// * `2` — driver/usage error: bad CLI arguments (clap also exits `2` for
///   these), an unreadable input file, a runtime-catalog load failure, or an
///   unknown / not-compiled-in `--backend`.
/// * `3` — the requested backend is recognised but not yet implemented
///   (today: `llvm`). Mapped from `cb_backend_api::BackendErrorKind::Unimplemented`
///   in the binary's backend-dispatch step.
pub mod exit {
    /// Driver or usage error. Matches clap's own exit code for argument errors.
    pub const USAGE: u8 = 2;
    /// Backend selected is recognised but has no codegen yet (e.g. `llvm`).
    pub const BACKEND_UNIMPLEMENTED: u8 = 3;
}

/// Map a program-level exit code (`i32`) onto an OS process exit code.
///
/// OS process exit codes occupy `0..=255`. CoolBasic's `End` / `request_exit`
/// can name any `i32`, so we clamp into range rather than wrapping: the old
/// `as u8` cast turned `256` into `0`, silently converting a failure into a
/// success. Values above `255` saturate to `255` (still non-zero, still a
/// failure); negative values clamp to `0`.
///
/// Used by a binary's backend-dispatch step for a backend that *runs* the
/// program (`cb_backend_api::BackendOutcome::Ran`); today that is the
/// interpreter.
pub fn clamp_exit(code: i32) -> u8 {
    code.clamp(0, 255) as u8
}

/// Which dump the caller requested instead of running the program. Both unset
/// means "compile and return the IR for a backend to run".
pub struct PipelineOptions {
    /// Print the parsed AST to stdout (skips execution).
    pub dump_ast: bool,
    /// Print the lowered IR to stdout (skips execution).
    pub dump_ir: bool,
}

/// Outcome of [`compile`].
pub enum Compilation {
    /// The program type-checked and lowered cleanly and no dump flag was set:
    /// the caller should run its selected backend on this IR.
    Ready {
        program: Program,
        interner: Interner,
    },
    /// Nothing to run — a dump-only invocation, an erroring program, or a usage
    /// failure (file read / catalog load / diagnostic render). `exit_code` is
    /// the OS exit code the process should return.
    Finished { exit_code: u8 },
}

/// Run the front half of the compiler on a single source file: read, tokenize,
/// parse, dump-AST, load the runtime catalog, analyze, and lower to IR —
/// emitting every diagnostic and any requested dump to std streams along the
/// way.
///
/// Returns [`Compilation::Ready`] with the lowered IR when the program is
/// error-free and no dump flag was set; otherwise [`Compilation::Finished`]
/// with the exit code to return (dumps, errors, or a usage failure).
pub fn compile(path: &Path, opts: &PipelineOptions) -> Compilation {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cb: failed to read {}: {}", path.display(), e);
            return Compilation::Finished {
                exit_code: exit::USAGE,
            };
        }
    };

    // Resolve `Include`s: parse the main file and every (transitively) included
    // file into one shared arena + source map, splicing each top-level
    // `Include` into the merged program (cb_syntax.md §2.2). `front_diags`
    // carries the combined lex, parse, and include-resolution diagnostics.
    let include::Resolved {
        arena,
        program,
        sources,
        diagnostics: front_diags,
    } = include::resolve(path, text);

    let mut stderr = CliRenderer::new(StandardStream::stderr(ColorChoice::Auto));
    let mut had_error = false;

    // The AST dump needs only the parsed arena — never the runtime catalog or
    // semantic analysis — so emit it up front. This lets a dump-only build
    // (`--no-default-features`, no runtime linked) inspect the AST even when
    // the runtime catalog cannot be loaded.
    if opts.dump_ast {
        println!("Program ({} top-level statements):", program.len());
        let mut buf = String::new();
        for &id in &program {
            buf.clear();
            ast_print::debug_print(&mut buf, &arena, id).expect("writing to String never fails");
            print!("{buf}");
        }
    }

    // Lex and parse diagnostics never depend on the runtime catalog; emit them
    // before attempting the catalog load so they survive a catalog failure on
    // the dump-only path below.
    if let Err(code) = emit_diagnostics(&mut stderr, &sources, &mut had_error, front_diags.iter()) {
        return Compilation::Finished { exit_code: code };
    }

    // Semantic analysis needs the runtime function catalog. The catalog is
    // required to lower/run the program or to dump IR, but a pure `--dump-ast`
    // does not need it — so a catalog-load failure is only fatal when the
    // lowered IR is actually required. The AST is already printed.
    let runtime_catalog = match cb_runtime_sys::load_catalog() {
        Ok(c) => c,
        Err(msg) => {
            if opts.dump_ast && !opts.dump_ir {
                return Compilation::Finished {
                    exit_code: if had_error { 1 } else { 0 },
                };
            }
            eprintln!("cb: failed to load runtime catalog: {msg}");
            return Compilation::Finished {
                exit_code: exit::USAGE,
            };
        }
    };

    // Run semantic analysis.
    let mut sema_result = cb_sema::analyze(&arena, &program, &sources, &runtime_catalog);
    if let Err(code) = emit_diagnostics(
        &mut stderr,
        &sources,
        &mut had_error,
        sema_result.diagnostics.iter(),
    ) {
        return Compilation::Finished { exit_code: code };
    }

    // Lower to IR (only if no errors).
    if !had_error {
        let ir_program = cb_sema::lower::lower(&arena, &program, &sources, &mut sema_result);

        #[cfg(debug_assertions)]
        cb_ir::verify::verify(&ir_program);

        if opts.dump_ir {
            let output = cb_ir::print::print_program(&ir_program, &sema_result.interner);
            print!("{output}");
        }

        if !opts.dump_ast && !opts.dump_ir {
            return Compilation::Ready {
                program: ir_program,
                interner: sema_result.interner,
            };
        }
    }

    Compilation::Finished {
        exit_code: if had_error { 1 } else { 0 },
    }
}

/// Emit a batch of diagnostics to `stderr`, OR-ing whether any was an error
/// into `had_error`. Returns `Err(exit::USAGE)` on a renderer I/O failure for
/// the caller to propagate. Shared by the catalog-independent (lex/parse) and
/// catalog-dependent (sema) batches even though they are reported at different
/// points.
fn emit_diagnostics<'a>(
    stderr: &mut CliRenderer<StandardStream>,
    sources: &SourceMap,
    had_error: &mut bool,
    diags: impl IntoIterator<Item = &'a Diagnostic>,
) -> Result<(), u8> {
    for d in diags {
        if matches!(d.severity, Severity::Error) {
            *had_error = true;
        }
        if let Err(e) = stderr.emit(d, sources) {
            eprintln!("cb: failed to render diagnostic: {e}");
            return Err(exit::USAGE);
        }
    }
    Ok(())
}
