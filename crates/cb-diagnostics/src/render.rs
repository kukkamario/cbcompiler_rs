//! Renderers consume [`Diagnostic`]s and emit them somewhere (terminal, LSP,
//! JSON, ...). Only the terminal renderer is implemented today, built on
//! `codespan-reporting`.

use std::io;
use std::ops::Range;

use codespan_reporting::diagnostic as cs_diag;
use codespan_reporting::files::{Error as FilesError, Files};
use codespan_reporting::term;
use codespan_reporting::term::termcolor::WriteColor;

use crate::diagnostic::{Diagnostic, Label, Severity};
use crate::source::{FileId, SourceMap};

/// A consumer of [`Diagnostic`]s.
///
/// The frontend produces diagnostics; how they are surfaced (terminal output,
/// LSP messages, structured JSON for tests) is left to implementations of
/// this trait.
///
/// Implementations return [`io::Result<()>`] so that I/O failures, malformed
/// spans, and labels referencing unknown [`FileId`](crate::FileId)s are
/// surfaced to the caller instead of being silently swallowed. Mapping
/// from typed codespan-reporting errors to [`io::Error`] uses
/// [`io::ErrorKind::InvalidInput`] for label-validation failures
/// (caller bug; produced by `validate_label`, not `emit`) and
/// [`io::ErrorKind::InvalidData`] for downstream codespan-reporting
/// failures (produced by `emit`'s non-`Io` arm).
pub trait Renderer {
    /// Emit a single diagnostic. Implementations resolve spans against the
    /// supplied [`SourceMap`].
    fn emit(&mut self, diag: &Diagnostic, sources: &SourceMap) -> io::Result<()>;
}

/// Terminal renderer built on `codespan-reporting`.
///
/// Generic over any [`WriteColor`] sink, so callers can plug in
/// `StandardStream`, a no-color buffer for tests, etc.
pub struct CliRenderer<W: WriteColor> {
    writer: W,
    config: term::Config,
}

impl<W: WriteColor> CliRenderer<W> {
    /// Wrap a writer with default rendering config.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            config: term::Config::default(),
        }
    }

    /// Wrap a writer with a custom rendering config.
    pub fn with_config(writer: W, config: term::Config) -> Self {
        Self { writer, config }
    }

    /// Borrow the configured `term::Config` so callers can tweak it.
    pub fn config_mut(&mut self) -> &mut term::Config {
        &mut self.config
    }

    /// Consume the renderer and return its writer.
    ///
    /// Useful in tests: after rendering into a `NoColor<Vec<u8>>` buffer,
    /// pull the buffer back out for assertion against snapshot output.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: WriteColor> Renderer for CliRenderer<W> {
    fn emit(&mut self, diag: &Diagnostic, sources: &SourceMap) -> io::Result<()> {
        validate_label(&diag.primary, sources)?;
        for sec in &diag.secondary {
            validate_label(sec, sources)?;
        }

        let files = SourceMapFiles(sources);
        let cs = to_codespan(diag);
        match term::emit(&mut self.writer, &self.config, &files, &cs) {
            Ok(()) => Ok(()),
            Err(FilesError::Io(e)) => Err(e),
            Err(other) => {
                eprintln!("cb-diagnostics: internal renderer error: {other}");
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    other.to_string(),
                ))
            }
        }
    }
}

/// Pre-flight check for one [`Label`]: the span must have `end >= start`,
/// the referenced [`FileId`](crate::FileId) must exist in the
/// [`SourceMap`], and `end` must not exceed the source's byte length
/// (`end` is exclusive, so `end == text_len` is valid).
///
/// A label on the [`FileId::SYNTHETIC`] sentinel is an exception: it has no
/// backing source, so it cannot be range-checked and is *not* an error here.
/// The renderer degrades instead of aborting — [`to_codespan`] drops the
/// snippet and folds the label's message into a note. This keeps a synthetic
/// span (e.g. a built-in/runtime declaration site) from swallowing an
/// otherwise-renderable real error (FD-027). Genuine caller bugs — an inverted
/// span, or an unknown *non-synthetic* `FileId` — still fail hard.
fn validate_label(label: &Label, sources: &SourceMap) -> io::Result<()> {
    // All-builds backstop for the same `end >= start` invariant that
    // [`Span::new`](crate::Span::new) only debug-asserts. Spans constructed
    // via direct field syntax (or in release builds) reach here unchecked.
    //
    // Note: we trust `start`/`end` to land on `char` boundaries — unlike
    // [`Source::offset_to_line_char_col`](crate::Source::offset_to_line_char_col),
    // which floors mid-codepoint offsets, this does *not* reject a span whose
    // bounds split a UTF-8 sequence. codespan-reporting tolerates such ranges.
    if label.span.end < label.span.start {
        let msg = format!(
            "invalid Span: end ({}) < start ({})",
            label.span.end, label.span.start
        );
        eprintln!("cb-diagnostics: {msg}");
        return Err(io::Error::new(io::ErrorKind::InvalidInput, msg));
    }
    if label.span.file == FileId::SYNTHETIC {
        return Ok(());
    }
    let Some(src) = sources.get(label.span.file) else {
        let msg = format!("Span references unknown FileId({})", label.span.file.0);
        eprintln!("cb-diagnostics: {msg}");
        return Err(io::Error::new(io::ErrorKind::InvalidInput, msg));
    };
    let text_len = u32::try_from(src.text.len())
        .expect("source longer than u32::MAX bytes — Source builders forbid this");
    if label.span.end > text_len {
        let msg = format!(
            "Span end ({}) exceeds source length ({}) of file '{}'",
            label.span.end, text_len, src.name
        );
        eprintln!("cb-diagnostics: {msg}");
        return Err(io::Error::new(io::ErrorKind::InvalidInput, msg));
    }
    Ok(())
}

fn to_codespan(diag: &Diagnostic) -> cs_diag::Diagnostic<usize> {
    let severity = match diag.severity {
        Severity::Error => cs_diag::Severity::Error,
        Severity::Warning => cs_diag::Severity::Warning,
        Severity::Note => cs_diag::Severity::Note,
        Severity::Help => cs_diag::Severity::Help,
    };

    let mut labels = Vec::with_capacity(1 + diag.secondary.len());
    // A label on a synthetic span has no source snippet to underline. Rather
    // than drop its message entirely, preserve it as a note so the diagnostic
    // still carries the information (FD-027).
    let mut degraded_notes: Vec<String> = Vec::new();
    push_label_or_note(
        &diag.primary,
        cs_diag::LabelStyle::Primary,
        &mut labels,
        &mut degraded_notes,
    );
    for sec in &diag.secondary {
        push_label_or_note(
            sec,
            cs_diag::LabelStyle::Secondary,
            &mut labels,
            &mut degraded_notes,
        );
    }

    let mut out = cs_diag::Diagnostic::new(severity)
        .with_message(&diag.message)
        .with_labels(labels);
    if let Some(code) = diag.code {
        out = out.with_code(code.as_str());
    }
    if !diag.notes.is_empty() || !degraded_notes.is_empty() {
        let mut notes = diag.notes.clone();
        notes.extend(degraded_notes);
        out = out.with_notes(notes);
    }
    out
}

/// Add `label` to `labels` as a codespan label, unless its span is synthetic
/// (no backing source) — in that case fold any message into `degraded_notes`
/// so it survives rendering without a snippet. See [`validate_label`].
fn push_label_or_note(
    label: &Label,
    style: cs_diag::LabelStyle,
    labels: &mut Vec<cs_diag::Label<usize>>,
    degraded_notes: &mut Vec<String>,
) {
    if label.span.file == FileId::SYNTHETIC {
        if let Some(msg) = &label.message {
            degraded_notes.push(format!("{msg} (built-in; no source location)"));
        }
        return;
    }
    labels.push(to_cs_label(label, style));
}

fn to_cs_label(label: &Label, style: cs_diag::LabelStyle) -> cs_diag::Label<usize> {
    let file_id = label.span.file.0 as usize;
    let range: Range<usize> = (label.span.start as usize)..(label.span.end as usize);
    let mut cs = cs_diag::Label::new(style, file_id, range);
    if let Some(msg) = &label.message {
        cs = cs.with_message(msg);
    }
    cs
}

/// Adapter exposing a [`SourceMap`] to `codespan-reporting`'s
/// [`Files`](codespan_reporting::files::Files) trait.
///
/// Construct one as `SourceMapFiles(&sources)` to feed a [`SourceMap`]
/// into any codespan-reporting renderer or test. The adapter routes line
/// and column queries back through our [`LineIndex`](crate::LineIndex),
/// so codespan's reported line numbers stay consistent with the rest of
/// the crate (including on bare-`\r` sources where codespan's default
/// `Files` impls would disagree).
pub struct SourceMapFiles<'a>(pub &'a SourceMap);

impl<'a> Files<'a> for SourceMapFiles<'a> {
    type FileId = usize;
    type Name = &'a str;
    type Source = &'a str;

    fn name(&'a self, id: usize) -> Result<&'a str, FilesError> {
        let file = file_from_index(self.0, id)?;
        Ok(file.name.as_str())
    }

    fn source(&'a self, id: usize) -> Result<&'a str, FilesError> {
        let file = file_from_index(self.0, id)?;
        Ok(file.text.as_str())
    }

    fn line_index(&'a self, id: usize, byte_index: usize) -> Result<usize, FilesError> {
        let file = file_from_index(self.0, id)?;
        let offset = u32::try_from(byte_index).map_err(|_| FilesError::IndexTooLarge {
            given: byte_index,
            max: u32::MAX as usize,
        })?;
        Ok(file.line_index().line_index_of_offset(offset))
    }

    fn line_range(&'a self, id: usize, line_index: usize) -> Result<Range<usize>, FilesError> {
        let file = file_from_index(self.0, id)?;
        let index = file.line_index();
        let (start, end) = index
            .line_byte_range(line_index)
            .ok_or(FilesError::LineTooLarge {
                given: line_index,
                max: index.line_count(),
            })?;
        Ok((start as usize)..(end as usize))
    }
}

fn file_from_index(map: &SourceMap, id: usize) -> Result<&crate::source::Source, FilesError> {
    let raw = u32::try_from(id).map_err(|_| FilesError::FileMissing)?;
    map.get(crate::source::FileId(raw))
        .ok_or(FilesError::FileMissing)
}

#[cfg(test)]
mod tests {
    use codespan_reporting::term::termcolor::NoColor;

    use super::*;
    use crate::diagnostic::{Label, Span};
    use crate::source::FileId;

    fn renderer() -> CliRenderer<NoColor<Vec<u8>>> {
        CliRenderer::new(NoColor::new(Vec::new()))
    }

    #[test]
    fn emit_unknown_file_id_returns_invalid_input() {
        let mut sources = SourceMap::new();
        let real = sources.add("real.cb".into(), "x = 1".into());
        let _ = real;
        // FileId(99) is not in the source map.
        let label = Label::new(Span::new(0, 1, FileId(99)));
        let diag = Diagnostic::error("E0001", "boom", label);
        let err = renderer()
            .emit(&diag, &sources)
            .expect_err("emit should fail on unknown FileId");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn emit_synthetic_label_degrades() {
        // A secondary label on the synthetic sentinel must NOT abort the whole
        // diagnostic (which would swallow the real error — FD-027). It renders
        // without a snippet, and its message is preserved as a note.
        let mut sources = SourceMap::new();
        let file = sources.add("real.cb".into(), "Dim box As Int\n".into());
        let diag = Diagnostic::error(
            "E0303",
            "`box` is a reserved runtime name",
            Label::new(Span::new(0, 3, file)),
        )
        .with_secondary(Label::with_message(
            Span::new(0, 0, FileId::SYNTHETIC),
            "previously declared here",
        ));
        let mut r = renderer();
        r.emit(&diag, &sources)
            .expect("synthetic label must degrade, not error");
        let out = String::from_utf8(r.into_inner().into_inner()).expect("utf-8");
        assert!(
            out.contains("reserved runtime name"),
            "real message preserved: {out}"
        );
        assert!(
            out.contains("previously declared here"),
            "synthetic label message folded into a note: {out}"
        );
    }

    #[test]
    fn emit_inverted_span_returns_invalid_input() {
        let mut sources = SourceMap::new();
        let file = sources.add("real.cb".into(), "x = 1".into());
        // Bypass `Span::new`'s debug-assert by constructing via direct
        // field syntax — the renderer must defend against this even when
        // the constructor's debug-assert is disabled (release builds).
        let span = Span {
            start: 5,
            end: 2,
            file,
        };
        let diag = Diagnostic::error("E0001", "boom", Label::new(span));
        let err = renderer()
            .emit(&diag, &sources)
            .expect_err("emit should fail on inverted Span");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn emit_valid_diagnostic_succeeds() {
        let mut sources = SourceMap::new();
        let file = sources.add("ok.cb".into(), "x = 1\n".into());
        let diag = Diagnostic::error("E0001", "demo", Label::new(Span::new(0, 1, file)));
        let mut r = renderer();
        r.emit(&diag, &sources).expect("emit on valid input");
    }

    #[test]
    fn emit_span_past_eof_returns_invalid_input() {
        let mut sources = SourceMap::new();
        let file = sources.add("short.cb".into(), "abc".into()); // 3 bytes
        let span = Span::new(0, 100, file);
        let diag = Diagnostic::error("E0001", "boom", Label::new(span));
        let err = renderer()
            .emit(&diag, &sources)
            .expect_err("emit should fail on span past EOF");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
