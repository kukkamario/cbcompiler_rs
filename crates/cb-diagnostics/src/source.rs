//! Source files and line/column resolution.
//!
//! [`SourceMap`] holds the loaded source files keyed by [`FileId`]. Each
//! [`Source`] precomputes a [`LineIndex`] so diagnostics can translate byte
//! offsets to (line, column) coordinates lazily and cheaply.

/// Opaque identifier for a source file inside a [`SourceMap`].
///
/// `FileId(u32::MAX)` is reserved as [`FileId::SYNTHETIC`] for tests and
/// internal placeholders.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct FileId(pub u32);

impl FileId {
    /// Sentinel `FileId` for synthetic/test spans not backed by a real file.
    pub const SYNTHETIC: FileId = FileId(u32::MAX);
}

/// A single loaded source file: name, full text, and a precomputed
/// [`LineIndex`] for fast offset-to-(line, column) lookups.
#[derive(Debug, Clone)]
pub struct Source {
    pub name: String,
    pub text: String,
    line_index: LineIndex,
}

impl Source {
    /// Build a `Source` from a display name and the file's text. The line
    /// index is computed eagerly.
    pub fn new(name: String, text: String) -> Self {
        let line_index = LineIndex::new(&text);
        Self {
            name,
            text,
            line_index,
        }
    }

    /// Borrow the precomputed line index.
    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }
}

/// Collection of source files indexed by [`FileId`].
#[derive(Debug, Default, Clone)]
pub struct SourceMap {
    sources: Vec<Source>,
}

impl SourceMap {
    /// Create an empty map.
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    /// Add a source file, returning its newly-assigned [`FileId`].
    ///
    /// # Panics
    ///
    /// Panics if more than `u32::MAX - 1` files are added (the last `u32`
    /// value is reserved for [`FileId::SYNTHETIC`]).
    pub fn add(&mut self, name: String, text: String) -> FileId {
        let idx = self.sources.len();
        let id = u32::try_from(idx).expect("source map index overflowed u32");
        assert!(
            id != u32::MAX,
            "source map exhausted: cannot allocate FileId({}) — reserved as SYNTHETIC",
            u32::MAX
        );
        self.sources.push(Source::new(name, text));
        FileId(id)
    }

    /// Get a source by id. Returns `None` for unknown ids, including
    /// [`FileId::SYNTHETIC`].
    pub fn get(&self, file: FileId) -> Option<&Source> {
        self.sources.get(file.0 as usize)
    }

    /// Number of loaded sources.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Whether no sources are loaded.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// Maps byte offsets to (line, column) coordinates within a single source.
///
/// `newline_offsets[i]` is the byte offset where line `i + 2` starts — i.e.
/// the position immediately after the `i`-th line terminator. Line 1 always
/// starts at offset `0`. `\r\n` is treated as one terminator (length 2);
/// `\n` and bare `\r` are each length 1 terminators.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offsets at which lines after the first begin.
    newline_offsets: Vec<u32>,
    /// Total length of the indexed text, in bytes.
    text_len: u32,
}

impl LineIndex {
    /// Build a `LineIndex` by scanning `text` once for line terminators.
    pub fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut newline_offsets = Vec::new();
        let mut i = 0usize;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'\r' {
                // CRLF counts as one terminator of length 2; bare CR is length 1.
                let next = if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i + 2
                } else {
                    i + 1
                };
                let start = u32::try_from(next).expect("source larger than u32::MAX bytes");
                newline_offsets.push(start);
                i = next;
            } else if b == b'\n' {
                let next = i + 1;
                let start = u32::try_from(next).expect("source larger than u32::MAX bytes");
                newline_offsets.push(start);
                i = next;
            } else {
                i += 1;
            }
        }
        let text_len = u32::try_from(bytes.len()).expect("source larger than u32::MAX bytes");
        Self {
            newline_offsets,
            text_len,
        }
    }

    /// Number of lines in the indexed text. Always at least 1.
    pub fn line_count(&self) -> usize {
        self.newline_offsets.len() + 1
    }

    /// Total length of the indexed text in bytes.
    pub fn text_len(&self) -> u32 {
        self.text_len
    }

    /// Translate a byte offset to a (line, column-in-bytes) pair.
    ///
    /// Lines are 1-based; columns are 0-based byte offsets into the line.
    /// Offsets at or beyond the end of the text are clamped to the last
    /// line's coordinates — this function never panics.
    pub fn offset_to_line_col(&self, offset: u32) -> (u32, u32) {
        let clamped = offset.min(self.text_len);
        // Find the first newline-start strictly greater than `clamped`.
        // partition_point returns the count of elements `<= clamped`, which
        // equals the 0-based line index of `clamped`.
        let line0 = self
            .newline_offsets
            .partition_point(|&start| start <= clamped);
        let line_start = if line0 == 0 {
            0
        } else {
            self.newline_offsets[line0 - 1]
        };
        let col = clamped - line_start;
        // Convert to 1-based line number; line0 already u-sized so cast safely.
        let line = u32::try_from(line0 + 1).expect("line count overflowed u32");
        (line, col)
    }

    /// Returns the 0-based line index (suitable for codespan-reporting's
    /// `Files::line_index`) for the given byte offset.
    pub fn line_index_of_offset(&self, offset: u32) -> usize {
        let (line1, _) = self.offset_to_line_col(offset);
        (line1 as usize) - 1
    }

    /// Returns the `[start, end)` byte range of the given 0-based line.
    ///
    /// `end` includes the line terminator (if any). Returns `None` if
    /// `line_index` is out of range.
    pub fn line_byte_range(&self, line_index: usize) -> Option<(u32, u32)> {
        if line_index >= self.line_count() {
            return None;
        }
        let start = if line_index == 0 {
            0
        } else {
            self.newline_offsets[line_index - 1]
        };
        let end = self
            .newline_offsets
            .get(line_index)
            .copied()
            .unwrap_or(self.text_len);
        Some((start, end))
    }
}

#[cfg(test)]
mod tests {}
