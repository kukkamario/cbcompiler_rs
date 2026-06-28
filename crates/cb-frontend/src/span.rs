//! Source spans. `Span` and `FileId` are defined in `cb-diagnostics`; this
//! module re-exports them and adds frontend-specific helpers.

pub use cb_diagnostics::{FileId, Span};

/// Frontend-side helpers on `Span`. Implemented as free functions on `Span`
/// via an extension trait so we don't have to redefine the struct here.
pub trait SpanExt {
    /// Merge two spans into one covering both. Panics if the file IDs differ.
    fn merge(self, other: Span) -> Span;
    /// Slice the source text covered by this span.
    ///
    /// Returns `""` if the span is out of range, inverted, or lands on a
    /// non-`char`-boundary byte offset — it never panics. This matches the
    /// defensive convention of `parser::span_slice` and
    /// `string_value::slice`; raw `&source[start..end]` indexing here would be
    /// a latent panic on a bad offset.
    fn slice(self, source: &str) -> &str;
}

impl SpanExt for Span {
    fn merge(self, other: Span) -> Span {
        assert_eq!(self.file, other.file, "cannot merge spans across files");
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
            file: self.file,
        }
    }

    fn slice(self, source: &str) -> &str {
        let start = self.start as usize;
        let end = self.end as usize;
        if start <= end
            && end <= source.len()
            && source.is_char_boundary(start)
            && source.is_char_boundary(end)
        {
            &source[start..end]
        } else {
            ""
        }
    }
}
