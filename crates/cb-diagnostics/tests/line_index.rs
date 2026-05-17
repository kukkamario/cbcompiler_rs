//! Unit tests for [`cb_diagnostics::LineIndex`].

use cb_diagnostics::LineIndex;

#[test]
fn empty_string() {
    let li = LineIndex::new("");
    assert_eq!(li.offset_to_line_col(0), (1, 0));
}

#[test]
fn single_line_no_terminator() {
    let li = LineIndex::new("hello");
    assert_eq!(li.offset_to_line_col(0), (1, 0));
    assert_eq!(li.offset_to_line_col(5), (1, 5));
}

#[test]
fn lf_line_endings() {
    let li = LineIndex::new("a\nb\nc");
    assert_eq!(li.offset_to_line_col(0), (1, 0));
    assert_eq!(li.offset_to_line_col(2), (2, 0));
    assert_eq!(li.offset_to_line_col(4), (3, 0));
}

#[test]
fn crlf_line_endings() {
    let li = LineIndex::new("a\r\nb\r\nc");
    assert_eq!(li.offset_to_line_col(0), (1, 0));
    assert_eq!(li.offset_to_line_col(3), (2, 0));
    assert_eq!(li.offset_to_line_col(6), (3, 0));
}

#[test]
fn bare_cr_line_endings() {
    let li = LineIndex::new("a\rb\rc");
    assert_eq!(li.offset_to_line_col(0), (1, 0));
    assert_eq!(li.offset_to_line_col(2), (2, 0));
    assert_eq!(li.offset_to_line_col(4), (3, 0));
}

#[test]
fn out_of_bounds_clamps() {
    let li = LineIndex::new("a\nb");
    // Past end — should not panic; returns last line's coord.
    let _ = li.offset_to_line_col(100);
}
