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
use crate::source::SourceMap;

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
/// (caller bug) and [`io::ErrorKind::InvalidData`] for downstream
/// codespan-reporting failures.
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
/// and the referenced [`FileId`](crate::FileId) must exist in the
/// [`SourceMap`].
fn validate_label(label: &Label, sources: &SourceMap) -> io::Result<()> {
    if label.span.end < label.span.start {
        let msg = format!(
            "invalid Span: end ({}) < start ({})",
            label.span.end, label.span.start
        );
        eprintln!("cb-diagnostics: {msg}");
        return Err(io::Error::new(io::ErrorKind::InvalidInput, msg));
    }
    if sources.get(label.span.file).is_none() {
        let msg = format!("Span references unknown FileId({})", label.span.file.0);
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
    labels.push(to_cs_label(&diag.primary, cs_diag::LabelStyle::Primary));
    for sec in &diag.secondary {
        labels.push(to_cs_label(sec, cs_diag::LabelStyle::Secondary));
    }

    let mut out = cs_diag::Diagnostic::new(severity)
        .with_message(&diag.message)
        .with_labels(labels);
    if let Some(code) = diag.code {
        out = out.with_code(code.as_str());
    }
    if !diag.notes.is_empty() {
        out = out.with_notes(diag.notes.clone());
    }
    out
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
}
