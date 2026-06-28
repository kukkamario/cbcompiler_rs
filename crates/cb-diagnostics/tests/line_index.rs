//! Unit tests for [`cb_diagnostics::LineIndex`], the [`Source`]
//! char-column helper, and [`SourceMap`] dedupe.

use cb_diagnostics::{LineIndex, Source, SourceMap, SourceMapFiles};
use codespan_reporting::files::Files;

#[test]
fn empty_string() {
    let li = LineIndex::new("");
    assert_eq!(li.offset_to_line_byte_col(0), (1, 0));
    assert_eq!(li.line_count(), 1);
    assert_eq!(li.text_len(), 0);
}

#[test]
fn single_line_no_terminator() {
    let li = LineIndex::new("hello");
    assert_eq!(li.offset_to_line_byte_col(0), (1, 0));
    assert_eq!(li.offset_to_line_byte_col(5), (1, 5));
    assert_eq!(li.line_count(), 1);
}

#[test]
fn lf_line_endings() {
    let li = LineIndex::new("a\nb\nc");
    assert_eq!(li.offset_to_line_byte_col(0), (1, 0));
    assert_eq!(li.offset_to_line_byte_col(2), (2, 0));
    assert_eq!(li.offset_to_line_byte_col(4), (3, 0));
    assert_eq!(li.line_count(), 3);
}

/// Pin the `partition_point` boundary: a byte offset landing *on* a terminator
/// is attributed to the line that terminator ends, with its in-line column —
/// not folded onto the next line. The other LF tests only probe line *starts*,
/// so this guards the `start <= clamped` comparison against off-by-one drift.
#[test]
fn terminator_byte_belongs_to_the_line_it_ends() {
    // "ab\ncd": a0 b1 \n2 c3 d4. The '\n' is byte 2, column 2 of line 1.
    let li = LineIndex::new("ab\ncd");
    assert_eq!(li.offset_to_line_byte_col(1), (1, 1)); // 'b'
    assert_eq!(li.offset_to_line_byte_col(2), (1, 2)); // the '\n' itself — still line 1
    assert_eq!(li.offset_to_line_byte_col(3), (2, 0)); // 'c' — first byte of line 2
}

#[test]
fn crlf_line_endings() {
    let li = LineIndex::new("a\r\nb\r\nc");
    assert_eq!(li.offset_to_line_byte_col(0), (1, 0));
    assert_eq!(li.offset_to_line_byte_col(3), (2, 0));
    assert_eq!(li.offset_to_line_byte_col(6), (3, 0));
    assert_eq!(li.line_count(), 3);
}

#[test]
fn bare_cr_line_endings() {
    let li = LineIndex::new("a\rb\rc");
    assert_eq!(li.offset_to_line_byte_col(0), (1, 0));
    assert_eq!(li.offset_to_line_byte_col(2), (2, 0));
    assert_eq!(li.offset_to_line_byte_col(4), (3, 0));
    assert_eq!(li.line_count(), 3);
}

/// A8: rewritten from the no-op `out_of_bounds_clamps` to actually assert.
#[test]
fn out_of_bounds_clamps_to_text_len() {
    let li = LineIndex::new("a\nb");
    let at_len = li.offset_to_line_byte_col(li.text_len());
    let past_end = li.offset_to_line_byte_col(100);
    assert_eq!(past_end, at_len);
    assert_eq!(past_end, (2, 1));
}

#[test]
fn line_index_of_offset_lf() {
    let li = LineIndex::new("a\nb\nc");
    assert_eq!(li.line_index_of_offset(0), 0);
    assert_eq!(li.line_index_of_offset(2), 1);
    assert_eq!(li.line_index_of_offset(4), 2);
}

#[test]
fn line_index_of_offset_clamps_past_end() {
    let li = LineIndex::new("a\nb");
    assert_eq!(
        li.line_index_of_offset(100),
        li.line_index_of_offset(li.text_len())
    );
}

#[test]
fn line_byte_range_lf() {
    let li = LineIndex::new("ab\ncde\nfg");
    // Line 0: "ab\n" → 0..3
    assert_eq!(li.line_byte_range(0), Some((0, 3)));
    // Line 1: "cde\n" → 3..7
    assert_eq!(li.line_byte_range(1), Some((3, 7)));
    // Line 2: "fg" (no terminator) → 7..9
    assert_eq!(li.line_byte_range(2), Some((7, 9)));
}

#[test]
fn line_byte_range_crlf_pair_is_one_terminator() {
    let li = LineIndex::new("a\r\nb");
    // Line 0 spans through CRLF: 0..3
    assert_eq!(li.line_byte_range(0), Some((0, 3)));
    assert_eq!(li.line_byte_range(1), Some((3, 4)));
}

#[test]
fn line_byte_range_out_of_range_is_none() {
    // "a\nb" has 2 lines (indices 0 and 1); index 2 is past the end.
    let li = LineIndex::new("a\nb");
    assert_eq!(li.line_byte_range(1), Some((2, 3)));
    assert_eq!(li.line_byte_range(2), None);
    assert_eq!(li.line_byte_range(usize::MAX), None);
}

#[test]
fn line_count_empty() {
    assert_eq!(LineIndex::new("").line_count(), 1);
}

#[test]
fn line_count_no_terminator() {
    assert_eq!(LineIndex::new("abc").line_count(), 1);
}

/// Pin the current trailing-newline behaviour: `"a\n"` counts as 2 lines —
/// the empty trailing line is counted explicitly, not collapsed.
#[test]
fn line_count_trailing_newline_counts_empty_line() {
    let li = LineIndex::new("a\n");
    assert_eq!(li.line_count(), 2);
    // The empty trailing line has its byte range pinned to (2, 2).
    assert_eq!(li.line_byte_range(1), Some((2, 2)));
}

/// A8: multi-byte UTF-8. The lambda character (U+03BB) takes 2 bytes; the
/// rocket emoji (U+1F680) takes 4 bytes. Byte columns and char columns
/// diverge.
#[test]
fn multi_byte_utf8_byte_col_vs_char_col() {
    let text = "λ🚀x";
    let src = Source::new("synthetic.cb".into(), text.into());
    let li = src.line_index();
    // Byte length: 2 + 4 + 1 = 7.
    assert_eq!(li.text_len(), 7);
    // After lambda (2 bytes): byte col = 2, char col = 1.
    assert_eq!(li.offset_to_line_byte_col(2), (1, 2));
    assert_eq!(src.offset_to_line_char_col(2), (1, 1));
    // After rocket (2+4=6 bytes): byte col = 6, char col = 2.
    assert_eq!(li.offset_to_line_byte_col(6), (1, 6));
    assert_eq!(src.offset_to_line_char_col(6), (1, 2));
    // After x: byte col = 7, char col = 3.
    assert_eq!(li.offset_to_line_byte_col(7), (1, 7));
    assert_eq!(src.offset_to_line_char_col(7), (1, 3));
}

/// A mid-codepoint byte offset must not panic. Before the fix,
/// `offset_to_line_char_col` clamped to `text_len` but did not snap to a
/// `char` boundary, so slicing `text[..mid_codepoint]` panicked with
/// "byte index N is not a char boundary". The boundary floor now makes a
/// bad offset return a clamped position instead.
#[test]
fn offset_to_line_char_col_mid_codepoint_does_not_panic() {
    // `λ` (U+03BB) is 2 bytes; offset 1 is mid-codepoint.
    let src = Source::new("synthetic.cb".into(), "λx".into());
    // Offset 1 floors back to the λ start (boundary 0): 0 chars precede it.
    assert_eq!(src.offset_to_line_char_col(1), (1, 0));
    // A 4-byte rocket on the next line; offsets 1..4 within it are all
    // mid-codepoint and must each floor to the rocket's start (char col 0).
    let src2 = Source::new("synthetic2.cb".into(), "a\n🚀".into());
    for bad in [3u32, 4, 5] {
        // Line 2 starts at byte 2; the rocket occupies bytes 2..6.
        assert_eq!(src2.offset_to_line_char_col(bad), (2, 0));
    }
}

#[test]
fn offset_to_line_char_col_ascii_matches_byte_col() {
    let src = Source::new("ascii.cb".into(), "hello\nworld".into());
    let li = src.line_index();
    for offset in [0u32, 1, 5, 6, 11] {
        assert_eq!(
            src.offset_to_line_char_col(offset),
            li.offset_to_line_byte_col(offset)
        );
    }
}

/// A8: mixed line endings in one source. Pin each line boundary.
#[test]
fn mixed_line_endings() {
    let text = "a\nb\r\nc\rd";
    let li = LineIndex::new(text);
    assert_eq!(li.line_count(), 4);
    // Line starts at offsets: 0 ('a'), 2 ('b'), 5 ('c'), 7 ('d').
    assert_eq!(li.offset_to_line_byte_col(0), (1, 0));
    assert_eq!(li.offset_to_line_byte_col(2), (2, 0));
    assert_eq!(li.offset_to_line_byte_col(5), (3, 0));
    assert_eq!(li.offset_to_line_byte_col(7), (4, 0));
}

/// A2: explicit cross-check that on a bare-`\r` source, our
/// `SourceMapFiles` adapter and `LineIndex::line_index_of_offset` agree.
/// This pins the adapter so any future drift (either side changing its
/// `\r` handling) breaks the test.
#[test]
fn bare_cr_cross_check_files_adapter_agrees_with_line_index() {
    let mut sources = SourceMap::new();
    let file = sources.add("cr.cb".into(), "a\rb\rc".into());

    let src = sources.get(file).expect("file just added");
    let li = src.line_index();
    let adapter = SourceMapFiles(&sources);
    let file_usize = file.0 as usize;

    // Offsets inside the source, including bytes immediately after each \r.
    for offset in [0u32, 1, 2, 3, 4] {
        let expected = li.line_index_of_offset(offset);
        let via_adapter = adapter
            .line_index(file_usize, offset as usize)
            .expect("offset in range");
        assert_eq!(
            via_adapter, expected,
            "files adapter disagreed with LineIndex at offset {offset}"
        );
    }
}

// A5: SourceMap dedupe and add_anonymous.

#[test]
fn source_map_add_dedupes_same_name() {
    let mut sm = SourceMap::new();
    let id1 = sm.add("foo.cb".into(), "x = 1".into());
    let id2 = sm.add("foo.cb".into(), "x = 1".into());
    assert_eq!(id1, id2);
    assert_eq!(sm.len(), 1);
}

#[test]
#[should_panic(expected = "different text")]
fn source_map_add_same_name_divergent_text_panics() {
    // The dedupe contract is integrity-checked in all build modes (not just
    // debug): a same-name/different-text add is a caller bug, not a silent
    // stale-source render. Runs identically under `cargo test --release`.
    let mut sm = SourceMap::new();
    sm.add("foo.cb".into(), "x = 1".into());
    sm.add("foo.cb".into(), "x = 2".into());
}

#[test]
fn source_map_add_anonymous_always_fresh() {
    let mut sm = SourceMap::new();
    let a = sm.add_anonymous("x = 1".into());
    let b = sm.add_anonymous("x = 1".into());
    assert_ne!(a, b);
    assert_eq!(sm.len(), 2);
}

#[test]
fn source_map_get_returns_stored_text() {
    let mut sm = SourceMap::new();
    let id = sm.add("bar.cb".into(), "hello".into());
    let stored = sm.get(id).expect("file just added");
    assert_eq!(stored.name, "bar.cb");
    assert_eq!(stored.text, "hello");
}
