//! `insta` snapshot tests for the lexer against hand-curated `.cb` fixtures.

use cb_frontend::span::FileId;
use cb_frontend::{LexerOptions, tokenize};

fn snapshot_fixture(name: &str) -> String {
    let path = format!("tests/fixtures/{name}.cb");
    let src = std::fs::read_to_string(&path).expect("fixture missing");
    let (tokens, diags) = tokenize(&src, FileId(0), LexerOptions::default());
    let mut out = String::new();
    use std::fmt::Write;
    for tok in &tokens {
        let lexeme: String = src
            .get(tok.span.start as usize..tok.span.end as usize)
            .unwrap_or("")
            .chars()
            .map(|c| {
                if c == '\n' {
                    '\u{23CE}' // ⏎
                } else if c == '\r' {
                    '\u{240D}' // ␍
                } else {
                    c
                }
            })
            .collect();
        writeln!(
            out,
            "{:?} @ {}..{}  {:?}",
            tok.kind, tok.span.start, tok.span.end, lexeme
        )
        .unwrap();
    }
    if !diags.is_empty() {
        out.push_str("\n--- DIAGNOSTICS ---\n");
        for d in &diags {
            writeln!(out, "{:?} {:?} {}", d.severity, d.code, d.message).unwrap();
        }
    }
    out
}

#[test]
fn hello() {
    insta::assert_snapshot!(snapshot_fixture("hello"));
}

#[test]
fn types() {
    insta::assert_snapshot!(snapshot_fixture("types"));
}

#[test]
fn numerics() {
    insta::assert_snapshot!(snapshot_fixture("numerics"));
}

#[test]
fn strings() {
    insta::assert_snapshot!(snapshot_fixture("strings"));
}

#[test]
fn comments() {
    insta::assert_snapshot!(snapshot_fixture("comments"));
}

#[test]
fn control_flow() {
    insta::assert_snapshot!(snapshot_fixture("control_flow"));
}

#[test]
fn errors() {
    insta::assert_snapshot!(snapshot_fixture("errors"));
}
