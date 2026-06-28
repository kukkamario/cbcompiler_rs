//! Regression tests: the parser must never abort the process on
//! pathologically nested input.
//!
//! Earlier the parser recursed once per nesting level with no depth
//! cap, so a few-thousand-deep `((((…))))` overflowed the stack and aborted
//! (`exit 134`). The `parser_props` `no_panic_on_arbitrary_utf8` proptest
//! can't realistically generate thousands of balanced delimiters, so these
//! deterministic cases pin the behaviour instead: a deep input now yields an
//! `E0218` diagnostic and recovers, and printing the recovered AST is safe.

use cb_frontend::ast::{Expr, Node, NodeId, Stmt, TypeExpr};
use cb_frontend::span::SpanExt;
use cb_frontend::{FileId, LexerOptions, Span, parse, tokenize};

/// Depth comfortably past the parser's `MAX_RECURSION_DEPTH` (256) and well
/// past the level that used to overflow the stack.
const DEEP: usize = 6000;

fn parse_src(src: &str) -> cb_frontend::ParseResult {
    let (tokens, _) = tokenize(src, FileId(0), LexerOptions::default());
    parse(&tokens, src, FileId(0))
}

fn has_nesting_too_deep(r: &cb_frontend::ParseResult) -> bool {
    r.diagnostics.iter().any(|d| d.code_is("E0218"))
}

#[test]
fn deeply_nested_parens_yield_diagnostic_not_abort() {
    // `x = ((((…1…))))` — the exact shape that aborted the process. Reaching
    // this line at all (no `exit 134`) is most of the test.
    let src = format!("x = {}1{}", "(".repeat(DEEP), ")".repeat(DEEP));
    let r = parse_src(&src);
    assert!(
        has_nesting_too_deep(&r),
        "expected an E0218 nesting-too-deep diagnostic, got: {:?}",
        r.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
    );
}

#[test]
fn deeply_nested_prefix_ops_yield_diagnostic_not_abort() {
    // `----…x` recurses through the prefix arm of `parse_expr_bp`.
    let src = format!("x = {}y", "-".repeat(DEEP));
    let r = parse_src(&src);
    assert!(has_nesting_too_deep(&r));
}

#[test]
fn deeply_nested_calls_yield_diagnostic_not_abort() {
    // `f(g(h(…)))` recurses through the postfix-call arm.
    let src = format!("x = {}{}", "f(".repeat(DEEP), ")".repeat(DEEP));
    let r = parse_src(&src);
    assert!(has_nesting_too_deep(&r));
}

#[test]
fn deeply_nested_types_yield_diagnostic_not_abort() {
    // `Dim a As ((((Integer))))` recurses through `parse_type_atom`.
    let src = format!("Dim a As {}Integer{}", "(".repeat(DEEP), ")".repeat(DEEP));
    let r = parse_src(&src);
    assert!(has_nesting_too_deep(&r));
}

#[test]
fn dump_ast_on_deep_input_does_not_abort() {
    // `--dump-ast` walks the AST with `ast_print::print_node`, which had the
    // same unbounded recursion. Printing the recovered tree must not abort.
    let src = format!("x = {}1{}", "(".repeat(DEEP), ")".repeat(DEEP));
    let r = parse_src(&src);
    let mut out = String::new();
    for &root in &r.program {
        cb_frontend::ast_print::debug_print(&mut out, &r.arena, root)
            .expect("writing to a String never fails");
    }
    // The point is that we got here without a stack overflow; the elision
    // marker only appears for a tree deeper than the printer's own cap,
    // which the parser's recovery keeps us under, so just assert non-empty.
    assert!(!out.is_empty());
}

#[test]
fn depth_resets_between_statements() {
    // A deep (recovered) statement must not poison the depth budget of a
    // following well-formed statement. The second line should parse cleanly.
    let deep_line = format!("x = {}1{}\n", "(".repeat(DEEP), ")".repeat(DEEP));
    let src = format!("{deep_line}y = 1 + 2 + 3\n");
    let r = parse_src(&src);
    // Exactly one E0218 (from the first line); the second line is shallow and
    // must not trip the guard.
    let deep_count = r.diagnostics.iter().filter(|d| d.code_is("E0218")).count();
    assert_eq!(
        deep_count, 1,
        "depth budget leaked into the second statement"
    );
}

#[test]
fn moderately_nested_input_still_parses_without_diagnostic() {
    // Well under the cap: legitimate nesting must be unaffected.
    let depth = 100;
    let src = format!("x = {}1{}", "(".repeat(depth), ")".repeat(depth));
    let r = parse_src(&src);
    assert!(
        !has_nesting_too_deep(&r),
        "100-deep nesting should be well within MAX_RECURSION_DEPTH",
    );
    assert!(
        r.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        r.diagnostics
    );
}

// ── SpanExt::slice ───────────────────────────────────────────────────────

#[test]
fn span_slice_on_valid_range_returns_text() {
    let src = "hello world";
    let span = Span {
        start: 0,
        end: 5,
        file: FileId(0),
    };
    assert_eq!(span.slice(src), "hello");
}

#[test]
fn span_slice_out_of_range_returns_empty() {
    let src = "abc";
    let span = Span {
        start: 1,
        end: 99,
        file: FileId(0),
    };
    assert_eq!(span.slice(src), "");
}

#[test]
fn span_slice_inverted_returns_empty() {
    let src = "abc";
    let span = Span {
        start: 3,
        end: 1,
        file: FileId(0),
    };
    assert_eq!(span.slice(src), "");
}

#[test]
fn span_slice_on_non_char_boundary_returns_empty() {
    // `é` is two bytes (0xC3 0xA9); byte offset 1 is mid-codepoint. Raw
    // `&src[..1]` would panic — the defensive slice returns "" instead.
    let src = "é";
    assert_eq!(src.len(), 2);
    let span = Span {
        start: 0,
        end: 1,
        file: FileId(0),
    };
    assert_eq!(span.slice(src), "");
}

/// Sanity helper assertions so an unused-import lint can't fire and the
/// AST-node imports stay meaningful even if the asserts above change.
#[test]
fn error_node_variants_exist() {
    fn is_error(n: &Node) -> bool {
        matches!(
            n,
            Node::Expr(Expr::Error) | Node::Stmt(Stmt::Error) | Node::TypeExpr(TypeExpr::Error)
        )
    }
    let r = parse_src("x = (");
    let any_error = (0..r.arena.len()).any(|i| is_error(&r.arena[NodeId(i as u32)]));
    assert!(any_error);
}
