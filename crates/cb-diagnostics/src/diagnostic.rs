//! Structured diagnostic types produced by the frontend.

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
    /// Construct a span. `end` must be `>= start` (not enforced here so the
    /// constructor stays cheap; callers are responsible).
    pub const fn new(start: u32, end: u32, file: FileId) -> Self {
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
    pub code: Option<&'static str>,
    pub message: String,
    pub primary: Label,
    pub secondary: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// Construct a diagnostic with explicit severity.
    pub fn new(
        severity: Severity,
        code: &'static str,
        message: impl Into<String>,
        primary: Label,
    ) -> Self {
        Self {
            severity,
            code: Some(code),
            message: message.into(),
            primary,
            secondary: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Construct an `Error`-severity diagnostic.
    pub fn error(code: &'static str, message: impl Into<String>, primary: Label) -> Self {
        Self::new(Severity::Error, code, message, primary)
    }

    /// Construct a `Warning`-severity diagnostic.
    pub fn warning(code: &'static str, message: impl Into<String>, primary: Label) -> Self {
        Self::new(Severity::Warning, code, message, primary)
    }

    /// Construct a `Note`-severity diagnostic.
    pub fn note(code: &'static str, message: impl Into<String>, primary: Label) -> Self {
        Self::new(Severity::Note, code, message, primary)
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
}

#[cfg(test)]
mod tests {}
