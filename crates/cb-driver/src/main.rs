//! `cb` — CoolBasic compiler driver.
//!
//! End-to-end smoke driver: tokenize + parse a single `.cb` file, run semantic
//! analysis, render any diagnostics to stderr, optionally dump the AST/IR, and
//! otherwise hand the lowered IR to the selected backend. Exit codes are
//! documented on [`exit`]. Codegen for the LLVM backend arrives later — for now
//! selecting it reports "not yet implemented" rather than silently doing
//! nothing (FD-025).

use std::path::PathBuf;
use std::process::ExitCode;

use cb_diagnostics::{CliRenderer, Diagnostic, Renderer, Severity, SourceMap};
use cb_frontend::ast_print;
use cb_frontend::parser::ParseResult;
use cb_frontend::{LexerOptions, parse, tokenize};
use clap::Parser;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};

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
/// * `3` — the requested backend is recognised but not yet implemented.
///   Only reachable in builds with the `llvm` feature; the constant is
///   `#[cfg(feature = "llvm")]`, so this code does not exist in an
///   interp-only build.
mod exit {
    /// Driver or usage error. Matches clap's own exit code for argument errors.
    pub const USAGE: u8 = 2;
    /// Backend selected is recognised but has no codegen yet (e.g. `llvm`).
    /// Only referenced when an unimplemented backend is compiled in.
    #[cfg(feature = "llvm")]
    pub const BACKEND_UNIMPLEMENTED: u8 = 3;
}

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

/// Map a program-level exit code (`i32`) onto an OS process exit code.
///
/// OS process exit codes occupy `0..=255`. CoolBasic's `End` / `request_exit`
/// can name any `i32`, so we clamp into range rather than wrapping: the old
/// `as u8` cast turned `256` into `0`, silently converting a failure into a
/// success. Values above `255` saturate to `255` (still non-zero, still a
/// failure); negative values clamp to `0`.
///
/// Only the interpreter backend produces a program-level exit code today.
#[cfg(feature = "interp")]
fn clamp_exit(code: i32) -> u8 {
    code.clamp(0, 255) as u8
}

/// Emit a batch of diagnostics to `stderr`, OR-ing whether any was an error
/// into `had_error`. A renderer I/O failure is fatal: it is reported and
/// returned as `Err(USAGE)` for the caller to propagate. Factored out so the
/// catalog-independent (lex/parse) and catalog-dependent (sema) batches share
/// one emit path even though they are now reported at different points.
fn emit_diagnostics<'a>(
    stderr: &mut CliRenderer<StandardStream>,
    sources: &SourceMap,
    had_error: &mut bool,
    diags: impl IntoIterator<Item = &'a Diagnostic>,
) -> Result<(), ExitCode> {
    for d in diags {
        if matches!(d.severity, Severity::Error) {
            *had_error = true;
        }
        if let Err(e) = stderr.emit(d, sources) {
            eprintln!("cb: failed to render diagnostic: {e}");
            return Err(ExitCode::from(exit::USAGE));
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let Cli {
        backend: backend_arg,
        dump_ast,
        dump_ir,
        file: path,
    } = Cli::parse();

    // Resolve the requested backend, but don't *require* one yet: a dump-only
    // build (no backend compiled in) must still be able to `--dump-ast` /
    // `--dump-ir`. The "no backend compiled in" error is deferred to the point
    // where we would actually run a program (see the run dispatch below). An
    // explicitly named but invalid/unavailable backend still fails fast.
    let backend: Option<Backend> = match backend_arg {
        Some(name) => match parse_backend(&name) {
            Ok(b) => Some(b),
            Err(msg) => {
                eprintln!("cb: {msg}");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => default_backend(),
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cb: failed to read {}: {}", path.display(), e);
            return ExitCode::from(exit::USAGE);
        }
    };

    let mut sources = SourceMap::new();
    let file = sources.add(path.display().to_string(), text.clone());

    let (tokens, lex_diags) = tokenize(&text, file, LexerOptions::default());
    let ParseResult {
        arena,
        program,
        diagnostics: parse_diags,
    } = parse(&tokens, &text, file);

    let mut stderr = CliRenderer::new(StandardStream::stderr(ColorChoice::Auto));
    let mut had_error = false;

    // The AST dump needs only the parsed arena — never the runtime catalog or
    // semantic analysis — so emit it up front. This lets a dump-only build
    // (`--no-default-features`, no runtime linked) inspect the AST even when
    // the runtime catalog cannot be loaded (DR-R1).
    if dump_ast {
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
    if let Err(code) = emit_diagnostics(
        &mut stderr,
        &sources,
        &mut had_error,
        lex_diags.iter().chain(parse_diags.iter()),
    ) {
        return code;
    }

    // Semantic analysis needs the runtime function catalog. The catalog is
    // required to lower/run the program or to dump IR, but a pure `--dump-ast`
    // does not need it — so a catalog-load failure is only fatal when the
    // lowered IR is actually required (DR-R1). The AST is already printed.
    let runtime_catalog = match cb_runtime_sys::load_catalog() {
        Ok(c) => c,
        Err(msg) => {
            if dump_ast && !dump_ir {
                return if had_error {
                    ExitCode::from(1)
                } else {
                    ExitCode::SUCCESS
                };
            }
            eprintln!("cb: failed to load runtime catalog: {msg}");
            return ExitCode::from(exit::USAGE);
        }
    };

    // Run semantic analysis.
    let mut sema_result = cb_sema::analyze(&arena, &program, &text, file, &runtime_catalog);
    if let Err(code) = emit_diagnostics(
        &mut stderr,
        &sources,
        &mut had_error,
        sema_result.diagnostics.iter(),
    ) {
        return code;
    }

    // Lower to IR (only if no errors).
    if !had_error {
        let ir_program = cb_sema::lower::lower(&arena, &program, &text, &mut sema_result);

        #[cfg(debug_assertions)]
        cb_ir::verify::verify(&ir_program);

        if dump_ir {
            let output = cb_ir::print::print_program(&ir_program, &sema_result.interner);
            print!("{output}");
        }

        if !dump_ast && !dump_ir {
            match backend {
                #[cfg(feature = "interp")]
                Some(Backend::Interp) => {
                    // `Ok(code)` is the program's own exit code (`End` → 0,
                    // `MakeError` → 1, `request_exit(n)` → n); `Err` is an
                    // interpreter trap / internal error, which always maps to
                    // exit 1 with a diagnostic.
                    match cb_backend_interp::interpret(&ir_program, &sema_result.interner) {
                        Ok(code) => return ExitCode::from(clamp_exit(code)),
                        Err(e) => {
                            eprintln!("cb: {e}");
                            return ExitCode::from(1);
                        }
                    }
                }
                #[cfg(feature = "llvm")]
                Some(Backend::Llvm) => {
                    // The `cb-backend-llvm` dependency is intentionally pre-wired
                    // (Cargo.toml `dep:cb-backend-llvm`) ahead of codegen: the
                    // `llvm` feature flips `HAS_LLVM` and adds this enum variant,
                    // but nothing here calls into the crate yet. Until codegen
                    // lands we report the gap explicitly (FD-025) rather than
                    // silently doing nothing.
                    eprintln!(
                        "cb: the llvm backend is not yet implemented; \
                         run with --backend interp to execute programs"
                    );
                    return ExitCode::from(exit::BACKEND_UNIMPLEMENTED);
                }
                None => {
                    eprintln!(
                        "cb: no backend compiled in; rebuild with --features interp or --features llvm"
                    );
                    return ExitCode::from(exit::USAGE);
                }
            }
        }
    }

    if had_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
