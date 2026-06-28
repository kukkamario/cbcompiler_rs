//! CoolBasic lexer. Spec: `docs/cb_syntax.md` §1.
//!
//! Single entry point: [`tokenize`]. The lexer never aborts — errors become
//! [`TokenKind::Error`] variants with structured [`Diagnostic`]s pushed to the
//! returned `Vec<Diagnostic>`.

use cb_diagnostics::{Diagnostic, DiagnosticCode, Label};

use crate::keywords;
use crate::span::{FileId, Span};
use crate::token::{
    CommentKind, FloatBits, LexErrorKind, Op, Punct, Sigil, StrLitKind, Token, TokenKind,
};

/// Options controlling lexer behaviour.
#[derive(Copy, Clone, Debug, Default)]
pub struct LexerOptions {
    /// When true, emit `Whitespace`, `Comment`, and `Continuation` tokens.
    /// When false, those are scanned but discarded.
    pub preserve_trivia: bool,
}

/// Lex `src` into a token stream and a (possibly empty) list of diagnostics.
///
/// The lexer never aborts: lexical errors become [`TokenKind::Error`] tokens
/// paired with structured [`Diagnostic`]s. A terminal [`TokenKind::Eof`] token
/// with a zero-length span is always appended.
pub fn tokenize(src: &str, file: FileId, opts: LexerOptions) -> (Vec<Token>, Vec<Diagnostic>) {
    debug_assert!(
        src.len() <= u32::MAX as usize,
        "source too large for u32 offsets"
    );
    let mut lex = Lexer::new(src, file, opts);
    lex.run();
    (lex.tokens, lex.diagnostics)
}

// Error codes (E01xx — lexer). E0104 was previously `NumberOverflow` and is
// retired: range-checking against the inferred signed target type now lives
// in sema; the lexer only flags values no type could represent and emits
// `MalformedNumber` (E0107) for both shape and out-of-`u64`-range cases.
const E_NEWLINE_IN_STRING: DiagnosticCode = DiagnosticCode::new("E0101");
const E_UNTERMINATED_STRING: DiagnosticCode = DiagnosticCode::new("E0102");
const E_UNTERMINATED_BLOCK_COMMENT: DiagnosticCode = DiagnosticCode::new("E0103");
const E_INVALID_DIGIT_SEPARATOR: DiagnosticCode = DiagnosticCode::new("E0105");
const E_UNEXPECTED_CHAR: DiagnosticCode = DiagnosticCode::new("E0106");
const E_MALFORMED_NUMBER: DiagnosticCode = DiagnosticCode::new("E0107");

/// Size of the on-stack scratch buffer used to strip digit separators (`_`)
/// before parsing. 64 bytes comfortably holds any literal whose magnitude
/// fits the `u64` the lexer parses into; longer runs yield [`Overflow`].
const DIGIT_SCRATCH: usize = 64;

/// Marker returned by [`Lexer::append_stripped`] when a digit run exceeds
/// [`DIGIT_SCRATCH`]; the caller turns it into a malformed-number diagnostic.
struct Overflow;

struct Lexer<'src> {
    src: &'src str,
    bytes: &'src [u8],
    pos: u32,
    file: FileId,
    opts: LexerOptions,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'src> Lexer<'src> {
    fn new(src: &'src str, file: FileId, opts: LexerOptions) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            file,
            opts,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    // ---------- cursor primitives ----------

    fn at_eof(&self) -> bool {
        (self.pos as usize) >= self.bytes.len()
    }

    fn peek_byte(&self) -> Option<u8> {
        self.bytes.get(self.pos as usize).copied()
    }

    fn peek_byte_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos as usize + offset).copied()
    }

    /// Peek the next char. Requires `pos` to sit on a char boundary (panics
    /// otherwise); the cursor only advances by whole chars/ASCII bytes, so this
    /// holds for every in-tree caller.
    fn peek_char(&self) -> Option<char> {
        self.src[self.pos as usize..].chars().next()
    }

    /// Advance one byte and return it. Caller must have verified the byte is ASCII.
    fn bump_byte(&mut self) -> u8 {
        let b = self.bytes[self.pos as usize];
        self.pos += 1;
        b
    }

    /// Advance by the next char's UTF-8 length and return the char. If the
    /// cursor is at EOF, this is a no-op and returns `'\0'` — every in-tree
    /// caller has an explicit peek-guard, so the EOF arm is unreachable on
    /// valid input. The `debug_assert!` surfaces the bug in debug builds; the
    /// saturating release-mode behaviour preserves the lexer's "never aborts"
    /// contract even if a future byte-level guard ever desynchronises.
    fn bump_char(&mut self) -> char {
        debug_assert!(!self.at_eof(), "bump_char at EOF");
        let rest = &self.src[self.pos as usize..];
        match rest.chars().next() {
            Some(c) => {
                self.pos += c.len_utf8() as u32;
                c
            }
            None => '\0',
        }
    }

    fn eat_while_byte(&mut self, mut pred: impl FnMut(u8) -> bool) {
        while let Some(b) = self.peek_byte() {
            if pred(b) {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn current_span(&self, start: u32) -> Span {
        Span::new(start, self.pos, self.file)
    }

    fn push_token(&mut self, kind: TokenKind, span: Span) {
        self.tokens.push(Token { kind, span });
    }

    // ---------- drive loop ----------

    fn run(&mut self) {
        // Optional UTF-8 BOM.
        if self.src.starts_with('\u{FEFF}') {
            self.pos += '\u{FEFF}'.len_utf8() as u32;
        }

        while !self.at_eof() {
            self.scan_one();
        }

        // Terminal Eof with zero-length span at current position.
        let eof_span = Span::new(self.pos, self.pos, self.file);
        self.push_token(TokenKind::Eof, eof_span);
    }

    fn scan_one(&mut self) {
        let start = self.pos;
        let b = match self.peek_byte() {
            Some(b) => b,
            None => return,
        };

        match b {
            b' ' | b'\t' => self.scan_whitespace(start),
            b'\n' | b'\r' => self.scan_newline(start),
            b'\\' => self.scan_continuation_or_backslash(start),
            b'\'' => self.scan_line_comment_tick(start),
            b'/' => self.scan_slash_or_comment(start),
            b'"' => {
                if self.peek_byte_at(1) == Some(b'"') && self.peek_byte_at(2) == Some(b'"') {
                    self.scan_raw_string(start);
                } else {
                    self.scan_string(start);
                }
            }
            b'$' => {
                if self.peek_byte_at(1) == Some(b'"') {
                    // `$"` opens an escape-aware string literal. `"` is
                    // not a hex digit, so no valid hex token is shadowed, and a
                    // token-start `$` is never a String sigil (sigils are only
                    // consumed as a trailing byte inside `scan_ident`).
                    self.scan_dollar_string(start);
                } else {
                    self.scan_radix_number(
                        start,
                        b'$',
                        |b| b.is_ascii_hexdigit(),
                        16,
                        "hexadecimal",
                    )
                }
            }
            b'%' => self.scan_radix_number(start, b'%', |b| matches!(b, b'0' | b'1'), 2, "binary"),
            b'0'..=b'9' => self.scan_number_decimal_or_float(start),
            // Operators and punctuation handled in a dedicated path.
            b'+' | b'-' | b'*' | b'^' | b'=' | b'<' | b'>' | b'(' | b')' | b'[' | b']' | b','
            | b':' | b';' | b'.' => self.scan_operator_or_punct(start),
            _ => {
                // Possibly an identifier (XID_Start or `_`), otherwise invalid.
                if b == b'_' || (b.is_ascii_alphabetic()) {
                    self.scan_ident(start);
                } else if b < 0x80 {
                    // ASCII byte that doesn't begin any token.
                    self.bump_byte();
                    let span = self.current_span(start);
                    self.push_token(TokenKind::Error(LexErrorKind::UnexpectedChar), span);
                    self.diagnostics.push(Diagnostic::error(
                        E_UNEXPECTED_CHAR,
                        format!("unexpected character `{}`", b as char),
                        Label::new(span),
                    ));
                } else {
                    // Non-ASCII byte: decode as a char and check XID_Start.
                    if let Some(c) = self.peek_char() {
                        if c == '_' || unicode_ident::is_xid_start(c) {
                            self.scan_ident(start);
                        } else {
                            self.bump_char();
                            let span = self.current_span(start);
                            self.push_token(TokenKind::Error(LexErrorKind::UnexpectedChar), span);
                            self.diagnostics.push(Diagnostic::error(
                                E_UNEXPECTED_CHAR,
                                format!("unexpected character `{}`", c),
                                Label::new(span),
                            ));
                        }
                    } else {
                        // Should be impossible: src is valid UTF-8 and we're
                        // not at EOF. Use `bump_char` (saturating) rather than
                        // `self.pos += 1` so a future regression doesn't
                        // desynchronise the cursor from a multi-byte boundary
                        // and break `&str` slicing in `peek_char`.
                        self.bump_char();
                    }
                }
            }
        }
    }

    // ---------- trivia ----------

    fn scan_whitespace(&mut self, start: u32) {
        self.eat_while_byte(|b| b == b' ' || b == b'\t');
        if self.opts.preserve_trivia {
            let span = self.current_span(start);
            self.push_token(TokenKind::Whitespace, span);
        }
    }

    /// Consume one line terminator (`\r`, `\n`, or `\r\n`) if present.
    fn consume_line_terminator(&mut self) {
        match self.peek_byte() {
            Some(b'\r') => {
                self.bump_byte();
                if self.peek_byte() == Some(b'\n') {
                    self.bump_byte();
                }
            }
            Some(b'\n') => {
                self.bump_byte();
            }
            _ => {}
        }
    }

    fn scan_newline(&mut self, start: u32) {
        self.consume_line_terminator();
        let span = self.current_span(start);
        self.push_token(TokenKind::Newline, span);
    }

    fn scan_continuation_or_backslash(&mut self, start: u32) {
        // Save state, look ahead for `\` ws* (\r|\n|\r\n).
        debug_assert_eq!(self.peek_byte(), Some(b'\\'));
        let save = self.pos;
        self.bump_byte(); // consume '\\'
        // Tentatively eat spaces/tabs.
        self.eat_while_byte(|b| b == b' ' || b == b'\t');
        let is_continuation = matches!(self.peek_byte(), Some(b'\n') | Some(b'\r'));
        if is_continuation {
            // Consume the line terminator.
            self.consume_line_terminator();
            if self.opts.preserve_trivia {
                let span = self.current_span(start);
                self.push_token(TokenKind::Continuation, span);
            }
        } else {
            // Not a continuation: rewind past the whitespace we tentatively ate
            // and emit a lone `\` as Op::BackSlash. The whitespace will be
            // re-scanned on the next iteration as a separate token.
            self.pos = save + 1;
            let span = Span::new(save, save + 1, self.file);
            self.push_token(TokenKind::Op(Op::BackSlash), span);
        }
    }

    fn scan_slash_or_comment(&mut self, start: u32) {
        debug_assert_eq!(self.peek_byte(), Some(b'/'));
        match self.peek_byte_at(1) {
            Some(b'/') => self.scan_line_comment_slashes(start),
            Some(b'*') => self.scan_block_comment(start),
            _ => {
                self.bump_byte();
                let span = self.current_span(start);
                self.push_token(TokenKind::Op(Op::Slash), span);
            }
        }
    }

    fn scan_line_comment_slashes(&mut self, start: u32) {
        // Consume the two slashes.
        self.bump_byte();
        self.bump_byte();
        // Consume up to (but not including) the line terminator.
        self.eat_while_byte(|b| b != b'\n' && b != b'\r');
        if self.opts.preserve_trivia {
            let span = self.current_span(start);
            self.push_token(TokenKind::Comment(CommentKind::Line), span);
        }
    }

    /// Scan a `'`-introduced line comment (classic BASIC / CoolBasic style).
    /// Consumes the apostrophe and everything up to (not including) the line
    /// terminator, mirroring `//` and `Rem`.
    fn scan_line_comment_tick(&mut self, start: u32) {
        debug_assert_eq!(self.peek_byte(), Some(b'\''));
        self.bump_byte(); // consume the `'`
        self.eat_while_byte(|b| b != b'\n' && b != b'\r');
        if self.opts.preserve_trivia {
            let span = self.current_span(start);
            self.push_token(TokenKind::Comment(CommentKind::Line), span);
        }
    }

    /// Used when an identifier body equals "rem" (any case). The `start`
    /// argument is the offset of the `R`; positioning is already at the byte
    /// after the ident body.
    fn finish_rem_comment(&mut self, start: u32) {
        self.eat_while_byte(|b| b != b'\n' && b != b'\r');
        if self.opts.preserve_trivia {
            let span = self.current_span(start);
            self.push_token(TokenKind::Comment(CommentKind::Line), span);
        }
    }

    fn scan_block_comment(&mut self, start: u32) {
        // Consume opening `/*`.
        self.bump_byte();
        self.bump_byte();
        let mut depth: u32 = 1;
        while depth > 0 {
            match (self.peek_byte(), self.peek_byte_at(1)) {
                (Some(b'/'), Some(b'*')) => {
                    self.bump_byte();
                    self.bump_byte();
                    depth += 1;
                }
                (Some(b'*'), Some(b'/')) => {
                    self.bump_byte();
                    self.bump_byte();
                    depth -= 1;
                }
                (Some(_), _) => {
                    // Advance by one char to stay UTF-8 safe.
                    self.bump_char();
                }
                (None, _) => {
                    // EOF inside block comment. Primary label spans the full
                    // consumed region so the user sees every byte that got
                    // swallowed; the opener is kept as a secondary anchor.
                    let opener = Span::new(start, start + 2, self.file);
                    let span = self.current_span(start);
                    self.diagnostics.push(
                        Diagnostic::error(
                            E_UNTERMINATED_BLOCK_COMMENT,
                            "unterminated block comment",
                            Label::with_message(span, "reached end of file inside block comment"),
                        )
                        .with_secondary(Label::with_message(opener, "block comment opened here")),
                    );
                    self.push_token(
                        TokenKind::Error(LexErrorKind::UnterminatedBlockComment),
                        span,
                    );
                    return;
                }
            }
        }
        if self.opts.preserve_trivia {
            let span = self.current_span(start);
            self.push_token(TokenKind::Comment(CommentKind::Block), span);
        }
    }

    // ---------- strings ----------

    /// Verbatim `"…"` literal. `\` is an ordinary character and the
    /// first unescaped `"` always closes the literal — there is no escape
    /// processing here. The escape-aware form is `$"…"` (see
    /// [`scan_dollar_string`]).
    fn scan_string(&mut self, start: u32) {
        debug_assert_eq!(self.peek_byte(), Some(b'"'));
        self.bump_byte(); // opening quote
        loop {
            match self.peek_byte() {
                Some(b'"') => {
                    self.bump_byte();
                    let span = self.current_span(start);
                    self.push_token(TokenKind::StrLit(StrLitKind::Plain), span);
                    return;
                }
                Some(b'\n') | Some(b'\r') => {
                    // Newline inside single-line string: error, do NOT consume the newline.
                    let span = self.current_span(start);
                    self.diagnostics.push(Diagnostic::error(
                        E_NEWLINE_IN_STRING,
                        "newline in string literal — use a triple-quoted `\"\"\"...\"\"\"` raw string, or `$\"\\n\"` for an escaped newline",
                        Label::new(span),
                    ));
                    self.push_token(TokenKind::Error(LexErrorKind::NewlineInString), span);
                    return;
                }
                Some(_) => {
                    self.bump_char();
                }
                None => {
                    let span = self.current_span(start);
                    self.diagnostics.push(Diagnostic::error(
                        E_UNTERMINATED_STRING,
                        "unterminated string literal",
                        Label::new(span),
                    ));
                    self.push_token(TokenKind::Error(LexErrorKind::UnterminatedString), span);
                    return;
                }
            }
        }
    }

    /// Escape-aware `$"…"` literal. The `$` is a mode marker only, not
    /// interpolation. Inside the quotes, `\` escapes the next character (so
    /// `\"` does not terminate the literal); the actual escape set is validated
    /// later by the decoder. The classification here only sets `Escaped`.
    fn scan_dollar_string(&mut self, start: u32) {
        debug_assert_eq!(self.peek_byte(), Some(b'$'));
        debug_assert_eq!(self.peek_byte_at(1), Some(b'"'));
        self.bump_byte(); // `$`
        self.bump_byte(); // opening quote
        loop {
            match self.peek_byte() {
                Some(b'\\') => {
                    self.bump_byte();
                    // Consume the next char of the escape (any char), if any.
                    // Don't validate; that's the decoder's job. But stop before
                    // newline so a stray `\` doesn't swallow it.
                    match self.peek_byte() {
                        Some(b'\n') | Some(b'\r') => {
                            // Treat as backslash-followed-by-newline; do not
                            // consume. Fall through to the post-loop diagnostic
                            // (distinct message from plain newline-in-string).
                            break;
                        }
                        Some(_) => {
                            self.bump_char();
                        }
                        None => {}
                    }
                }
                Some(b'"') => {
                    self.bump_byte();
                    let span = self.current_span(start);
                    self.push_token(TokenKind::StrLit(StrLitKind::Escaped), span);
                    return;
                }
                Some(b'\n') | Some(b'\r') => {
                    // Newline inside single-line string: error, do NOT consume the newline.
                    let span = self.current_span(start);
                    self.diagnostics.push(Diagnostic::error(
                        E_NEWLINE_IN_STRING,
                        "newline in string literal — use `\\n` or a triple-quoted `\"\"\"...\"\"\"` raw string",
                        Label::new(span),
                    ));
                    self.push_token(TokenKind::Error(LexErrorKind::NewlineInString), span);
                    return;
                }
                Some(_) => {
                    self.bump_char();
                }
                None => {
                    let span = self.current_span(start);
                    self.diagnostics.push(Diagnostic::error(
                        E_UNTERMINATED_STRING,
                        "unterminated string literal",
                        Label::new(span),
                    ));
                    self.push_token(TokenKind::Error(LexErrorKind::UnterminatedString), span);
                    return;
                }
            }
        }
        // Fell out via backslash-immediately-before-newline. (The other
        // newline-in-string path returns inside the loop with its own,
        // distinct diagnostic message.)
        let span = self.current_span(start);
        self.diagnostics.push(Diagnostic::error(
            E_NEWLINE_IN_STRING,
            "backslash followed by newline inside string literal — use `\\n` for a newline, or a triple-quoted `\"\"\"...\"\"\"` raw string for multi-line content",
            Label::new(span),
        ));
        self.push_token(TokenKind::Error(LexErrorKind::NewlineInString), span);
    }

    fn scan_raw_string(&mut self, start: u32) {
        // Consume the three opening quotes.
        self.bump_byte();
        self.bump_byte();
        self.bump_byte();
        loop {
            // Look for three consecutive `"`s.
            if self.peek_byte() == Some(b'"')
                && self.peek_byte_at(1) == Some(b'"')
                && self.peek_byte_at(2) == Some(b'"')
            {
                self.bump_byte();
                self.bump_byte();
                self.bump_byte();
                let span = self.current_span(start);
                self.push_token(TokenKind::StrLit(StrLitKind::Raw), span);
                return;
            }
            if self.at_eof() {
                // Primary label on the opener (where the literal began);
                // secondary label at EOF tells the user where scanning gave
                // up. The token span still covers the full consumed region
                // so the parser advances past it.
                let opener = Span::new(start, start + 3, self.file);
                let eof_span = Span::new(self.pos, self.pos, self.file);
                let span = self.current_span(start);
                self.diagnostics.push(
                    Diagnostic::error(
                        E_UNTERMINATED_STRING,
                        "unterminated raw string literal",
                        Label::with_message(opener, "raw string opened here"),
                    )
                    .with_secondary(Label::with_message(
                        eof_span,
                        "reached end of file without `\"\"\"` closer",
                    )),
                );
                self.push_token(TokenKind::Error(LexErrorKind::UnterminatedString), span);
                return;
            }
            self.bump_char();
        }
    }

    // ---------- numbers ----------

    /// Scan a digit run with underscore separators for the given radix.
    /// Validates separator placement, pushing diagnostics for each violation.
    /// Returns the byte range `[run_start, run_end)` for the digit run and a
    /// flag indicating whether any separator error was emitted.
    ///
    /// `is_digit` is the predicate for valid digits (not including `_`).
    /// The lexer cursor must be positioned at the start of the digit run.
    /// The cursor advances past the entire run (digits + underscores).
    fn scan_digit_run(
        &mut self,
        is_digit: fn(u8) -> bool,
        radix_label: &'static str,
    ) -> (u32, u32, bool) {
        self.scan_digit_run_inner(is_digit, radix_label, false)
    }

    /// Variant of [`scan_digit_run`] that skips the "leading underscore"
    /// diagnostic. Used by hex/binary scanners which already reported the
    /// bad prefix-adjacent `_` themselves and pre-consumed it.
    fn scan_digit_run_no_leading_diag(
        &mut self,
        is_digit: fn(u8) -> bool,
        radix_label: &'static str,
    ) -> (u32, u32, bool) {
        self.scan_digit_run_inner(is_digit, radix_label, true)
    }

    fn scan_digit_run_inner(
        &mut self,
        is_digit: fn(u8) -> bool,
        radix_label: &'static str,
        suppress_leading_diag: bool,
    ) -> (u32, u32, bool) {
        let run_start = self.pos;
        // When the caller pre-consumed an offending leading `_` and diagnosed
        // it, treat the state as if we just saw an underscore so a second
        // adjacent `_` is not re-diagnosed as "doubled".
        let mut last_was_underscore = suppress_leading_diag;
        // In suppress mode the caller's already-diagnosed leading `_` is itself
        // a bad separator, so the run is bad from the start — seed `bad_sep`.
        let mut bad_sep = suppress_leading_diag;
        // Leading underscore check (only when not suppressed; in suppress mode
        // the caller already handled the bad leading `_`).
        if !suppress_leading_diag && self.peek_byte() == Some(b'_') {
            // Leading underscore — invalid. Will report on first underscore.
            let bad_span = Span::new(self.pos, self.pos + 1, self.file);
            self.diagnostics.push(Diagnostic::error(
                E_INVALID_DIGIT_SEPARATOR,
                format!("digit separator cannot appear before any {radix_label} digit"),
                Label::new(bad_span),
            ));
            bad_sep = true;
        }
        while let Some(b) = self.peek_byte() {
            if is_digit(b) {
                self.bump_byte();
                last_was_underscore = false;
            } else if b == b'_' {
                if last_was_underscore {
                    let bad_span = Span::new(self.pos, self.pos + 1, self.file);
                    self.diagnostics.push(Diagnostic::error(
                        E_INVALID_DIGIT_SEPARATOR,
                        "digit separators cannot be doubled",
                        Label::new(bad_span),
                    ));
                    bad_sep = true;
                }
                self.bump_byte();
                last_was_underscore = true;
            } else {
                break;
            }
        }
        if last_was_underscore && self.pos > run_start {
            // Trailing underscore — but only if this run actually consumed
            // bytes. An empty run in suppress mode (e.g. `$_`) has
            // `last_was_underscore` seeded `true` from the caller's already-
            // diagnosed leading `_`; reporting it again as "trailing" would be a
            // spurious second diagnostic for the same separator.
            let bad_span = Span::new(self.pos - 1, self.pos, self.file);
            self.diagnostics.push(Diagnostic::error(
                E_INVALID_DIGIT_SEPARATOR,
                "digit separator cannot trail a number",
                Label::new(bad_span),
            ));
            bad_sep = true;
        }
        (run_start, self.pos, bad_sep)
    }

    /// Append `src` into `buf` starting at `*n`, skipping `_` separators.
    /// Returns `Err(Overflow)` if the digit run doesn't fit; the caller then
    /// reports a malformed-number diagnostic. Shared by the float
    /// int/frac/exponent sub-parts and by `strip_underscores`.
    fn append_stripped(
        buf: &mut [u8; DIGIT_SCRATCH],
        n: &mut usize,
        src: &[u8],
    ) -> Result<(), Overflow> {
        for &b in src {
            if b == b'_' {
                continue;
            }
            if *n >= buf.len() {
                return Err(Overflow);
            }
            buf[*n] = b;
            *n += 1;
        }
        Ok(())
    }

    /// Strip underscores from `src_bytes` into `scratch`, returning the slice
    /// of `scratch` actually filled. If the digit run doesn't fit, returns
    /// `None` (signal for overflow handling).
    fn strip_underscores<'a>(
        src_bytes: &[u8],
        scratch: &'a mut [u8; DIGIT_SCRATCH],
    ) -> Option<&'a str> {
        let mut n = 0usize;
        Self::append_stripped(scratch, &mut n, src_bytes).ok()?;
        // Safe — only ASCII digits/letters reach scratch.
        std::str::from_utf8(&scratch[..n]).ok()
    }

    fn scan_number_decimal_or_float(&mut self, start: u32) {
        debug_assert!(matches!(self.peek_byte(), Some(b'0'..=b'9')));
        // Integer-part digit run.
        let (int_lo, int_hi, mut bad_sep) = self.scan_digit_run(|b| b.is_ascii_digit(), "decimal");

        // Detect float continuation.
        let mut is_float = false;
        let mut frac_lo = int_hi;
        let mut frac_hi = int_hi;
        // `exp_marker_lo` is the offset of the `e`/`E`; captured during the
        // forward scan so the float rebuild can slice `e[sign]<digits>` directly
        // instead of back-scanning to relocate the marker. Only read when
        // `exp_hi > exp_lo` (i.e. a complete exponent was scanned).
        let mut exp_marker_lo = int_hi;
        let mut exp_lo = int_hi;
        let mut exp_hi = int_hi;

        if self.peek_byte() == Some(b'.')
            && self
                .peek_byte_at(1)
                .map(|b| b.is_ascii_digit())
                .unwrap_or(false)
        {
            is_float = true;
            self.bump_byte(); // '.'
            let (lo, hi, bad) = self.scan_digit_run(|b| b.is_ascii_digit(), "decimal");
            frac_lo = lo;
            frac_hi = hi;
            bad_sep |= bad;
        }
        if matches!(self.peek_byte(), Some(b'e') | Some(b'E')) {
            is_float = true;
            exp_marker_lo = self.pos; // offset of the `e`/`E`, before consuming it
            self.bump_byte(); // 'e'/'E'
            // Optional sign.
            if matches!(self.peek_byte(), Some(b'+') | Some(b'-')) {
                self.bump_byte();
            }
            // Exponent digits (must have at least one).
            let exp_run_start = self.pos;
            let (lo, hi, bad) = self.scan_digit_run(|b| b.is_ascii_digit(), "decimal");
            exp_lo = lo;
            exp_hi = hi;
            bad_sep |= bad;
            if hi == exp_run_start {
                // No digits after `e` — malformed numeric literal. Emit
                // `MalformedNumber` (E0107) covering the whole partial literal.
                let span = self.current_span(start);
                self.diagnostics.push(Diagnostic::error(
                    E_MALFORMED_NUMBER,
                    "missing digits after exponent",
                    Label::new(span),
                ));
                self.push_token(TokenKind::Error(LexErrorKind::MalformedNumber), span);
                return;
            }
        }

        let span = self.current_span(start);
        if bad_sep {
            self.push_token(TokenKind::Error(LexErrorKind::InvalidDigitSeparator), span);
            return;
        }

        let mut scratch = [0u8; DIGIT_SCRATCH];
        if is_float {
            // Build "<int>.<frac>[e[sign]<exp>]" stripped of underscores.
            let mut buf = [0u8; DIGIT_SCRATCH];
            let mut n = 0usize;
            // int part
            if Self::append_stripped(
                &mut buf,
                &mut n,
                &self.bytes[int_lo as usize..int_hi as usize],
            )
            .is_err()
            {
                self.report_malformed_number(span);
                return;
            }
            if frac_hi > frac_lo {
                if n >= buf.len() {
                    self.report_malformed_number(span);
                    return;
                }
                buf[n] = b'.';
                n += 1;
                if Self::append_stripped(
                    &mut buf,
                    &mut n,
                    &self.bytes[frac_lo as usize..frac_hi as usize],
                )
                .is_err()
                {
                    self.report_malformed_number(span);
                    return;
                }
            }
            if exp_hi > exp_lo {
                // `f64::from_str` needs the original `e`/`E` and optional sign.
                // The exponent's source bytes are contiguous from the marker
                // captured during the forward scan (`e[sign]<digits>`), so slice
                // straight from there — no back-scan needed. Underscores in the
                // digit run are stripped by `append_stripped`.
                if Self::append_stripped(
                    &mut buf,
                    &mut n,
                    &self.bytes[exp_marker_lo as usize..exp_hi as usize],
                )
                .is_err()
                {
                    self.report_malformed_number(span);
                    return;
                }
            }
            let s = match std::str::from_utf8(&buf[..n]) {
                Ok(s) => s,
                Err(_) => {
                    self.report_malformed_number(span);
                    return;
                }
            };
            match s.parse::<f64>() {
                Ok(v) if v.is_finite() => {
                    self.push_token(TokenKind::FloatLit(FloatBits::from_f64(v)), span);
                }
                _ => {
                    self.report_malformed_number(span);
                }
            }
        } else {
            // Integer. Parse as `u64` — the lexer never commits to a signed
            // representation. Range-checking against the inferred signed
            // target type is sema's job (`cb_syntax.md` §3.4).
            let raw = &self.bytes[int_lo as usize..int_hi as usize];
            let stripped = match Self::strip_underscores(raw, &mut scratch) {
                Some(s) => s,
                None => {
                    self.report_malformed_number(span);
                    return;
                }
            };
            match stripped.parse::<u64>() {
                Ok(v) => self.push_token(TokenKind::IntLit(v), span),
                Err(_) => self.report_malformed_number(span),
            }
        }
    }

    /// Scan a radix-prefixed integer literal (`$..` hex, `%..` binary). The two
    /// forms differ only in the prefix byte, digit predicate, parse radix, and
    /// the noun used in diagnostics; everything else — leading-`_` pre-check,
    /// empty-run handling, separator validation, and overflow — is shared.
    fn scan_radix_number(
        &mut self,
        start: u32,
        prefix: u8,
        is_digit: fn(u8) -> bool,
        radix: u32,
        label: &'static str,
    ) {
        let prefix_char = prefix as char;
        debug_assert_eq!(self.peek_byte(), Some(prefix));
        self.bump_byte(); // prefix
        // Must be followed by a digit; if next is '_' or absent/non-digit, report.
        let mut pre_consumed_underscore = false;
        match self.peek_byte() {
            Some(b) if is_digit(b) => {}
            Some(b'_') => {
                let bad_span = Span::new(self.pos, self.pos + 1, self.file);
                self.diagnostics.push(Diagnostic::error(
                    E_INVALID_DIGIT_SEPARATOR,
                    format!("digit separator cannot appear before any {label} digit"),
                    Label::new(bad_span),
                ));
                // Pre-consume the offending `_` so `scan_digit_run` does not
                // re-diagnose the same byte as a leading separator.
                self.bump_byte();
                pre_consumed_underscore = true;
            }
            _ => {
                let span = self.current_span(start);
                self.diagnostics.push(Diagnostic::error(
                    E_UNEXPECTED_CHAR,
                    format!("expected {label} digits after `{prefix_char}`"),
                    Label::new(span),
                ));
                self.push_token(TokenKind::Error(LexErrorKind::UnexpectedChar), span);
                return;
            }
        }
        let (run_lo, run_hi, bad_sep_from_run) = if pre_consumed_underscore {
            self.scan_digit_run_no_leading_diag(is_digit, label)
        } else {
            self.scan_digit_run(is_digit, label)
        };
        let span = self.current_span(start);
        let raw = &self.bytes[run_lo as usize..run_hi as usize];
        if raw.is_empty() {
            // `$_` / `%_` form — a leading separator with no digits. The
            // pre-check already emitted the single primary diagnostic (E0105,
            // `InvalidDigitSeparator`), consistent with `$_ff`; just push the
            // matching error token here (no second diagnostic).
            self.push_token(TokenKind::Error(LexErrorKind::InvalidDigitSeparator), span);
            return;
        }
        // `scan_digit_run` already diagnosed (E0105) and flagged any
        // leading/doubled/trailing separator in this run via `bad_sep_from_run`
        // (the pre-check handles the pre-consumed leading `_`). Trust that flag.
        if bad_sep_from_run {
            self.push_token(TokenKind::Error(LexErrorKind::InvalidDigitSeparator), span);
            return;
        }
        let mut scratch = [0u8; DIGIT_SCRATCH];
        let stripped = match Self::strip_underscores(raw, &mut scratch) {
            Some(s) => s,
            None => {
                self.report_malformed_number(span);
                return;
            }
        };
        match u64::from_str_radix(stripped, radix) {
            Ok(v) => self.push_token(TokenKind::IntLit(v), span),
            Err(_) => self.report_malformed_number(span),
        }
    }

    /// Emit `MalformedNumber` (E0107) — used for both structurally malformed
    /// literals (e.g. `1e` with no exponent digits) and values that exceed
    /// the lexer's representable range (`u64` for integers, finite `f64` for
    /// floats). Range-checking against the inferred signed target type is
    /// sema's job, not the lexer's.
    fn report_malformed_number(&mut self, span: Span) {
        self.diagnostics.push(Diagnostic::error(
            E_MALFORMED_NUMBER,
            "numeric literal is malformed or out of representable range",
            Label::new(span),
        ));
        self.push_token(TokenKind::Error(LexErrorKind::MalformedNumber), span);
    }

    // ---------- identifiers / keywords / REM ----------

    fn scan_ident(&mut self, start: u32) {
        // Consume first char (already verified by caller).
        let first = self.bump_char();
        debug_assert!(first == '_' || unicode_ident::is_xid_start(first));

        // Consume body chars.
        while let Some(c) = self.peek_char() {
            if c == '_' || unicode_ident::is_xid_continue(c) {
                self.bump_char();
            } else {
                break;
            }
        }
        let body_end = self.pos;
        let body = &self.src[start as usize..body_end as usize];

        // REM pivot — case-insensitive ASCII match.
        if body.eq_ignore_ascii_case("rem") {
            self.finish_rem_comment(start);
            return;
        }

        // Keyword lookup (on bare body, before any sigil).
        if let Some(kw) = keywords::lookup(body) {
            let span = Span::new(start, body_end, self.file);
            self.push_token(TokenKind::Keyword(kw), span);
            return;
        }

        // Optional trailing sigil.
        let sigil = match self.peek_byte() {
            Some(b'%') => Some(Sigil::Integer),
            Some(b'#') => Some(Sigil::Float),
            Some(b'$') => Some(Sigil::String),
            _ => None,
        };
        if sigil.is_some() {
            self.bump_byte();
        }
        let span = self.current_span(start);
        self.push_token(TokenKind::Ident { sigil }, span);
    }

    // ---------- operators / punctuation ----------

    fn scan_operator_or_punct(&mut self, start: u32) {
        let b = self.peek_byte().expect("scan_operator_or_punct: at EOF");
        match b {
            b'+' => self.emit_single(start, TokenKind::Op(Op::Plus)),
            b'-' => self.emit_single(start, TokenKind::Op(Op::Minus)),
            b'*' => self.emit_single(start, TokenKind::Op(Op::Star)),
            b'^' => self.emit_single(start, TokenKind::Op(Op::Caret)),
            b'=' => self.emit_single(start, TokenKind::Op(Op::Eq)),
            b'<' => {
                self.bump_byte();
                match self.peek_byte() {
                    Some(b'=') => {
                        self.bump_byte();
                        let span = self.current_span(start);
                        self.push_token(TokenKind::Op(Op::LtEq), span);
                    }
                    Some(b'>') => {
                        self.bump_byte();
                        let span = self.current_span(start);
                        self.push_token(TokenKind::Op(Op::NotEq), span);
                    }
                    _ => {
                        let span = self.current_span(start);
                        self.push_token(TokenKind::Op(Op::Lt), span);
                    }
                }
            }
            b'>' => {
                self.bump_byte();
                if self.peek_byte() == Some(b'=') {
                    self.bump_byte();
                    let span = self.current_span(start);
                    self.push_token(TokenKind::Op(Op::GtEq), span);
                } else {
                    let span = self.current_span(start);
                    self.push_token(TokenKind::Op(Op::Gt), span);
                }
            }
            b'(' => self.emit_single(start, TokenKind::Punct(Punct::LParen)),
            b')' => self.emit_single(start, TokenKind::Punct(Punct::RParen)),
            b'[' => self.emit_single(start, TokenKind::Punct(Punct::LBracket)),
            b']' => self.emit_single(start, TokenKind::Punct(Punct::RBracket)),
            b',' => self.emit_single(start, TokenKind::Punct(Punct::Comma)),
            b':' => self.emit_single(start, TokenKind::Punct(Punct::Colon)),
            b';' => self.emit_single(start, TokenKind::Punct(Punct::Semicolon)),
            b'.' => self.emit_single(start, TokenKind::Punct(Punct::Dot)),
            _ => {
                // Shouldn't reach: dispatcher only routes the bytes above here.
                self.bump_byte();
                let span = self.current_span(start);
                self.diagnostics.push(Diagnostic::error(
                    E_UNEXPECTED_CHAR,
                    format!("unexpected character `{}`", b as char),
                    Label::new(span),
                ));
                self.push_token(TokenKind::Error(LexErrorKind::UnexpectedChar), span);
            }
        }
    }

    fn emit_single(&mut self, start: u32, kind: TokenKind) {
        self.bump_byte();
        let span = self.current_span(start);
        self.push_token(kind, span);
    }
}
