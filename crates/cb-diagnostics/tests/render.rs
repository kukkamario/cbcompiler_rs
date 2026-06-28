//! Integration tests for [`CliRenderer`].
//!
//! The renderer is load-bearing for every user-visible diagnostic, so
//! these tests pin the rendered form against snapshots (success paths)
//! and exercise the validation failure modes
//! (`Err` paths). Black-box: everything goes through the published
//! `cb_diagnostics` API.

use std::io;

use cb_diagnostics::{CliRenderer, Diagnostic, FileId, Label, Renderer, SourceMap, Span};
use codespan_reporting::term::termcolor::NoColor;

/// Render `diag` against `sources` into an in-memory `NoColor` buffer
/// and return the captured UTF-8 output.
fn render(diag: &Diagnostic, sources: &SourceMap) -> io::Result<String> {
    let mut r = CliRenderer::new(NoColor::new(Vec::<u8>::new()));
    r.emit(diag, sources)?;
    let bytes = r.into_inner().into_inner();
    Ok(String::from_utf8(bytes).expect("renderer output is utf-8"))
}

#[test]
fn single_line_single_label() {
    let mut sources = SourceMap::new();
    let file = sources.add("test.cb".into(), "let x = 42\n".into());
    let diag = Diagnostic::error(
        "E0001",
        "demo error",
        Label::with_message(Span::new(4, 5, file), "here"),
    );
    let out = render(&diag, &sources).expect("emit");
    insta::assert_snapshot!(out);
}

#[test]
fn span_crossing_two_lines() {
    // `"line one\nline two\n"`. Span 5..14 covers the tail of line 1
    // (`"ne\n"`) plus the head of line 2 (`"line t"`) — codespan-reporting
    // renders this as a multi-line underlined region.
    let mut sources = SourceMap::new();
    let file = sources.add("two_lines.cb".into(), "line one\nline two\n".into());
    let diag = Diagnostic::error(
        "E0002",
        "spans two lines",
        Label::with_message(Span::new(5, 14, file), "crosses newline"),
    );
    let out = render(&diag, &sources).expect("emit");
    insta::assert_snapshot!(out);
}

#[test]
fn primary_and_secondary_in_different_files() {
    let mut sources = SourceMap::new();
    let main = sources.add("main.cb".into(), "Dim x As Int\n".into());
    let other = sources.add("other.cb".into(), "Dim x As Float\n".into());
    let diag = Diagnostic::error(
        "E0003",
        "type mismatch across files",
        Label::with_message(Span::new(4, 5, main), "primary x"),
    )
    .with_secondary(Label::with_message(
        Span::new(4, 5, other),
        "also declared here",
    ));
    let out = render(&diag, &sources).expect("emit");
    insta::assert_snapshot!(out);
}

#[test]
fn unknown_file_id_returns_err() {
    let mut sources = SourceMap::new();
    let _real = sources.add("real.cb".into(), "x = 1\n".into());
    // `FileId(99)` is not in `sources`.
    let diag = Diagnostic::error("E0004", "boom", Label::new(Span::new(0, 1, FileId(99))));
    let err = render(&diag, &sources).expect_err("unknown FileId must fail");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn span_past_eof_returns_err() {
    let mut sources = SourceMap::new();
    let file = sources.add("short.cb".into(), "abc".into()); // 3 bytes
    let diag = Diagnostic::error("E0005", "boom", Label::new(Span::new(0, 100, file)));
    let err = render(&diag, &sources).expect_err("span past EOF must fail");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}
