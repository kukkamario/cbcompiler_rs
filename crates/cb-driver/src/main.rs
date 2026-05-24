//! `cb` — CoolBasic compiler driver.
//!
//! End-to-end smoke driver: tokenize + parse a single `.cb` file, render any
//! diagnostics to stderr, print a debug view of the AST to stdout, and exit
//! non-zero if any error-severity diagnostics were emitted. Codegen and
//! backend selection arrive later — see FD-002 plan §E.

use std::path::PathBuf;
use std::process::ExitCode;

use cb_diagnostics::{CliRenderer, Renderer, Severity, SourceMap};
use cb_frontend::ast_print;
use cb_frontend::parser::ParseResult;
use cb_frontend::{LexerOptions, parse, tokenize};
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};

#[cfg(feature = "interp")]
const HAS_INTERP: bool = true;
#[cfg(not(feature = "interp"))]
const HAS_INTERP: bool = false;
#[cfg(feature = "llvm")]
const HAS_LLVM: bool = true;
#[cfg(not(feature = "llvm"))]
const HAS_LLVM: bool = false;

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

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut backend_arg: Option<String> = None;
    let mut positional: Option<String> = None;
    let mut dump_ir = false;
    let mut dump_ast = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--backend" => match args.next() {
                Some(v) => backend_arg = Some(v),
                None => {
                    eprintln!("cb: --backend requires a value");
                    return ExitCode::from(2);
                }
            },
            "--dump-ir" => dump_ir = true,
            "--dump-ast" => dump_ast = true,
            _ if a.starts_with("--backend=") => {
                backend_arg = Some(a["--backend=".len()..].to_string());
            }
            _ if positional.is_none() => positional = Some(a),
            _ => {
                eprintln!("cb: unexpected argument: {a}");
                return ExitCode::from(2);
            }
        }
    }

    let Some(path_arg) = positional else {
        eprintln!("usage: cb [--backend <name>] [--dump-ast] [--dump-ir] <file.cb>");
        return ExitCode::from(2);
    };

    let _backend = match backend_arg {
        Some(name) => match parse_backend(&name) {
            Ok(b) => b,
            Err(msg) => {
                eprintln!("cb: {msg}");
                return ExitCode::from(2);
            }
        },
        None => match default_backend() {
            Some(b) => b,
            None => {
                eprintln!(
                    "cb: no backend compiled in; rebuild with --features interp or --features llvm"
                );
                return ExitCode::from(2);
            }
        },
    };

    let path = PathBuf::from(&path_arg);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cb: failed to read {}: {}", path.display(), e);
            return ExitCode::from(2);
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

    // Load runtime function catalog from the C runtime library.
    let runtime_catalog = match cb_runtime_sys::load_catalog() {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("cb: failed to load runtime catalog: {msg}");
            return ExitCode::from(2);
        }
    };

    // Run semantic analysis.
    let mut sema_result = cb_sema::analyze(&arena, &program, &text, file, &runtime_catalog);

    let mut stderr = CliRenderer::new(StandardStream::stderr(ColorChoice::Auto));
    let mut had_error = false;
    let all_diags = lex_diags
        .iter()
        .chain(parse_diags.iter())
        .chain(sema_result.diagnostics.iter());
    for d in all_diags {
        if matches!(d.severity, Severity::Error) {
            had_error = true;
        }
        if let Err(e) = stderr.emit(d, &sources) {
            eprintln!("cb: failed to render diagnostic: {e}");
            return ExitCode::from(2);
        }
    }

    if dump_ast {
        println!("Program ({} top-level statements):", program.len());
        let mut buf = String::new();
        for &id in &program {
            buf.clear();
            ast_print::debug_print(&mut buf, &arena, id).expect("writing to String never fails");
            print!("{buf}");
        }
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
            #[cfg(feature = "interp")]
            if matches!(_backend, Backend::Interp)
                && let Err(e) = cb_backend_interp::interpret(&ir_program, &sema_result.interner)
            {
                eprintln!("cb: {e}");
                return ExitCode::from(1);
            }
        }
    }

    if had_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
