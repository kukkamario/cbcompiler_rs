//! Decoding of `StrLit` tokens into their concrete `String` value per
//! `docs/cb_syntax.md` §1.6: `Plain` (`"..."`) is verbatim, `Escaped`
//! (`$"..."`) runs C-style escapes, `Raw` (`"""..."""`) has no escapes plus
//! indent stripping.
//!
//! `decode` is total — it always returns a best-effort `String` even when
//! diagnostics are produced. This lets the parser continue building the AST
//! instead of bailing on a malformed escape.

use cb_diagnostics::{Diagnostic, DiagnosticCode, Label};

use crate::span::Span;
use crate::token::StrLitKind;

pub(crate) const E_INVALID_ESCAPE: DiagnosticCode = DiagnosticCode::new("E0208");
pub(crate) const E_BAD_RAW_INDENT: DiagnosticCode = DiagnosticCode::new("E0209");

/// Decode a string literal's source bytes into its runtime value.
///
/// `span` is the literal's full span including its delimiting quotes (single
/// or triple). Returns the decoded string and any diagnostics produced along
/// the way. The returned `String` is always usable — even on error the
/// function falls back to a sensible best-effort value so callers can keep
/// recovery going.
pub(crate) fn decode(kind: StrLitKind, src: &str, span: Span) -> (String, Vec<Diagnostic>) {
    let raw = slice(src, span);
    match kind {
        StrLitKind::Plain => decode_plain(raw),
        StrLitKind::Escaped => decode_escaped(raw, span),
        StrLitKind::Raw => decode_raw(raw, span),
    }
}

fn slice(src: &str, span: Span) -> &str {
    let start = span.start as usize;
    let end = span.end as usize;
    if start <= end && end <= src.len() {
        &src[start..end]
    } else {
        ""
    }
}

/// `Plain`: source has the form `"...body..."` and is verbatim (FD-051) — the
/// body may contain `\`, which carries no special meaning. Strip the outer
/// quotes and return the body unchanged.
fn decode_plain(raw: &str) -> (String, Vec<Diagnostic>) {
    let body = strip_single_quotes(raw);
    (body.to_string(), Vec::new())
}

/// Strip a leading and trailing `"` from a single-line string literal. If the
/// quotes aren't both present (which shouldn't happen for a well-formed lexer
/// token, but we stay defensive), return the input verbatim.
fn strip_single_quotes(raw: &str) -> &str {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        &raw[1..raw.len() - 1]
    } else {
        raw
    }
}

/// `Escaped`: the source slice has the form `$"...body..."` (FD-051). Strip the
/// leading `$` mode marker and the outer quotes, then walk the body character
/// by character; on `\` consume an escape sequence per §1.6. Unknown escapes
/// copy the offending characters verbatim into the output and produce an
/// `E0208 InvalidEscape` diagnostic so the parser can keep going.
fn decode_escaped(raw: &str, lit_span: Span) -> (String, Vec<Diagnostic>) {
    // Strip the `$` mode marker first, then the quotes. The `$` adds one byte
    // to the literal-relative offset of every escape span (load-bearing for the
    // E0208 span alignment the F-L14 regression guards).
    let dollar_stripped = raw.strip_prefix('$');
    let after_dollar = dollar_stripped.unwrap_or(raw);
    let body = strip_single_quotes(after_dollar);
    // Offset of `body` inside the original literal. This MUST use the same
    // predicates as the stripping above: if a step didn't strip (e.g. a `$`
    // with no leading `$`, or a start quote but no end quote), the offset must
    // not advance past the bytes that are still present in `body`, or every
    // escape span would misalign (F-L14). For a well-formed `$"..."` token the
    // offset is 2 (`$` + `"`); for a defensive partial token it is less.
    let dollar_len: u32 = if dollar_stripped.is_some() { 1 } else { 0 };
    let stripped =
        after_dollar.len() >= 2 && after_dollar.starts_with('"') && after_dollar.ends_with('"');
    let quote_len: u32 = if stripped { 1 } else { 0 };
    let body_offset_in_lit: u32 = dollar_len + quote_len;

    let mut out = String::with_capacity(body.len());
    let mut diags = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' {
            // Push one UTF-8 character. `i` is at a char boundary in the valid
            // `&str`, so read the char there and advance by its encoded length.
            let ch = body[i..].chars().next().expect("byte index in bounds");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }

        // Escape: at least one more byte must follow (`\` at end of body would
        // be a lexer-level issue, but stay defensive).
        // Absolute file offset of the `\`: the literal's start, past the opening
        // quote (`body_offset_in_lit`), plus the index into `body`. `sub_span`
        // expects absolute offsets (see `decode_raw`); omitting `lit_span.start`
        // here pointed escape diagnostics at the wrong line (FD-036 follow-up).
        let escape_start_in_lit = lit_span.start + body_offset_in_lit + i as u32;
        if i + 1 >= bytes.len() {
            // Lone `\` at end of body — invalid.
            let span = sub_span(lit_span, escape_start_in_lit, escape_start_in_lit + 1);
            diags.push(Diagnostic::error(
                E_INVALID_ESCAPE,
                "incomplete escape sequence",
                Label::new(span),
            ));
            out.push('\\');
            i += 1;
            continue;
        }

        let next = bytes[i + 1];
        match next {
            b'\\' => {
                out.push('\\');
                i += 2;
            }
            b'"' => {
                out.push('"');
                i += 2;
            }
            b'n' => {
                out.push('\n');
                i += 2;
            }
            b'r' => {
                out.push('\r');
                i += 2;
            }
            b't' => {
                out.push('\t');
                i += 2;
            }
            b'0' => {
                out.push('\0');
                i += 2;
            }
            b'x' => {
                // `\xNN`: parse the two following hex digits as the Unicode code
                // point U+00NN (range [0, 0xFF]), encoded as UTF-8. CoolBasic-rs
                // strings are sequences of Unicode code points, not bytes
                // (cb_syntax.md §1.6 "String model"), so `\xFF` is U+00FF — the
                // same code point as `ÿ`, not the raw byte 0xFF. This is a
                // deliberate divergence from the original byte-string runtime.
                let hex_start = i + 2;
                let hex_end = hex_start + 2;
                if hex_end > bytes.len()
                    || !is_hex(bytes[hex_start])
                    || !is_hex(bytes[hex_start + 1])
                {
                    let span = sub_span(
                        lit_span,
                        escape_start_in_lit,
                        escape_start_in_lit + (hex_end.min(bytes.len()) - i) as u32,
                    );
                    diags.push(Diagnostic::error(
                        E_INVALID_ESCAPE,
                        "`\\x` escape needs exactly two hex digits",
                        Label::new(span),
                    ));
                    // Copy the offending chars verbatim and skip past `\x`.
                    out.push('\\');
                    out.push('x');
                    i += 2;
                } else {
                    let hi = hex_value(bytes[hex_start]);
                    let lo = hex_value(bytes[hex_start + 1]);
                    let cp = (hi << 4) | lo;
                    // 0..=0xFF is always a valid Unicode scalar value.
                    if let Some(c) = char::from_u32(cp as u32) {
                        out.push(c);
                    } else {
                        // Unreachable given the range, but stay defensive.
                        let span = sub_span(lit_span, escape_start_in_lit, escape_start_in_lit + 4);
                        diags.push(Diagnostic::error(
                            E_INVALID_ESCAPE,
                            "`\\x` value is not a valid Unicode scalar",
                            Label::new(span),
                        ));
                    }
                    i = hex_end;
                }
            }
            b'u' => {
                // `\uNNNN`: parse four following hex digits as a Unicode scalar.
                let hex_start = i + 2;
                let hex_end = hex_start + 4;
                let have_digits =
                    hex_end <= bytes.len() && (hex_start..hex_end).all(|k| is_hex(bytes[k]));
                if !have_digits {
                    let consumed = (hex_end.min(bytes.len())) - i;
                    let span = sub_span(
                        lit_span,
                        escape_start_in_lit,
                        escape_start_in_lit + consumed as u32,
                    );
                    diags.push(Diagnostic::error(
                        E_INVALID_ESCAPE,
                        "`\\u` escape needs exactly four hex digits",
                        Label::new(span),
                    ));
                    out.push('\\');
                    out.push('u');
                    i += 2;
                } else {
                    let mut cp: u32 = 0;
                    for &b in &bytes[hex_start..hex_end] {
                        cp = (cp << 4) | u32::from(hex_value(b));
                    }
                    if let Some(c) = char::from_u32(cp) {
                        out.push(c);
                    } else {
                        let span = sub_span(lit_span, escape_start_in_lit, escape_start_in_lit + 6);
                        diags.push(Diagnostic::error(
                            E_INVALID_ESCAPE,
                            "`\\u` value is not a valid Unicode scalar (surrogate or out of range)",
                            Label::new(span),
                        ));
                        // Recovery: copy the offending source verbatim (`\uNNNN`),
                        // matching the `\x` invalid-digits and unknown-escape arms
                        // (F-L13). `body[i..hex_end]` is the `\u` plus four hex
                        // digits, all ASCII, so this is a valid `&str` slice.
                        out.push_str(&body[i..hex_end]);
                    }
                    i = hex_end;
                }
            }
            _ => {
                // Unknown escape character. Copy the offending characters
                // verbatim ('\\' + the next char) and emit a diagnostic. The
                // escaped char begins at `i + 1`, a char boundary in `body`.
                let ch = body[i + 1..].chars().next().expect("byte after `\\`");
                let ch_len = ch.len_utf8();
                let escape_end_in_lit = escape_start_in_lit + 1 + ch_len as u32;
                let span = sub_span(lit_span, escape_start_in_lit, escape_end_in_lit);
                diags.push(Diagnostic::error(
                    E_INVALID_ESCAPE,
                    "unknown escape sequence",
                    Label::new(span),
                ));
                // Friendly recovery: drop the backslash and keep the literal
                // character — `"a\qb"` → `"aqb"`. This matches the choice
                // documented in the FD-002 plan.
                out.push(ch);
                i += 1 + ch_len;
            }
        }
    }

    (out, diags)
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

fn hex_value(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => 10 + (b - b'a'),
        b'A'..=b'F' => 10 + (b - b'A'),
        _ => 0,
    }
}

/// Construct a sub-span inside the surrounding literal span. The byte offsets
/// are absolute positions within the original source.
fn sub_span(lit_span: Span, start: u32, end: u32) -> Span {
    Span {
        start,
        end,
        file: lit_span.file,
    }
}

/// `Raw`: triple-quote body. No escape processing. After stripping the outer
/// `"""…"""`, perform the indent strip per §1.6:
/// - The closing `"""` must be on its own line. The whitespace from the
///   newline before it up to the `"""` is the closer's indent.
/// - Each non-empty content line must begin with at least that indent; that
///   indent is stripped from every content line.
/// - Whitespace-only lines are normalized to empty (their whitespace, which is
///   at most the closer indent, is dropped) — only the line's newline survives.
/// - The leading newline after the opening `"""` is dropped; the trailing
///   whitespace + `"""` (the closer line) is also dropped.
fn decode_raw(raw: &str, lit_span: Span) -> (String, Vec<Diagnostic>) {
    let mut diags = Vec::new();

    // Strip the outer `"""` … `"""`.
    let body = if raw.len() >= 6 && raw.starts_with("\"\"\"") && raw.ends_with("\"\"\"") {
        &raw[3..raw.len() - 3]
    } else {
        // Malformed token (shouldn't happen for a lexer-produced Raw token).
        // Treat the whole thing as the body and produce no diagnostics; this
        // is purely defensive.
        raw
    };

    // Find the last newline in the body. Everything between that newline and
    // the (already-stripped) closing `"""` is the closer's indent; everything
    // before that newline is the content. If there is no newline, the close
    // is not on its own line — emit E0209 and return the body verbatim.
    let last_nl = match body.rfind('\n') {
        Some(idx) => idx,
        None => {
            // The closing `"""` started at lit_span.end - 3. Use that span.
            let close_end = lit_span.end;
            let close_start = close_end.saturating_sub(3);
            diags.push(Diagnostic::error(
                E_BAD_RAW_INDENT,
                "closing `\"\"\"` must be on its own line",
                Label::new(sub_span(lit_span, close_start, close_end)),
            ));
            return (body.to_string(), diags);
        }
    };

    // closer-indent = body[last_nl+1 ..]. Must be whitespace-only — anything
    // else means the close isn't on its own line.
    let closer_indent = &body[last_nl + 1..];
    if !closer_indent.chars().all(|c| c == ' ' || c == '\t') {
        let close_end = lit_span.end;
        let close_start = close_end.saturating_sub(3);
        diags.push(Diagnostic::error(
            E_BAD_RAW_INDENT,
            "closing `\"\"\"` must be on its own line",
            Label::new(sub_span(lit_span, close_start, close_end)),
        ));
        return (body.to_string(), diags);
    }

    // Content includes the newline that precedes the closer line — that
    // newline is part of the value (the §1.6 example shows `…\n` as the
    // final character). Drop the single leading newline after the opener so
    // the first content line starts cleanly.
    let content = &body[..=last_nl];
    let content = content
        .strip_prefix("\r\n")
        .or_else(|| content.strip_prefix('\n'))
        .unwrap_or(content);

    let indent_len = closer_indent.len();
    // Content offset within the original literal source: the opener `"""` is
    // 3 bytes, plus however many bytes of the leading newline we dropped.
    let dropped_leading = (body[..=last_nl].len()) - content.len();
    let content_start_in_lit = lit_span.start + 3 + dropped_leading as u32;
    // Walk each line; strip the indent if present.
    let mut out = String::with_capacity(content.len());
    let mut bytes_consumed: u32 = 0;
    for line in split_keep_newlines(content) {
        let line_start_in_lit = content_start_in_lit + bytes_consumed;
        bytes_consumed += line.len() as u32;
        // Determine the trailing newline portion (CRLF, LF, or none).
        let (text, nl) = split_line_newline(line);
        let is_blank = text.chars().all(|c| c == ' ' || c == '\t');

        if is_blank {
            // Blank or whitespace-only lines emit just the newline (drop the
            // whitespace, which would otherwise be the closer-indent or less).
            out.push_str(nl);
        } else if let Some(rest) = text.strip_prefix(closer_indent) {
            out.push_str(rest);
            out.push_str(nl);
        } else {
            // Less indented than the closer: emit E0209 at the line. Then
            // strip whatever leading whitespace exists and emit the rest, so
            // recovery yields a usable string.
            let line_span = sub_span(
                lit_span,
                line_start_in_lit,
                line_start_in_lit + line.len() as u32,
            );
            diags.push(Diagnostic::error(
                E_BAD_RAW_INDENT,
                "content line is less indented than the closing `\"\"\"`",
                Label::new(line_span),
            ));
            let ws_end = text
                .find(|c: char| c != ' ' && c != '\t')
                .unwrap_or(text.len())
                .min(indent_len);
            out.push_str(&text[ws_end..]);
            out.push_str(nl);
        }
    }

    (out, diags)
}

/// Split `text` into lines while keeping the trailing newline on each line.
/// The final line may be empty (after a trailing newline).
fn split_keep_newlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            out.push(&text[start..=i]);
            start = i + 1;
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(&text[start..]);
    }
    out
}

/// Split a line (possibly containing a trailing `\r\n` or `\n`) into its text
/// portion and the trailing newline portion.
fn split_line_newline(line: &str) -> (&str, &str) {
    if let Some(stripped) = line.strip_suffix("\r\n") {
        (stripped, "\r\n")
    } else if let Some(stripped) = line.strip_suffix('\n') {
        (stripped, "\n")
    } else {
        (line, "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::FileId;

    fn decode_at(kind: StrLitKind, src: &str) -> (String, Vec<Diagnostic>) {
        let span = Span::new(0, src.len() as u32, FileId(0));
        decode(kind, src, span)
    }

    #[test]
    fn plain_hello() {
        let (s, d) = decode_at(StrLitKind::Plain, "\"hello\"");
        assert_eq!(s, "hello");
        assert!(d.is_empty());
    }

    #[test]
    fn plain_empty() {
        let (s, d) = decode_at(StrLitKind::Plain, "\"\"");
        assert_eq!(s, "");
        assert!(d.is_empty());
    }

    #[test]
    fn plain_keeps_backslash_verbatim() {
        // FD-051: `"a\nb"` is verbatim — the `\n` stays two characters.
        let (s, d) = decode_at(StrLitKind::Plain, "\"a\\nb\"");
        assert_eq!(s, "a\\nb");
        assert!(d.is_empty());
    }

    #[test]
    fn plain_windows_path_verbatim() {
        // FD-051 motivating case: `"C:\new"` decodes to the literal path, NOT
        // `C:<LF>ew`.
        let (s, d) = decode_at(StrLitKind::Plain, "\"C:\\new\"");
        assert_eq!(s, "C:\\new");
        assert!(d.is_empty());
    }

    #[test]
    fn escaped_newline_and_tab() {
        let (s, d) = decode_at(StrLitKind::Escaped, "$\"a\\nb\\tc\"");
        assert_eq!(s, "a\nb\tc");
        assert!(d.is_empty());
    }

    #[test]
    fn escaped_backslash_quote() {
        // Source bytes: $ " \ " \ \ "  → decoded `\"\\` is `"\` (one quote, one backslash).
        let (s, d) = decode_at(StrLitKind::Escaped, "$\"\\\"\\\\\"");
        assert_eq!(s, "\"\\");
        assert!(d.is_empty());
    }

    #[test]
    fn escaped_hex_ff() {
        // `\xFF` is the Unicode code point U+00FF.
        let (s, d) = decode_at(StrLitKind::Escaped, "$\"\\xFF\"");
        assert_eq!(s, "\u{FF}");
        assert!(d.is_empty());
    }

    #[test]
    fn escaped_hex_short() {
        // `\x` followed by only one hex digit is invalid.
        let (_s, d) = decode_at(StrLitKind::Escaped, "$\"\\xF\"");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, Some(E_INVALID_ESCAPE));
    }

    #[test]
    fn escaped_quote() {
        // FD-051: a literal `"` inside an escaped string is written `\"`.
        let (s, d) = decode_at(StrLitKind::Escaped, "$\"\\\"\"");
        assert_eq!(s, "\"");
        assert!(d.is_empty());
    }

    #[test]
    fn escaped_non_ascii_char_verbatim() {
        // A non-escape character (here a multi-byte `é`) passes through
        // unchanged inside `$"..."`.
        let (s, d) = decode_at(StrLitKind::Escaped, "$\"é\"");
        assert_eq!(s, "é");
        assert!(d.is_empty());
    }

    #[test]
    fn escaped_unicode_scalar() {
        // U+00E9 = é.
        let (s, d) = decode_at(StrLitKind::Escaped, "$\"\\u00E9\"");
        assert_eq!(s, "é");
        assert!(d.is_empty());
    }

    #[test]
    fn escaped_unicode_surrogate_rejected() {
        // D83D is a high surrogate — not a valid scalar value.
        let (_s, d) = decode_at(StrLitKind::Escaped, "$\"\\uD83D\"");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, Some(E_INVALID_ESCAPE));
    }

    #[test]
    fn escaped_unicode_surrogate_recovers_verbatim() {
        // F-L13: an invalid-scalar `\u` escape now copies the source verbatim
        // (like `\x` invalid-digits and the unknown-escape arm), rather than
        // dropping the escape entirely.
        let (s, d) = decode_at(StrLitKind::Escaped, "$\"\\uD83D\"");
        assert_eq!(s, "\\uD83D");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, Some(E_INVALID_ESCAPE));
    }

    #[test]
    fn escaped_body_offset_with_unterminated_literal() {
        // F-L14: a `$"` token with NO trailing `"` must not strip the (absent)
        // closing quote, so the body offset advances only past `$` (the
        // un-stripped `"` stays in the body) and the escape span still aligns
        // with the `\`. `$"\q` (no closing quote): `\` is at literal byte index 2.
        let lit = "$\"\\q";
        let span = Span::new(0, lit.len() as u32, FileId(0));
        let (s, d) = decode(StrLitKind::Escaped, lit, span);
        // strip_single_quotes returns the post-`$` slice verbatim (no trailing
        // quote), so the leading `"` stays in the body and the unknown escape
        // `\q` recovers to `q` → `"q`.
        assert_eq!(s, "\"q");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, Some(E_INVALID_ESCAPE));
        // `\` is at index 2 (after `$` and `"`); span covers `\q` (2 bytes):
        // 2..4, NOT 3..5.
        assert_eq!(d[0].primary.span.start, 2);
        assert_eq!(d[0].primary.span.end, 4);
    }

    #[test]
    fn escaped_unknown_escape_recovers() {
        // `\q` is unknown — emit E0208 and recover as `q`.
        let (s, d) = decode_at(StrLitKind::Escaped, "$\"a\\qb\"");
        assert_eq!(s, "aqb");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, Some(E_INVALID_ESCAPE));
    }

    #[test]
    fn escaped_diagnostic_span_is_absolute() {
        // Regression: the escape diagnostic must point at the `\` in the *file*,
        // not at an offset relative to the literal. `decode_at` always starts
        // at 0, so a missing base offset was invisible — place the literal at a
        // non-zero start and assert the label span lands on the real `\`.
        let prefix = "xxxxxxxx"; // 8 bytes before the literal
        let lit = "$\"ab\\qcd\""; // `\q` unknown; `\` is at literal byte index 4
        let src = format!("{prefix}{lit}");
        let start = prefix.len() as u32;
        let span = Span::new(start, src.len() as u32, FileId(0));
        let (s, d) = decode(StrLitKind::Escaped, &src, span);
        assert_eq!(s, "abqcd");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, Some(E_INVALID_ESCAPE));
        // `\` at literal index 4 (after `$"ab`) → absolute start+4; the `\q`
        // span is 2 bytes.
        assert_eq!(d[0].primary.span.start, start + 4);
        assert_eq!(d[0].primary.span.end, start + 6);
    }

    #[test]
    fn raw_single_line_of_content() {
        // """\n    hello\n    """ → "hello\n"
        let (s, d) = decode_at(StrLitKind::Raw, "\"\"\"\n    hello\n    \"\"\"");
        assert_eq!(s, "hello\n");
        assert!(d.is_empty(), "got diags: {d:?}");
    }

    #[test]
    fn raw_multiple_content_lines() {
        let (s, d) = decode_at(StrLitKind::Raw, "\"\"\"\n    line1\n    line2\n    \"\"\"");
        assert_eq!(s, "line1\nline2\n");
        assert!(d.is_empty(), "got diags: {d:?}");
    }

    #[test]
    fn raw_content_less_indented_errors() {
        // Closer indent is 4 spaces; content line `  short` has 2.
        let (_s, d) = decode_at(StrLitKind::Raw, "\"\"\"\n    ok\n  short\n    \"\"\"");
        assert!(
            d.iter().any(|x| x.code == Some(E_BAD_RAW_INDENT)),
            "got diags: {d:?}"
        );
    }

    #[test]
    fn raw_close_not_on_own_line_errors() {
        // No newline before closing `"""` → close not on its own line.
        let (_s, d) = decode_at(StrLitKind::Raw, "\"\"\"hello\"\"\"");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, Some(E_BAD_RAW_INDENT));
    }

    #[test]
    fn raw_blank_lines_kept_as_blank() {
        // A blank line between two content lines should become a `\n` in the
        // output — its whitespace (if any) is stripped.
        let (s, d) = decode_at(StrLitKind::Raw, "\"\"\"\n    a\n\n    b\n    \"\"\"");
        assert!(d.is_empty(), "got diags: {d:?}");
        assert_eq!(s, "a\n\nb\n");
    }
}
