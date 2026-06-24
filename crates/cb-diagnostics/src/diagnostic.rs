//! Structured diagnostic types produced by the frontend.

use std::fmt;

use crate::source::FileId;

/// A byte-offset span within a single source file.
///
/// `start` and `end` are byte offsets; `end` is exclusive. The frontend
/// re-exports this type so it doesn't need its own `Span` definition.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub file: FileId,
}

impl Span {
    /// Construct a span. `end` must be `>= start`.
    ///
    /// In debug builds this is enforced by [`debug_assert!`]; release
    /// builds skip the check so the constructor stays cheap on the
    /// lexer's hot path. [`Span::len`] still returns `0` for an inverted
    /// span via `saturating_sub`, so release builds do not panic on bad
    /// data — they just produce a degenerate (empty) span.
    ///
    /// The same `end >= start` invariant is re-checked at render time by
    /// `validate_label` (in `render.rs`) in *all* build modes, so an inverted
    /// span that slips past this debug-only assert is still rejected before
    /// reaching codespan.
    pub const fn new(start: u32, end: u32, file: FileId) -> Self {
        debug_assert!(end >= start, "Span::new: end < start");
        Self { start, end, file }
    }

    /// Span length in bytes.
    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Whether the span is empty (`start == end`).
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// A diagnostic code such as `"E0101"`.
///
/// Today this is a zero-cost newtype around `&'static str`. The wrapper
/// exists so the public API does not couple to the lifetime of the
/// underlying string — once generated or namespaced codes need anything
/// other than a static literal, this type can grow an interior `Cow` or
/// interner without rippling through every callsite.
///
/// Callers construct codes with [`DiagnosticCode::new`] or rely on the
/// `impl From<&'static str>` so the existing `Diagnostic::error("E0101", …)`
/// shape keeps compiling.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct DiagnosticCode(&'static str);

impl DiagnosticCode {
    /// Wrap a `&'static str` as a diagnostic code.
    pub const fn new(code: &'static str) -> Self {
        Self(code)
    }

    /// Borrow the underlying string.
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Debug for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0, f)
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl From<&'static str> for DiagnosticCode {
    fn from(s: &'static str) -> Self {
        Self(s)
    }
}

impl PartialEq<&str> for DiagnosticCode {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<DiagnosticCode> for &str {
    fn eq(&self, other: &DiagnosticCode) -> bool {
        *self == other.0
    }
}

/// Severity of a [`Diagnostic`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

/// A labelled span attached to a diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Label {
    pub span: Span,
    pub message: Option<String>,
}

impl Label {
    /// A label with no attached message.
    pub fn new(span: Span) -> Self {
        Self {
            span,
            message: None,
        }
    }

    /// A label with an inline message rendered alongside the span.
    pub fn with_message(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: Some(message.into()),
        }
    }
}

/// A structured diagnostic: severity, optional code, message, primary span,
/// and any number of secondary labels and notes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<DiagnosticCode>,
    pub message: String,
    pub primary: Label,
    pub secondary: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// Construct a diagnostic with explicit severity.
    pub fn new(
        severity: Severity,
        code: impl Into<DiagnosticCode>,
        message: impl Into<String>,
        primary: Label,
    ) -> Self {
        Self {
            severity,
            code: Some(code.into()),
            message: message.into(),
            primary,
            secondary: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Construct an `Error`-severity diagnostic.
    pub fn error(
        code: impl Into<DiagnosticCode>,
        message: impl Into<String>,
        primary: Label,
    ) -> Self {
        Self::new(Severity::Error, code, message, primary)
    }

    /// Construct a `Warning`-severity diagnostic.
    pub fn warning(
        code: impl Into<DiagnosticCode>,
        message: impl Into<String>,
        primary: Label,
    ) -> Self {
        Self::new(Severity::Warning, code, message, primary)
    }

    /// Construct a `Note`-severity diagnostic.
    pub fn note(
        code: impl Into<DiagnosticCode>,
        message: impl Into<String>,
        primary: Label,
    ) -> Self {
        Self::new(Severity::Note, code, message, primary)
    }

    /// Construct a `Help`-severity diagnostic.
    pub fn help(
        code: impl Into<DiagnosticCode>,
        message: impl Into<String>,
        primary: Label,
    ) -> Self {
        Self::new(Severity::Help, code, message, primary)
    }

    /// Append a secondary label.
    #[must_use]
    pub fn with_secondary(mut self, label: Label) -> Self {
        self.secondary.push(label);
        self
    }

    /// Append a free-form note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// True iff `self.code` is `Some` and equals `code` as a string.
    ///
    /// Lets tests compare against literal strings without spelling out the
    /// `DiagnosticCode` wrapping: `d.code_is("E0101")` instead of
    /// `d.code == Some(DiagnosticCode::new("E0101"))`.
    pub fn code_is(&self, code: &str) -> bool {
        self.code.is_some_and(|c| c == code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syn_span() -> Span {
        Span::new(0, 1, FileId::SYNTHETIC)
    }

    #[test]
    fn diagnostic_code_debug_matches_string() {
        // Manual Debug impl prints as if it were a `&str`, so existing
        // snapshot output (`{:?} d.code` → `Some("E0101")`) stays stable.
        assert_eq!(format!("{:?}", DiagnosticCode::new("E0101")), "\"E0101\"");
        assert_eq!(
            format!("{:?}", Some(DiagnosticCode::new("E0101"))),
            "Some(\"E0101\")"
        );
    }

    #[test]
    fn diagnostic_code_partial_eq_str() {
        let code = DiagnosticCode::new("E0101");
        assert!(code == "E0101");
        assert!("E0101" == code);
        assert!(code != "E0102");
    }

    #[test]
    fn help_factory_severity_is_help() {
        let d = Diagnostic::help("E0001", "hint", Label::new(syn_span()));
        assert_eq!(d.severity, Severity::Help);
        assert!(d.code_is("E0001"));
    }

    #[test]
    fn code_is_handles_none_and_mismatches() {
        let d = Diagnostic::error("E0101", "boom", Label::new(syn_span()));
        assert!(d.code_is("E0101"));
        assert!(!d.code_is("E0102"));
        let mut d2 = d.clone();
        d2.code = None;
        assert!(!d2.code_is("E0101"));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "Span::new: end < start")]
    fn span_new_end_before_start_debug_asserts() {
        let _ = Span::new(5, 2, FileId::SYNTHETIC);
    }
}
