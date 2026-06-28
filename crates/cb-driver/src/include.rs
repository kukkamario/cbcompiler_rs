//! `Include` resolution.
//!
//! `Include "file"` (cb_syntax.md §2.2) is a **top-level, textual-paste**
//! construct: each top-level `Include` is replaced in place by the
//! (recursively expanded) top-level statements of the named file. All files
//! are parsed into one shared [`Arena`] so their `NodeId`s share an id space,
//! and each is registered in a [`SourceMap`] under its own [`FileId`] so spans
//! — and therefore diagnostics — resolve to the right file.
//!
//! Semantics enforced here:
//! - **Relative paths** resolve against the *including* file's directory;
//!   absolute paths are taken as-is.
//! - **At most once:** each file is included only on first encounter; repeats
//!   and cycles are silently dropped (a re-include is a no-op, §2.2).
//! - **Unreadable include:** reported as `E0334` at the path literal.
//!
//! A *nested* `Include` (inside a function or block) is intentionally **not**
//! expanded here — only the top-level statement list of each file is scanned.
//! Such a node survives into sema, which reports it as misplaced (`E0333`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cb_diagnostics::{Diagnostic, DiagnosticCode, FileId, Label, SourceMap, Span};
use cb_frontend::ast::{Expr, Node, Stmt};
use cb_frontend::{Arena, LexerOptions, NodeId, parse_into, tokenize};

/// An included file could not be read (missing, unreadable, or a path that does
/// not resolve). Continues the sema `E03xx` sequence; emitted here because
/// include resolution runs in the driver, ahead of sema.
const E_INCLUDE_READ_FAILED: DiagnosticCode = DiagnosticCode::new("E0334");

/// The merged result of resolving every `Include` reachable from the main file.
pub struct Resolved {
    /// One arena holding every file's nodes.
    pub arena: Arena,
    /// The merged top-level program (includes expanded in place).
    pub program: Vec<NodeId>,
    /// Every file that was read, keyed by `FileId`.
    pub sources: SourceMap,
    /// Lex, parse, and include-resolution diagnostics across all files.
    pub diagnostics: Vec<Diagnostic>,
}

/// Resolve all top-level `Include`s starting from the main file. `main_text` is
/// the already-read contents of `main_path`.
pub fn resolve(main_path: &Path, main_text: String) -> Resolved {
    let mut arena = Arena::new();
    let mut sources = SourceMap::new();
    let mut diagnostics = Vec::new();
    let mut visited = HashSet::new();

    visited.insert(canonical_key(main_path));
    let file_id = sources.add(main_path.display().to_string(), main_text.clone());
    let program = expand(
        &mut arena,
        &mut sources,
        &mut diagnostics,
        &mut visited,
        main_path,
        file_id,
        &main_text,
    );

    Resolved {
        arena,
        program,
        sources,
        diagnostics,
    }
}

/// Canonicalize `path` for the at-most-once dedup key. Falls back to the path
/// as-given when canonicalization fails (e.g. the file does not exist — the
/// subsequent read then surfaces the failure as a diagnostic).
fn canonical_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Tokenize + parse one file into the shared arena, then walk its top-level
/// statements, splicing each `Include`'s expanded contents in place. Returns
/// the file's contribution to the merged program.
fn expand(
    arena: &mut Arena,
    sources: &mut SourceMap,
    diagnostics: &mut Vec<Diagnostic>,
    visited: &mut HashSet<PathBuf>,
    file_path: &Path,
    file_id: FileId,
    text: &str,
) -> Vec<NodeId> {
    let (tokens, lex_diags) = tokenize(text, file_id, LexerOptions::default());
    diagnostics.extend(lex_diags);
    let (program, parse_diags) = parse_into(arena, &tokens, text, file_id);
    diagnostics.extend(parse_diags);

    let mut merged = Vec::with_capacity(program.len());
    for stmt_id in program {
        // Pull the include's path + span out as owned data so the immutable
        // arena borrow ends before the recursive call reborrows it mutably.
        let include = match &arena[stmt_id] {
            Node::Stmt(Stmt::Include { path }) => match &arena[*path] {
                Node::Expr(Expr::StrLit { value, .. }) => {
                    Some((value.clone(), arena.span_of(*path)))
                }
                // The parser guarantees a string-literal path; if it isn't one,
                // a parse error was already emitted — keep the node and move on.
                _ => None,
            },
            _ => None,
        };

        let Some((raw_path, span)) = include else {
            merged.push(stmt_id);
            continue;
        };

        let target = resolve_path(file_path, &raw_path);
        if !visited.insert(canonical_key(&target)) {
            continue; // already included (repeat or cycle) — silently skip (§2.2)
        }

        match std::fs::read_to_string(&target) {
            Ok(child_text) => {
                let child_id = sources.add(target.display().to_string(), child_text.clone());
                let child_program = expand(
                    arena,
                    sources,
                    diagnostics,
                    visited,
                    &target,
                    child_id,
                    &child_text,
                );
                merged.extend(child_program);
            }
            Err(e) => diagnostics.push(read_failed_diag(&target, span, &e)),
        }
    }
    merged
}

/// Resolve an include path string against the including file's directory.
/// Absolute paths are returned unchanged.
fn resolve_path(including_file: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        including_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(p)
    }
}

fn read_failed_diag(target: &Path, span: Span, err: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        E_INCLUDE_READ_FAILED,
        format!("cannot read included file `{}`: {err}", target.display()),
        Label::with_message(span, "included here"),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::{TempDir, tempdir};

    use super::*;

    fn codes(r: &Resolved) -> Vec<&str> {
        r.diagnostics
            .iter()
            .filter_map(|d| d.code.as_ref().map(|c| c.as_str()))
            .collect()
    }

    /// Write `body` to `name` under `dir` and return its path.
    fn write(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let p = dir.path().join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, body).unwrap();
        p
    }

    /// Resolve `main`, returning the merged result. `main` must already exist.
    fn resolve_file(main: &Path) -> Resolved {
        let text = fs::read_to_string(main).unwrap();
        resolve(main, text)
    }

    #[test]
    fn merges_included_top_level_statements() {
        let dir = tempdir().unwrap();
        write(
            &dir,
            "lib.cb",
            "Function f() As Integer\nReturn 1\nEndFunction\n",
        );
        let main = write(&dir, "main.cb", "Include \"lib.cb\"\nPrint f()\n");

        let r = resolve_file(&main);

        assert!(codes(&r).is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.sources.len(), 2, "main + lib registered");
        // The top-level `Include` is replaced by lib's one statement, leaving
        // [Function f, Print f()].
        assert_eq!(r.program.len(), 2);
    }

    #[test]
    fn nested_include_resolves_relative_to_its_own_file() {
        let dir = tempdir().unwrap();
        write(
            &dir,
            "sub/b.cb",
            "Function fromB() As Integer\nReturn 42\nEndFunction\n",
        );
        // a.cb includes "b.cb" — must resolve against sub/, not the main dir.
        write(
            &dir,
            "sub/a.cb",
            "Include \"b.cb\"\nFunction fromA() As Integer\nReturn fromB()\nEndFunction\n",
        );
        let main = write(&dir, "main.cb", "Include \"sub/a.cb\"\nPrint fromA()\n");

        let r = resolve_file(&main);

        assert!(codes(&r).is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.sources.len(), 3, "main + a + b");
        assert_eq!(r.program.len(), 3, "fromB, fromA, Print");
    }

    #[test]
    fn repeat_include_is_included_only_once() {
        let dir = tempdir().unwrap();
        write(
            &dir,
            "lib.cb",
            "Function f() As Integer\nReturn 1\nEndFunction\n",
        );
        let main = write(
            &dir,
            "main.cb",
            "Include \"lib.cb\"\nInclude \"lib.cb\"\nPrint f()\n",
        );

        let r = resolve_file(&main);

        assert!(codes(&r).is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.sources.len(), 2, "lib added once");
        assert_eq!(r.program.len(), 2, "lib spliced once, then Print");
    }

    #[test]
    fn cyclic_include_terminates_and_includes_each_once() {
        let dir = tempdir().unwrap();
        // cyc1 includes cyc2 includes cyc1 — the back-edge to cyc1 is dropped.
        write(
            &dir,
            "cyc2.cb",
            "Include \"cyc1.cb\"\nFunction inTwo() As Integer\nReturn 2\nEndFunction\n",
        );
        let main = write(
            &dir,
            "cyc1.cb",
            "Include \"cyc2.cb\"\nFunction inOne() As Integer\nReturn 1\nEndFunction\n",
        );

        let r = resolve_file(&main);

        assert!(codes(&r).is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.sources.len(), 2, "cyc1 + cyc2, no infinite loop");
        assert_eq!(r.program.len(), 2, "inTwo, inOne");
    }

    #[test]
    fn unreadable_include_reports_e0334() {
        let dir = tempdir().unwrap();
        let main = write(&dir, "main.cb", "Include \"nope.cb\"\nPrint 1\n");

        let r = resolve_file(&main);

        assert_eq!(codes(&r), vec!["E0334"]);
        // The missing include contributes nothing; only `Print 1` remains.
        assert_eq!(r.program.len(), 1);
    }

    #[test]
    fn absolute_include_path_is_used_as_is() {
        let dir = tempdir().unwrap();
        let lib = write(
            &dir,
            "lib.cb",
            "Function f() As Integer\nReturn 1\nEndFunction\n",
        );
        // Reference the lib by its absolute path from a file in a different dir.
        let other = tempdir().unwrap();
        let main_body = format!("Include \"{}\"\nPrint f()\n", lib.display());
        let main = other.path().join("main.cb");
        fs::write(&main, &main_body).unwrap();

        let r = resolve(&main, main_body);

        assert!(codes(&r).is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.sources.len(), 2);
        assert_eq!(r.program.len(), 2);
    }
}
