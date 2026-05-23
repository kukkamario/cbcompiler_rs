//! CoolBasic parser. Spec: `docs/cb_syntax.md` §2 and FD-002.
//!
//! Top-down recursive-descent parser with a Pratt loop for expressions; see
//! FD-002 for the architectural rationale. The parser is recovering — every
//! [`ParseError`] is captured at the nearest statement boundary, turned into
//! an `Stmt::Error` node, and the cursor resynchronises past the next sync
//! token (see [`Parser::sync_to_stmt_boundary`]). Expression-level recovery
//! is currently coarse — errors bubble to statement boundary. Refine in a
//! future FD when concrete cases motivate it.

use cb_diagnostics::{Diagnostic, Label};

use crate::ast::{
    Arena, BinOp, CaseArm, DimName, ElseIf, Expr, IfForm, NewKind, Node, NodeId, Param, Stmt,
    TypeExpr, UnOp,
};
use crate::span::{FileId, Span, SpanExt};
use crate::string_value;
use crate::token::{Kw, Op, Punct, Sigil, Token, TokenKind};

// Error codes (E02xx — parser).
pub const E_EXPECTED_TOKEN: &str = "E0201";
pub const E_UNEXPECTED_TOKEN: &str = "E0202";
pub const E_UNTERMINATED_BLOCK: &str = "E0203";
pub const E_MISMATCHED_END_KEYWORD: &str = "E0204";
pub const E_INVALID_TYPE_EXPR: &str = "E0205";
pub const E_BAD_STATEMENT: &str = "E0206";
pub const E_RESERVED_WORD_AS_NAME: &str = "E0207";
pub const E_INVALID_ESCAPE: &str = "E0208";
pub const E_BAD_RAW_INDENT: &str = "E0209";
pub const E_MULTI_NAME_NOT_ALLOWED: &str = "E0210";
pub const E_FIELD_OUTSIDE_TYPE_BODY: &str = "E0211";
pub const E_SINGLELINE_IF_DISALLOWS_ELSEIF: &str = "E0212";
pub const E_BREAK_COUNT_NOT_POSITIVE_INT_LITERAL: &str = "E0213";
pub const E_LABEL_HAS_SIGIL: &str = "E0214";
pub const E_EMPTY_SINGLE_LINE_IF_BODY: &str = "E0215";
pub const E_DUPLICATE_DEFAULT: &str = "E0216";
pub const E_NEXT_SIGIL_MISMATCH: &str = "E0217";
/// Internal compiler error — emitted only from defensive branches that
/// should be unreachable by the parser invariants. Surfaced (rather than
/// `panic!`) so a future change that violates the invariants produces a
/// clear failure instead of a hard crash.
pub const E_INTERNAL_PARSER: &str = "E0299";

/// Internal error type that propagates up to `parse_stmt`'s recovery point.
/// Carries a structured diagnostic and the span where parsing failed. The
/// diagnostic is boxed so `Result<NodeId, ParseError>` stays small (one
/// pointer plus a `Span`) across every parser function.
#[derive(Debug)]
pub(crate) struct ParseError {
    pub diag: Box<Diagnostic>,
    pub span: Span,
}

/// Result of parsing: the populated arena, the list of top-level statement
/// node ids (the program), and any diagnostics produced along the way.
pub struct ParseResult {
    pub arena: Arena,
    pub program: Vec<NodeId>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Cursor over a token slice. The lexer guarantees a terminal [`TokenKind::Eof`]
/// token with a zero-length span, so the cursor never needs to invent one when
/// reading the last position; it only synthesises an Eof token when asked to
/// look past the slice.
pub(crate) struct Cursor<'t> {
    tokens: &'t [Token],
    pos: usize,
    src: &'t str,
    file: FileId,
}

impl<'t> Cursor<'t> {
    pub(crate) fn new(tokens: &'t [Token], src: &'t str, file: FileId) -> Self {
        let mut c = Self {
            tokens,
            pos: 0,
            src,
            file,
        };
        c.skip_continuations();
        c
    }

    /// Advance `pos` past any [`TokenKind::Continuation`] tokens at the front
    /// of the lookahead. Per `cb_syntax.md` §1.1 the `\` line-continuation
    /// fuses the surrounding lines unconditionally, so the parser must treat
    /// the token as invisible. Maintained as an invariant: every public
    /// `Cursor` mutation leaves `pos` either past the slice or on a
    /// non-Continuation token. Constructors and [`Cursor::bump`] call this;
    /// [`Cursor::peek_n`] walks past them in its own lookup.
    fn skip_continuations(&mut self) {
        while self.pos < self.tokens.len()
            && matches!(self.tokens[self.pos].kind, TokenKind::Continuation)
        {
            self.pos += 1;
        }
    }

    /// Kind of the token at the cursor. Returns `Eof` past end of stream.
    pub(crate) fn peek(&self) -> TokenKind {
        self.peek_tok().kind
    }

    /// Kind of the token `n` positions ahead, skipping any
    /// [`TokenKind::Continuation`] tokens in between. Returns `Eof` past end
    /// of stream.
    pub(crate) fn peek_n(&self, n: usize) -> TokenKind {
        let mut remaining = n;
        let mut idx = self.pos;
        while idx < self.tokens.len() {
            if matches!(self.tokens[idx].kind, TokenKind::Continuation) {
                idx += 1;
                continue;
            }
            if remaining == 0 {
                return self.tokens[idx].kind;
            }
            remaining -= 1;
            idx += 1;
        }
        TokenKind::Eof
    }

    /// The current token. When the cursor is past the end of the token stream
    /// (or the stream is empty), returns a synthetic zero-length Eof token at
    /// the end of the source so the rest of the parser can stay uniform.
    ///
    /// Invariant: `pos` is never on a [`TokenKind::Continuation`]; the
    /// constructor and [`Cursor::bump`] both call `skip_continuations` after
    /// every position change.
    pub(crate) fn peek_tok(&self) -> Token {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos]
        } else {
            self.synthetic_eof()
        }
    }

    /// Advance one token and return the consumed token. Past end-of-stream
    /// this returns the (real or synthetic) Eof token repeatedly without
    /// advancing `pos`, which keeps error-recovery loops bounded.
    ///
    /// Caller contract: do NOT accumulate `bump().span` in a loop without
    /// a separate termination guard. At Eof this function returns the same
    /// zero-length span repeatedly without advancing `pos`; callers that
    /// merge those spans (e.g. into a span-of-recovered-tokens) get a fixed
    /// zero-length region. The forced-progress guard in [`Parser::parse_stmt`]
    /// owns the only legitimate at-Eof recovery loop and exits via an
    /// explicit Eof check.
    pub(crate) fn bump(&mut self) -> Token {
        let tok = self.peek_tok();
        if self.pos < self.tokens.len() && !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
            self.skip_continuations();
        }
        tok
    }

    pub(crate) fn at_kw(&self, kw: Kw) -> bool {
        matches!(self.peek(), TokenKind::Keyword(k) if k == kw)
    }

    pub(crate) fn at_punct(&self, p: Punct) -> bool {
        matches!(self.peek(), TokenKind::Punct(pp) if pp == p)
    }

    pub(crate) fn eat_kw(&mut self, kw: Kw) -> bool {
        if self.at_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn eat_punct(&mut self, p: Punct) -> bool {
        if self.at_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Require a keyword; on mismatch builds a structured `E0201` ParseError.
    /// `ctx` is a short noun phrase ("after `If` condition") interpolated
    /// into the diagnostic.
    ///
    /// The error variant is intentionally large (it carries a full
    /// `Diagnostic`); see the spec for the chosen shape.
    pub(crate) fn expect_kw(&mut self, kw: Kw, ctx: &str) -> Result<Token, ParseError> {
        if self.at_kw(kw) {
            Ok(self.bump())
        } else {
            let tok = self.peek_tok();
            let msg = format!(
                "expected keyword `{}` {}, found {}",
                kw.as_str(),
                ctx,
                describe_token(tok.kind, self.src, tok.span),
            );
            let label = Label::with_message(
                tok.span,
                format!("found: {}", describe_token(tok.kind, self.src, tok.span)),
            );
            Err(ParseError {
                diag: Box::new(Diagnostic::error(E_EXPECTED_TOKEN, msg, label)),
                span: tok.span,
            })
        }
    }

    /// Require a punctuation token; on mismatch builds a structured `E0201`
    /// ParseError. See [`Cursor::expect_kw`] for the `ctx` convention.
    pub(crate) fn expect_punct(&mut self, p: Punct, ctx: &str) -> Result<Token, ParseError> {
        if self.at_punct(p) {
            Ok(self.bump())
        } else {
            let tok = self.peek_tok();
            let msg = format!(
                "expected `{}` {}, found {}",
                punct_str(p),
                ctx,
                describe_token(tok.kind, self.src, tok.span),
            );
            let label = Label::with_message(
                tok.span,
                format!("found: {}", describe_token(tok.kind, self.src, tok.span)),
            );
            Err(ParseError {
                diag: Box::new(Diagnostic::error(E_EXPECTED_TOKEN, msg, label)),
                span: tok.span,
            })
        }
    }

    /// Consume consecutive `Newline` tokens; return how many were consumed.
    pub(crate) fn eat_newlines(&mut self) -> usize {
        let mut n = 0;
        while matches!(self.peek(), TokenKind::Newline) {
            self.bump();
            n += 1;
        }
        n
    }

    /// True at any token that terminates a statement (`Newline`, `:`, `Eof`).
    pub(crate) fn is_stmt_terminator(&self) -> bool {
        matches!(self.peek(), TokenKind::Newline | TokenKind::Eof) || self.at_punct(Punct::Colon)
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Borrow the original source string. Needed so callers (e.g. the parser
    /// driving `string_value::decode`) can slice tokens out of the source.
    pub(crate) fn src(&self) -> &str {
        self.src
    }

    /// Span of the token at the cursor; an end-of-stream zero-length span
    /// (from the terminal Eof token, or `0..0` for an empty input) past end.
    pub(crate) fn current_span(&self) -> Span {
        self.peek_tok().span
    }

    /// Build a synthetic zero-length Eof token at the end of the source.
    /// The lexer guarantees a terminal Eof in non-empty inputs, so this is
    /// only ever reached if `pos` runs past the slice — defensive only.
    fn synthetic_eof(&self) -> Token {
        let end = self
            .tokens
            .last()
            .map(|t| t.span.end)
            .unwrap_or(self.src.len() as u32);
        Token {
            kind: TokenKind::Eof,
            span: Span::new(end, end, self.file),
        }
    }
}

/// Human-readable description of a token for use inside diagnostic messages.
fn describe_token(kind: TokenKind, src: &str, span: Span) -> String {
    match kind {
        TokenKind::Eof => "end of input".to_string(),
        TokenKind::Newline => "end of line".to_string(),
        TokenKind::Keyword(kw) => format!("keyword `{}`", kw.as_str()),
        TokenKind::Ident { .. } => {
            let lexeme = span_slice(src, span);
            format!("identifier `{lexeme}`")
        }
        TokenKind::IntLit(v) => format!("integer literal `{v}`"),
        TokenKind::FloatLit(v) => format!("float literal `{}`", v.to_f64()),
        TokenKind::StrLit(_) => "string literal".to_string(),
        TokenKind::Punct(p) => format!("`{}`", punct_str(p)),
        TokenKind::Op(_) => {
            let lexeme = span_slice(src, span);
            format!("operator `{lexeme}`")
        }
        TokenKind::Continuation => "line continuation".to_string(),
        TokenKind::Whitespace => "whitespace".to_string(),
        TokenKind::Comment(_) => "comment".to_string(),
        TokenKind::Error(_) => "lexical error".to_string(),
    }
}

fn span_slice(src: &str, span: Span) -> &str {
    let start = span.start as usize;
    let end = span.end as usize;
    if start <= end && end <= src.len() {
        &src[start..end]
    } else {
        ""
    }
}

fn punct_str(p: Punct) -> &'static str {
    match p {
        Punct::LParen => "(",
        Punct::RParen => ")",
        Punct::LBracket => "[",
        Punct::RBracket => "]",
        Punct::Comma => ",",
        Punct::Colon => ":",
        Punct::Semicolon => ";",
        Punct::Dot => ".",
    }
}

/// What `consume_stmt_sep_or_terminator` last saw at the end of a statement.
/// Exposed via `Parser::last_term` so multi-statement contexts (single-line
/// `If` body) can tell "the body ended at `:` — more statements follow on the
/// same line" from "the body ended at `Newline` / `Eof` / a block-end keyword
/// — the arm is finished".
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum LastTerm {
    None,
    Colon,
    Newline,
    Eof,
    BlockEnd,
}

/// Owning state for the parser: cursor over the token stream, the arena it
/// is building into, and the diagnostic accumulator. Statement / expression
/// parsers are methods on `Parser` so they don't have to thread these three
/// pieces of state around as arguments.
pub(crate) struct Parser<'t> {
    cursor: Cursor<'t>,
    arena: Arena,
    diagnostics: Vec<Diagnostic>,
    last_term: LastTerm,
    /// Cursor position + diagnostic span at which the most recent
    /// `parse_stmt_inner` error was reported. If `parse_stmt` re-enters at
    /// the same position, recovery must force at least one token of forward
    /// progress before retrying — otherwise a sync target that's also a
    /// statement-position keyword (e.g. `EndFunction` appearing at top
    /// level) would parse-error, sync, stop at the same token, and loop
    /// forever. See `recovery_endfunction_at_top_level_does_not_loop`.
    ///
    /// FD-004 #9: the `Span` field is preserved so any forced-progress
    /// `Stmt::Error` node can span from the original error to the bumped
    /// token, instead of pinning the Error to just the bumped token's span.
    last_error: Option<(usize, Span)>,
}

impl<'t> Parser<'t> {
    pub(crate) fn new(tokens: &'t [Token], src: &'t str, file: FileId) -> Self {
        Self {
            cursor: Cursor::new(tokens, src, file),
            arena: Arena::new(),
            diagnostics: Vec::new(),
            last_term: LastTerm::None,
            last_error: None,
        }
    }

    /// Consume the parser and return the final [`ParseResult`]. The program
    /// statement list is filled in by W4+; for now it is empty by default and
    /// the [`parse`] driver below pushes top-level `Stmt::Error`s into it for
    /// the W2 scaffolding behavior.
    pub(crate) fn finish(self) -> ParseResult {
        ParseResult {
            arena: self.arena,
            program: Vec::new(),
            diagnostics: self.diagnostics,
        }
    }

    fn alloc(&mut self, node: Node, span: Span) -> NodeId {
        self.arena.alloc(node, span)
    }

    fn src(&self) -> &str {
        self.cursor.src()
    }

    /// Pratt expression parser. `min_bp` is the minimum left-binding-power
    /// the loop will accept for the next operator; recurse with a higher
    /// `min_bp` to enforce that operators bind tighter.
    pub(crate) fn parse_expr_bp(&mut self, min_bp: u8) -> Result<NodeId, ParseError> {
        let lhs_tok = self.cursor.peek_tok();
        let mut lhs = if let Some(bp) = prefix_bp(&lhs_tok.kind) {
            let op_tok = self.cursor.bump();
            let op = unop_from(&op_tok.kind).expect("prefix_bp matched but unop_from didn't");
            let rhs = self.parse_expr_bp(bp)?;
            let span = op_tok.span.merge(self.arena.span_of(rhs));
            self.alloc(Node::Expr(Expr::Unary { op, operand: rhs }), span)
        } else {
            self.parse_primary()?
        };

        loop {
            let tok = self.cursor.peek_tok();
            if let Some(pbp) = postfix_bp(&tok.kind) {
                if pbp < min_bp {
                    break;
                }
                lhs = self.parse_postfix(lhs)?;
                continue;
            }
            if let Some((lbp, rbp)) = infix_bp(&tok.kind) {
                if lbp < min_bp {
                    break;
                }
                let op_tok = self.cursor.bump();
                let op = binop_from(&op_tok.kind).expect("infix_bp matched but binop_from didn't");
                let rhs = self.parse_expr_bp(rbp)?;
                let span = self.arena.span_of(lhs).merge(self.arena.span_of(rhs));
                lhs = self.alloc(Node::Expr(Expr::Binary { op, lhs, rhs }), span);
                continue;
            }
            break;
        }

        Ok(lhs)
    }

    /// Atomic / prefix expressions: literals, identifiers, parenthesized
    /// expressions, and `New` expressions. Prefix unary operators are handled
    /// directly in [`Parser::parse_expr_bp`] so the binding-power table stays
    /// in one place.
    pub(crate) fn parse_primary(&mut self) -> Result<NodeId, ParseError> {
        let tok = self.cursor.peek_tok();
        match tok.kind {
            TokenKind::IntLit(v) => {
                self.cursor.bump();
                Ok(self.alloc(Node::Expr(Expr::IntLit(v)), tok.span))
            }
            TokenKind::FloatLit(v) => {
                self.cursor.bump();
                Ok(self.alloc(Node::Expr(Expr::FloatLit(v)), tok.span))
            }
            TokenKind::StrLit(kind) => {
                self.cursor.bump();
                // Decode escapes / strip raw indent. `decode` is total, so we
                // always get a usable value; diagnostics are side-channel and
                // attached at the offending byte. We keep the `StrLit` node
                // even on diagnostics so the rest of the parse can use it.
                let (value, mut diags) = string_value::decode(kind, self.src(), tok.span);
                self.diagnostics.append(&mut diags);
                Ok(self.alloc(Node::Expr(Expr::StrLit { value, kind }), tok.span))
            }
            TokenKind::Keyword(Kw::True) => {
                self.cursor.bump();
                Ok(self.alloc(Node::Expr(Expr::BoolLit(true)), tok.span))
            }
            TokenKind::Keyword(Kw::False) => {
                self.cursor.bump();
                Ok(self.alloc(Node::Expr(Expr::BoolLit(false)), tok.span))
            }
            TokenKind::Keyword(Kw::Null) => {
                self.cursor.bump();
                Ok(self.alloc(Node::Expr(Expr::NullLit), tok.span))
            }
            TokenKind::Ident { sigil } => {
                self.cursor.bump();
                let name_span = bare_name_span(tok.span, sigil);
                Ok(self.alloc(Node::Expr(Expr::Ident { name_span, sigil }), tok.span))
            }
            TokenKind::Punct(Punct::LParen) => {
                let open = self.cursor.bump();
                let inner = self.parse_expr_bp(0)?;
                let close = self
                    .cursor
                    .expect_punct(Punct::RParen, "to close parenthesized expression")?;
                let span = open.span.merge(close.span);
                Ok(self.alloc(Node::Expr(Expr::Paren { inner }), span))
            }
            TokenKind::Keyword(Kw::New) => self.parse_new(),
            TokenKind::Error(_) => {
                // Lexical error already reported by the lexer; emit a parser
                // diagnostic too so it is clear the parser couldn't make
                // progress here, and recover with `Expr::Error`.
                self.cursor.bump();
                self.diagnostics.push(Diagnostic::error(
                    E_BAD_STATEMENT,
                    "expected expression, found lexical error",
                    Label::new(tok.span),
                ));
                Ok(self.alloc(Node::Expr(Expr::Error), tok.span))
            }
            _ => Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_BAD_STATEMENT,
                    format!(
                        "expected expression, found {}",
                        describe_token(tok.kind, self.src(), tok.span)
                    ),
                    Label::new(tok.span),
                )),
                span: tok.span,
            }),
        }
    }

    /// Handle a single postfix operator (`(`, `[`, or `.`). Multiple postfix
    /// operations chain via the outer loop in [`Parser::parse_expr_bp`].
    fn parse_postfix(&mut self, lhs: NodeId) -> Result<NodeId, ParseError> {
        let lhs_span = self.arena.span_of(lhs);
        let tok = self.cursor.peek_tok();
        match tok.kind {
            TokenKind::Punct(Punct::LParen) => {
                self.cursor.bump();
                let args = self.parse_arg_list(Punct::RParen)?;
                let close = self
                    .cursor
                    .expect_punct(Punct::RParen, "in call argument list")?;
                let span = lhs_span.merge(close.span);
                Ok(self.alloc(Node::Expr(Expr::Call { callee: lhs, args }), span))
            }
            TokenKind::Punct(Punct::LBracket) => {
                self.cursor.bump();
                let indices = self.parse_arg_list(Punct::RBracket)?;
                let close = self
                    .cursor
                    .expect_punct(Punct::RBracket, "in index expression")?;
                if indices.is_empty() {
                    // `arr[]` is not a valid index expression. FD-004 #6:
                    // returning a malformed `Index { indices: [] }` would
                    // force every downstream consumer to special-case empty
                    // indices; `Expr::Error` is the standard
                    // diagnostic-already-emitted carrier.
                    let span = lhs_span.merge(close.span);
                    self.diagnostics.push(Diagnostic::error(
                        E_BAD_STATEMENT,
                        "index expression must have at least one index",
                        Label::new(span),
                    ));
                    return Ok(self.alloc(Node::Expr(Expr::Error), span));
                }
                let span = lhs_span.merge(close.span);
                Ok(self.alloc(
                    Node::Expr(Expr::Index {
                        array: lhs,
                        indices,
                    }),
                    span,
                ))
            }
            TokenKind::Punct(Punct::Dot) => {
                self.cursor.bump();
                let name_tok = self.cursor.peek_tok();
                match name_tok.kind {
                    TokenKind::Ident { sigil } => {
                        self.cursor.bump();
                        let name_span = bare_name_span(name_tok.span, sigil);
                        let span = lhs_span.merge(name_tok.span);
                        Ok(self.alloc(
                            Node::Expr(Expr::Field {
                                target: lhs,
                                name_span,
                            }),
                            span,
                        ))
                    }
                    _ => Err(ParseError {
                        diag: Box::new(Diagnostic::error(
                            E_EXPECTED_TOKEN,
                            format!(
                                "expected identifier after `.`, found {}",
                                describe_token(name_tok.kind, self.src(), name_tok.span)
                            ),
                            Label::new(name_tok.span),
                        )),
                        span: name_tok.span,
                    }),
                }
            }
            _ => unreachable!("parse_postfix called on non-postfix token"),
        }
    }

    /// Parse `expr (, expr)*` up to (but not consuming) `close`. An empty
    /// list — i.e. the next token is already `close` — is allowed; callers
    /// that require at least one argument check the result themselves.
    fn parse_arg_list(&mut self, close: Punct) -> Result<Vec<NodeId>, ParseError> {
        let mut args = Vec::new();
        if self.cursor.at_punct(close) {
            return Ok(args);
        }
        loop {
            let expr = self.parse_expr_bp(0)?;
            args.push(expr);
            if self.cursor.at_punct(Punct::Comma) {
                self.cursor.bump();
                continue;
            }
            break;
        }
        Ok(args)
    }

    /// Parse a `New T` or `New T[dim, …]` expression. The current token must
    /// be `Kw::New`.
    fn parse_new(&mut self) -> Result<NodeId, ParseError> {
        let new_tok = self.cursor.bump();
        // Use `parse_type_atom` (NOT `parse_type_expr`) so the postfix `[…]`
        // stays for `New` itself to consume as a dim-expression list. The
        // disambiguation in `parse_array_brackets` would actually leave a
        // `[10, 20]` alone, but calling `parse_type_atom` avoids relying on
        // that branch and keeps `New Integer[]` (no dims) producing a clear
        // "needs at least one dim" diagnostic below.
        let elem_id = self.parse_type_atom()?;
        if self.cursor.at_punct(Punct::LBracket) {
            self.cursor.bump();
            let dims = self.parse_arg_list(Punct::RBracket)?;
            let close = self
                .cursor
                .expect_punct(Punct::RBracket, "in `New` array dimensions")?;
            let span = new_tok.span.merge(close.span);
            if dims.is_empty() {
                // FD-004 #6: previously emitted a diagnostic and returned a
                // malformed `NewKind::Array { dims: [] }`; downstream code
                // (e.g. lvalue checks for FD-005 `Delete`) would then need to
                // special-case the empty-dims shape. `Expr::Error` is the
                // standard "diagnostic was already emitted" carrier.
                self.diagnostics.push(Diagnostic::error(
                    E_BAD_STATEMENT,
                    "`New T[…]` requires at least one dimension expression",
                    Label::new(span),
                ));
                return Ok(self.alloc(Node::Expr(Expr::Error), span));
            }
            return Ok(self.alloc(
                Node::Expr(Expr::New(NewKind::Array {
                    elem: elem_id,
                    dims,
                })),
                span,
            ));
        }
        let span = new_tok.span.merge(self.arena.span_of(elem_id));
        Ok(self.alloc(Node::Expr(Expr::New(NewKind::Type(elem_id))), span))
    }

    // ──────────────────────────────────────────────────────────────────────
    // W6: type expressions.
    // ──────────────────────────────────────────────────────────────────────

    /// Parse a full type expression: an atom (primitive, named, fn-ptr, or
    /// parenthesised) followed by zero or more postfix array brackets. See
    /// `cb_syntax.md` §5.4 and the FD-002 plan §B.6.
    pub(crate) fn parse_type_expr(&mut self) -> Result<NodeId, ParseError> {
        let atom = self.parse_type_atom()?;
        self.parse_array_brackets(atom)
    }

    /// Parse a "bare" type expression with no postfix array brackets. Used by
    /// `New` (which consumes the `[…]` itself as a dim-list) and `Redim`
    /// (which uses `[expr, …]` as a sizing list, not a type-rank marker).
    pub(crate) fn parse_type_atom(&mut self) -> Result<NodeId, ParseError> {
        let tok = self.cursor.peek_tok();
        match tok.kind {
            TokenKind::Keyword(kw) if is_primitive_type_kw(kw) => {
                self.cursor.bump();
                Ok(self.alloc(Node::TypeExpr(TypeExpr::Primitive { kw }), tok.span))
            }
            TokenKind::Keyword(Kw::Function) => self.parse_fn_ptr_type(),
            TokenKind::Punct(Punct::LParen) => {
                self.cursor.bump();
                let inner = self.parse_type_expr()?;
                let rparen = self
                    .cursor
                    .expect_punct(Punct::RParen, "in parenthesised type")?;
                let span = tok.span.merge(rparen.span);
                Ok(self.alloc(Node::TypeExpr(TypeExpr::Paren { inner }), span))
            }
            TokenKind::Ident { sigil: None } => {
                self.cursor.bump();
                Ok(self.alloc(
                    Node::TypeExpr(TypeExpr::Named {
                        name_span: tok.span,
                    }),
                    tok.span,
                ))
            }
            TokenKind::Ident { sigil: Some(_) } => {
                // A sigil on a type name is invalid. Consume the token so
                // recovery can continue and surface the issue via E0205.
                self.cursor.bump();
                Err(ParseError {
                    diag: Box::new(Diagnostic::error(
                        E_INVALID_TYPE_EXPR,
                        "type names cannot carry a sigil",
                        Label::new(tok.span),
                    )),
                    span: tok.span,
                })
            }
            _ => Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_INVALID_TYPE_EXPR,
                    format!(
                        "expected a type expression, found {}",
                        describe_token(tok.kind, self.src(), tok.span)
                    ),
                    Label::new(tok.span),
                )),
                span: tok.span,
            }),
        }
    }

    /// Wrap `base` in zero or more `Array(rank)` nodes by reading postfix
    /// `[]`, `[,]`, `[,,]`, etc. Stops as soon as the next bracket contains
    /// anything other than commas (i.e. the `[` belongs to a surrounding
    /// expression like `New T[10, 20]`).
    fn parse_array_brackets(&mut self, base: NodeId) -> Result<NodeId, ParseError> {
        let mut current = base;
        loop {
            if !matches!(self.cursor.peek(), TokenKind::Punct(Punct::LBracket)) {
                break;
            }
            // Only consume the bracket if its contents are empty or only
            // commas (a type-rank marker). A digit/ident/etc. inside means
            // the bracket is an expression list belonging to a parent node
            // (e.g. `New Integer[10]`).
            match self.cursor.peek_n(1) {
                TokenKind::Punct(Punct::RBracket) | TokenKind::Punct(Punct::Comma) => {}
                _ => break,
            }
            self.cursor.bump(); // `[`
            let mut rank: u8 = 1;
            while self.cursor.eat_punct(Punct::Comma) {
                rank = rank.saturating_add(1);
            }
            let rbrack = self.cursor.expect_punct(Punct::RBracket, "in array type")?;
            let span = self.arena.span_of(current).merge(rbrack.span);
            current = self.alloc(
                Node::TypeExpr(TypeExpr::Array {
                    elem: current,
                    rank,
                }),
                span,
            );
        }
        Ok(current)
    }

    /// Parse a function-pointer type starting at `Function`. The current
    /// token must be `Kw::Function`. Right-associative return chain (`As`
    /// recurses into `parse_type_expr`) per §5.4.
    fn parse_fn_ptr_type(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Function`
        self.cursor
            .expect_punct(Punct::LParen, "after `Function` in type expression")?;
        let mut params: Vec<NodeId> = Vec::new();
        if !matches!(self.cursor.peek(), TokenKind::Punct(Punct::RParen)) {
            loop {
                params.push(self.parse_type_position_param()?);
                if !self.cursor.eat_punct(Punct::Comma) {
                    break;
                }
            }
        }
        let rparen = self
            .cursor
            .expect_punct(Punct::RParen, "in `Function` type parameter list")?;
        let mut span = opener.merge(rparen.span);

        // Optional `As <ret_type>` — recurses into parse_type_expr (right-assoc).
        let ret = if self.cursor.eat_kw(Kw::As) {
            let r = self.parse_type_expr()?;
            span = span.merge(self.arena.span_of(r));
            Some(r)
        } else {
            None
        };

        Ok(self.alloc(Node::TypeExpr(TypeExpr::FnPtr { params, ret }), span))
    }

    /// Parse one parameter of a function-pointer type. The name is optional
    /// (so `Function(Integer, Float)` is legal); when present, `As` follows
    /// it. Defaults are NOT allowed in type position.
    fn parse_type_position_param(&mut self) -> Result<NodeId, ParseError> {
        let start = self.cursor.current_span();
        let mut name_span: Option<Span> = None;
        let mut sigil_opt: Option<Sigil> = None;

        // Decide whether the leading Ident is a parameter name. Heuristic:
        // - `Ident As …` → the Ident is a name.
        // - `Ident<sigil> ,` or `Ident<sigil> )` → sigil-only name (no `As`).
        // - otherwise the Ident itself is a Named type.
        if let TokenKind::Ident { sigil } = self.cursor.peek() {
            let tok = self.cursor.peek_tok();
            let nxt = self.cursor.peek_n(1);
            let looks_like_named_param = matches!(nxt, TokenKind::Keyword(Kw::As))
                || (sigil.is_some()
                    && matches!(nxt, TokenKind::Punct(Punct::Comma | Punct::RParen)));
            if looks_like_named_param {
                self.cursor.bump();
                name_span = Some(bare_name_span(tok.span, sigil));
                sigil_opt = sigil;
            }
        }

        // Optional `As <type>`. If we already consumed a name without `As`
        // (sigil-only case), no type expression follows. Otherwise (no name
        // taken, or `As` present) the next token must start a type expr.
        let ty = if self.cursor.eat_kw(Kw::As) || name_span.is_none() {
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        let end = if let Some(t) = ty {
            self.arena.span_of(t)
        } else {
            self.cursor.current_span()
        };
        let span = start.merge(end);
        Ok(self.alloc(
            Node::Param(Param {
                name_span,
                sigil: sigil_opt,
                ty,
                default: None,
            }),
            span,
        ))
    }

    // ──────────────────────────────────────────────────────────────────────
    // W4: statement dispatch + simple statements.
    // ──────────────────────────────────────────────────────────────────────

    /// Parse a single statement. On error, push a diagnostic, allocate a
    /// `Stmt::Error`, and synchronise to the next statement boundary. Returns
    /// `None` only at EOF (after eating any leading newlines).
    pub(crate) fn parse_stmt(&mut self) -> Option<NodeId> {
        self.cursor.eat_newlines();
        if matches!(self.cursor.peek(), TokenKind::Eof) {
            return None;
        }
        // Forced-progress guard: if the previous call errored, ran sync, and
        // left us at exactly this position, retrying `parse_stmt_inner` here
        // would produce the same error and the same no-progress sync — an
        // infinite loop. The sync set intentionally contains tokens we
        // *don't* consume (block-closers, statement-position keywords); when
        // such a token appears in a context that can't actually start a
        // statement, we bump past it once and try again.
        if let Some((pos, orig_span)) = self.last_error
            && pos == self.cursor.pos()
            && !matches!(self.cursor.peek(), TokenKind::Eof)
        {
            let bad = self.cursor.bump();
            // Re-eat any newlines we may have just exposed.
            self.cursor.eat_newlines();
            if matches!(self.cursor.peek(), TokenKind::Eof) {
                // Allocate a placeholder Error so the program still has a node
                // attributed to this position; the diagnostic was already
                // emitted on the previous turn. FD-004 #9: span the Error
                // from the original error site to the bumped token so the
                // recovered range is visible.
                let span = orig_span.merge(bad.span);
                let id = self.alloc(Node::Stmt(Stmt::Error), span);
                self.last_error = None;
                return Some(id);
            }
            self.last_error = None;
        }
        let result = self.parse_stmt_inner();
        match result {
            Ok(id) => {
                self.last_error = None;
                Some(id)
            }
            Err(err) => {
                let err_span = err.span;
                self.diagnostics.push(*err.diag);
                let id = self.alloc(Node::Stmt(Stmt::Error), err_span);
                let pre_sync = self.cursor.pos();
                self.sync_to_stmt_boundary();
                // Track where this recovery left the cursor *only when sync
                // made no progress*. If sync consumed a `Newline`/`:` (or
                // bumped past any token), the next `parse_stmt` starts on a
                // fresh statement and doesn't need the forced-progress bump.
                // Sync stopping in place implies the next token is a
                // statement-position keyword (e.g. `EndFunction` at top level)
                // — re-entering would just repeat the same diagnostic, so
                // `parse_stmt` will bump once before retrying.
                self.last_error = if self.cursor.pos() == pre_sync {
                    Some((self.cursor.pos(), err_span))
                } else {
                    None
                };
                Some(id)
            }
        }
    }

    fn parse_stmt_inner(&mut self) -> Result<NodeId, ParseError> {
        let tok = self.cursor.peek_tok();
        match tok.kind {
            TokenKind::Keyword(kw) => match kw {
                Kw::Return => self.parse_return(),
                Kw::Goto => self.parse_goto(),
                Kw::Include => self.parse_include(),
                Kw::Break => self.parse_break(),
                Kw::Continue => self.parse_continue(),

                // Block statements (W5).
                Kw::If => self.parse_if(),
                Kw::While => self.parse_while(),
                Kw::Repeat => self.parse_repeat(),
                Kw::For => self.parse_for(),
                Kw::Select => self.parse_select(),

                // Declaration statements (W6).
                Kw::Function => self.parse_function(),
                Kw::Type => self.parse_type_decl(),
                Kw::Struct => self.parse_struct_decl(),
                Kw::Dim => self.parse_dim(),
                Kw::Redim => self.parse_redim(),
                Kw::Const => self.parse_const(/*global=*/ false),
                Kw::Global => {
                    // `Global Const X = …` is a global constant; otherwise a
                    // plain variable declaration.
                    if matches!(self.cursor.peek_n(1), TokenKind::Keyword(Kw::Const)) {
                        self.cursor.bump(); // consume `Global`
                        self.parse_const(/*global=*/ true)
                    } else {
                        self.parse_global()
                    }
                }
                Kw::Field => {
                    // `Field` at top level / inside a function body is E0211;
                    // the legal site is inside a `Type`/`Struct` body, which
                    // calls `parse_field_decl_in_body` directly.
                    let span = self.cursor.peek_tok().span;
                    self.cursor.bump();
                    // Best-effort: consume to end of line so sync recovery
                    // doesn't double-emit on the rest of the Field line.
                    while !matches!(
                        self.cursor.peek(),
                        TokenKind::Newline | TokenKind::Eof | TokenKind::Punct(Punct::Colon)
                    ) {
                        self.cursor.bump();
                    }
                    Err(ParseError {
                        diag: Box::new(
                            Diagnostic::error(
                                E_FIELD_OUTSIDE_TYPE_BODY,
                                "`Field` declarations are only allowed inside a `Type` or `Struct` body",
                                Label::new(span),
                            )
                            .with_note(
                                "`Field` declarations only appear inside `Type` / `Struct` bodies",
                            ),
                        ),
                        span,
                    })
                }

                // Other keywords cannot start a statement.
                _ => Err(ParseError {
                    diag: Box::new(Diagnostic::error(
                        E_UNEXPECTED_TOKEN,
                        format!("`{}` cannot start a statement", kw.as_str()),
                        Label::new(tok.span),
                    )),
                    span: tok.span,
                }),
            },
            TokenKind::Ident { .. } => self.parse_label_or_expr_stmt(),

            // Should be unreachable: `parse_stmt` eats leading newlines and
            // returns `None` at EOF before calling `parse_stmt_inner`, and the
            // single-line `If` body guard in `parse_if` handles
            // `Colon`/`Newline`/`Eof` itself. Demoted from `unreachable!` to a
            // structured diagnostic so a future invariant violation produces
            // a clear failure instead of a hard panic.
            TokenKind::Newline | TokenKind::Eof => Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_INTERNAL_PARSER,
                    "internal: `parse_stmt_inner` reached a statement terminator",
                    Label::new(tok.span),
                )),
                span: tok.span,
            }),

            // Lex errors at statement start: skip the offending token with a
            // diagnostic and emit a `Stmt::Error` — but do NOT run the full
            // statement-boundary sync, since the lex error is a *single token*
            // and the rest of the line is very likely the user's real
            // statement (e.g. `@ x = 1\n` should leave `x = 1` parseable).
            TokenKind::Error(_) => {
                self.cursor.bump();
                self.diagnostics.push(Diagnostic::error(
                    E_UNEXPECTED_TOKEN,
                    "lexical error",
                    Label::new(tok.span),
                ));
                Ok(self.alloc(Node::Stmt(Stmt::Error), tok.span))
            }

            _ => Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_UNEXPECTED_TOKEN,
                    "unexpected token at start of statement",
                    Label::new(tok.span),
                )),
                span: tok.span,
            }),
        }
    }

    /// Handle a statement that starts with an `Ident`: either a `Label` (per
    /// §6.4), an implicit declaration with `As Type =` (per §4.1, FD-004 #4),
    /// or an assignment / expression statement / paren-less call.
    ///
    /// Uses pure lookahead (`peek_n`); never bumps speculatively.
    fn parse_label_or_expr_stmt(&mut self) -> Result<NodeId, ParseError> {
        let first = self.cursor.peek_tok();
        if let TokenKind::Ident { sigil } = first.kind {
            // Label shape: `Ident : Newline` or `Ident : Eof`.
            if matches!(self.cursor.peek_n(1), TokenKind::Punct(Punct::Colon))
                && matches!(self.cursor.peek_n(2), TokenKind::Newline | TokenKind::Eof)
            {
                // Sigil on a label is invalid; emit E0214 but still allocate.
                self.cursor.bump(); // ident
                let colon = self.cursor.bump(); // :
                let span = first.span.merge(colon.span);
                if sigil.is_some() {
                    self.diagnostics.push(Diagnostic::error(
                        E_LABEL_HAS_SIGIL,
                        "labels cannot carry a type sigil",
                        Label::new(first.span),
                    ));
                }
                let name_span = bare_name_span(first.span, sigil);
                // Consume the trailing newline/eof per the statement
                // terminator convention; EOF is not consumed.
                if matches!(self.cursor.peek(), TokenKind::Newline) {
                    self.cursor.bump();
                }
                return Ok(self.alloc(Node::Stmt(Stmt::Label { name_span }), span));
            }

            // Implicit declaration with type annotation: `Ident [Sigil] As Type = expr`.
            // §4.1 explicitly shows `z As String = "asd"` as a first-assignment
            // form; the bare-`Ident = expr` case continues to go through
            // `parse_expr_or_assign_stmt` (it lexes as an assignment to a
            // possibly-new variable).
            if matches!(self.cursor.peek_n(1), TokenKind::Keyword(Kw::As)) {
                return self.parse_implicit_decl_stmt(first.span, sigil);
            }
        }
        self.parse_expr_or_assign_stmt()
    }

    /// Parse an implicit declaration with `As` annotation:
    /// `<name>[<sigil>] As <Type> = <expr>` (§4.1, FD-004 #4). Produces a
    /// single-name `Stmt::Dim` with the type and initializer. The `= <expr>`
    /// tail is required; without it sema can't tell an implicit decl from a
    /// dangling `Ident As Type` (which has no statement meaning), and any
    /// later assignment would conflict. The current token at entry is the
    /// name `Ident`; `name_tok_span` and `sigil` come from
    /// `parse_label_or_expr_stmt`'s peek.
    fn parse_implicit_decl_stmt(
        &mut self,
        name_tok_span: Span,
        sigil: Option<Sigil>,
    ) -> Result<NodeId, ParseError> {
        self.cursor.bump(); // consume the name Ident
        let name_span = bare_name_span(name_tok_span, sigil);
        self.cursor
            .expect_kw(Kw::As, "after implicit declaration name")?;
        let ty = self.parse_type_expr()?;
        if !matches!(self.cursor.peek(), TokenKind::Op(Op::Eq)) {
            let tok = self.cursor.peek_tok();
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "implicit declaration with `As` requires an initializer; \
                     use `Dim` to declare without one",
                    Label::new(tok.span),
                )),
                span: tok.span,
            });
        }
        self.cursor.bump(); // `=`
        let value = self.parse_expr_bp(0)?;
        let span = name_tok_span.merge(self.arena.span_of(value));
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(
            Node::Stmt(Stmt::Dim {
                names: vec![DimName { name_span, sigil }],
                ty: Some(ty),
                init: Some(value),
            }),
            span,
        ))
    }

    /// Parse the three Ident-starting expression-statement forms:
    /// assignment, paren-less subroutine call, or a plain expression statement.
    ///
    /// Note on `=`: in CoolBasic the same `=` token serves both comparison
    /// (inside conditional contexts) and assignment (at statement level).
    /// At statement-level we want the top-level `=` to mean assignment, with
    /// the rhs parsed in full so `a = b = 5` reads as `Assign(a, Eq(b, 5))`.
    /// We achieve this by parsing the LHS with `min_bp = STMT_LHS_MIN_BP`,
    /// which is one above the comparison binding power (16) — so the Pratt
    /// loop will not consume `=` (or any other comparison) into the LHS.
    /// Pure expression statements that contain a top-level comparison are
    /// not allowed at statement level (a bare `a < b` would fall through to
    /// the "trailing token" error from `consume_stmt_sep_or_terminator`).
    fn parse_expr_or_assign_stmt(&mut self) -> Result<NodeId, ParseError> {
        let lhs = self.parse_expr_bp(STMT_LHS_MIN_BP)?;

        // Assignment? `<lhs-expr> = <rhs-expr>` — the parser is permissive on
        // lvalue shape; sema validates it.
        if matches!(self.cursor.peek(), TokenKind::Op(Op::Eq)) {
            self.cursor.bump();
            let rhs = self.parse_expr_bp(0)?;
            let span = self.arena.span_of(lhs).merge(self.arena.span_of(rhs));
            self.consume_stmt_sep_or_terminator()?;
            return Ok(self.alloc(
                Node::Stmt(Stmt::Assign {
                    target: lhs,
                    value: rhs,
                }),
                span,
            ));
        }

        // Paren-less subroutine call (§7.1)? Only when `lhs` is a bare Ident
        // (no postfix Call/Index/Field applied to it by the Pratt loop) and
        // the next token can start an expression.
        if self.is_paren_less_call_start(lhs) {
            let args = self.parse_comma_expr_list_until_stmt_end()?;
            let last_span = args
                .last()
                .map(|&id| self.arena.span_of(id))
                .unwrap_or_else(|| self.arena.span_of(lhs));
            let span = self.arena.span_of(lhs).merge(last_span);
            let call_id = self.alloc(Node::Expr(Expr::Call { callee: lhs, args }), span);
            self.consume_stmt_sep_or_terminator()?;
            return Ok(self.alloc(Node::Stmt(Stmt::ExprStmt { expr: call_id }), span));
        }

        // Plain expression statement.
        let span = self.arena.span_of(lhs);
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(Node::Stmt(Stmt::ExprStmt { expr: lhs }), span))
    }

    fn is_paren_less_call_start(&self, lhs: NodeId) -> bool {
        let is_bare_ident = matches!(&self.arena[lhs], Node::Expr(Expr::Ident { .. }));
        if !is_bare_ident {
            return false;
        }
        let next = self.cursor.peek();
        if matches!(
            next,
            TokenKind::Newline | TokenKind::Eof | TokenKind::Punct(Punct::Colon)
        ) {
            return false;
        }
        is_expr_start(next)
    }

    fn parse_comma_expr_list_until_stmt_end(&mut self) -> Result<Vec<NodeId>, ParseError> {
        let mut out = Vec::new();
        out.push(self.parse_expr_bp(0)?);
        while matches!(self.cursor.peek(), TokenKind::Punct(Punct::Comma)) {
            self.cursor.bump();
            out.push(self.parse_expr_bp(0)?);
        }
        Ok(out)
    }

    fn parse_return(&mut self) -> Result<NodeId, ParseError> {
        let start = self.cursor.bump().span; // consume `Return`
        let value = if self.cursor.is_stmt_terminator() {
            None
        } else {
            Some(self.parse_expr_bp(0)?)
        };
        let span = if let Some(v) = value {
            start.merge(self.arena.span_of(v))
        } else {
            start
        };
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(Node::Stmt(Stmt::Return { value }), span))
    }

    fn parse_goto(&mut self) -> Result<NodeId, ParseError> {
        let start = self.cursor.bump().span;
        let label_tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil } = label_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected label name after `Goto`",
                    Label::new(label_tok.span),
                )),
                span: label_tok.span,
            });
        };
        self.cursor.bump();
        // Parser is permissive on label sigils; sema enforces no-sigil.
        let label_span = bare_name_span(label_tok.span, sigil);
        let span = start.merge(label_tok.span);
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(Node::Stmt(Stmt::Goto { label_span }), span))
    }

    fn parse_include(&mut self) -> Result<NodeId, ParseError> {
        let start = self.cursor.bump().span;
        let path_tok = self.cursor.peek_tok();
        let TokenKind::StrLit(kind) = path_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected string literal path after `Include`",
                    Label::new(path_tok.span),
                )),
                span: path_tok.span,
            });
        };
        self.cursor.bump();
        let (value, mut diags) = string_value::decode(kind, self.src(), path_tok.span);
        self.diagnostics.append(&mut diags);
        let path_id = self.alloc(Node::Expr(Expr::StrLit { value, kind }), path_tok.span);
        let span = start.merge(path_tok.span);
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(Node::Stmt(Stmt::Include { path: path_id }), span))
    }

    fn parse_break(&mut self) -> Result<NodeId, ParseError> {
        let start = self.cursor.bump().span;
        let mut count: Option<u32> = None;
        let mut span = start;
        if !self.cursor.is_stmt_terminator() {
            // Must be a positive integer literal (§6.3). Anything else → E0213.
            let n_tok = self.cursor.peek_tok();
            match n_tok.kind {
                TokenKind::IntLit(v) if v > 0 && v <= u32::MAX as u64 => {
                    self.cursor.bump();
                    count = Some(v as u32);
                    span = start.merge(n_tok.span);
                }
                _ => {
                    // Consume the offending expression for recovery and record E0213.
                    let bad = self.parse_expr_bp(0)?;
                    let bad_span = self.arena.span_of(bad);
                    self.diagnostics.push(Diagnostic::error(
                        E_BREAK_COUNT_NOT_POSITIVE_INT_LITERAL,
                        "`Break` count must be a positive integer literal",
                        Label::new(bad_span),
                    ));
                    span = start.merge(bad_span);
                }
            }
        }
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(Node::Stmt(Stmt::Break { count }), span))
    }

    fn parse_continue(&mut self) -> Result<NodeId, ParseError> {
        let span = self.cursor.bump().span;
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(Node::Stmt(Stmt::Continue), span))
    }

    // ──────────────────────────────────────────────────────────────────────
    // W5: block statements (If / While / Repeat / For / Select).
    // ──────────────────────────────────────────────────────────────────────

    /// Parse statements until any of `closers` appears at the start of a
    /// statement (or EOF, or any `End*`/split `End <Kw>` form). Does NOT
    /// consume the closer. On EOF without a closer, emits `E0203
    /// UnterminatedBlock` with a secondary label pointing at `opener_span`.
    ///
    /// We additionally break on *any* `End*` keyword (joined or split) so the
    /// caller's `consume_block_closer` can emit `E0204` for a mismatched
    /// closer. Same for the loop-closer keywords (`Wend`, `Forever`, `Next`):
    /// breaking on them inside the wrong block lets the parent see them and
    /// either close the block or emit a mismatch.
    fn parse_block_until(
        &mut self,
        closers: &[Kw],
        opener_span: Span,
        opener_name: &str,
    ) -> Vec<NodeId> {
        let mut out = Vec::new();
        loop {
            self.cursor.eat_newlines();
            match self.cursor.peek() {
                TokenKind::Eof => {
                    self.diagnostics.push(
                        Diagnostic::error(
                            E_UNTERMINATED_BLOCK,
                            format!("unterminated `{}` block", opener_name),
                            Label::new(self.cursor.current_span()),
                        )
                        .with_secondary(Label::with_message(
                            opener_span,
                            format!("`{}` opens here", opener_name),
                        )),
                    );
                    return out;
                }
                // Explicit closer match: caller knows what to do.
                TokenKind::Keyword(kw) if closers.contains(&kw) => return out,
                // Any `End*` keyword — stop here. If it matches the expected
                // closer, `consume_block_closer` will accept it; otherwise it
                // emits E0204.
                TokenKind::Keyword(kw) if is_end_kw(kw) => return out,
                // Split `End <Kw>` form (e.g. `End If`).
                TokenKind::Keyword(Kw::End)
                    if split_end_to_joined(self.cursor.peek_n(1)).is_some() =>
                {
                    return out;
                }
                // Other loop-closers / case markers — let the parent decide.
                TokenKind::Keyword(kw)
                    if matches!(
                        kw,
                        Kw::Wend | Kw::Forever | Kw::Next | Kw::Else | Kw::ElseIf
                    ) && !closers.contains(&kw) =>
                {
                    return out;
                }
                _ => {
                    if let Some(id) = self.parse_stmt() {
                        out.push(id);
                    } else {
                        // EOF — top of loop will handle the E0203 emit.
                    }
                }
            }
        }
    }

    /// Consume the closer token(s). Accepts both joined (`EndIf`) and split
    /// (`End If`) forms. On mismatch (e.g. `EndType` closing a `Function`),
    /// emits `E0204` and consumes the bad closer so the parent block can move
    /// on.
    fn consume_block_closer(&mut self, expected: Kw, opener_span: Span, opener_name: &str) -> Span {
        let tok = self.cursor.peek_tok();
        match tok.kind {
            TokenKind::Keyword(kw) if kw == expected => {
                self.cursor.bump();
                tok.span
            }
            // Split form: `End <X>` where `End X` ≡ `expected`.
            TokenKind::Keyword(Kw::End)
                if split_end_to_joined(self.cursor.peek_n(1)) == Some(expected) =>
            {
                let start = self.cursor.bump().span; // `End`
                let end = self.cursor.bump().span; // the next keyword
                start.merge(end)
            }
            // Mismatched split: `End <other>`.
            TokenKind::Keyword(Kw::End) if split_end_to_joined(self.cursor.peek_n(1)).is_some() => {
                let actual_kw = split_end_to_joined(self.cursor.peek_n(1))
                    .expect("split_end_to_joined was Some above");
                let start = self.cursor.bump().span;
                let end = self.cursor.bump().span;
                let span = start.merge(end);
                self.diagnostics.push(
                    Diagnostic::error(
                        E_MISMATCHED_END_KEYWORD,
                        format!(
                            "expected `{}` to close `{}`, found `{}`",
                            kw_close_str(expected),
                            opener_name,
                            kw_close_str(actual_kw),
                        ),
                        Label::new(span),
                    )
                    .with_secondary(Label::with_message(
                        opener_span,
                        format!("`{}` opens here", opener_name),
                    )),
                );
                span
            }
            // Mismatched joined: `EndType` etc.
            TokenKind::Keyword(kw) if is_end_kw(kw) => {
                let span = self.cursor.bump().span;
                self.diagnostics.push(
                    Diagnostic::error(
                        E_MISMATCHED_END_KEYWORD,
                        format!(
                            "expected `{}` to close `{}`, found `{}`",
                            kw_close_str(expected),
                            opener_name,
                            kw_close_str(kw),
                        ),
                        Label::new(span),
                    )
                    .with_secondary(Label::with_message(
                        opener_span,
                        format!("`{}` opens here", opener_name),
                    )),
                );
                span
            }
            _ => {
                // FD-004 #13: `parse_block_until` should only return when a
                // closer-shaped token is present (or EOF, which it already
                // diagnosed). If we somehow land here, surface a structured
                // internal-error diagnostic so a future invariant violation
                // is loud instead of silently producing a node with a
                // current-token span.
                self.diagnostics.push(Diagnostic::error(
                    E_INTERNAL_PARSER,
                    format!(
                        "internal: `consume_block_closer` reached an unexpected token while closing `{opener_name}`",
                    ),
                    Label::new(tok.span),
                ));
                tok.span
            }
        }
    }

    fn parse_if(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `If`
        let cond = self.parse_expr_bp(0)?;
        self.cursor.expect_kw(Kw::Then, "after `If` condition")?;

        // Single-line vs block: peek ONE token (no eat_newlines).
        if matches!(self.cursor.peek(), TokenKind::Newline) {
            // Block form.
            self.cursor.bump(); // consume the newline
            let then_body =
                self.parse_block_until(&[Kw::ElseIf, Kw::Else, Kw::EndIf], opener, "If");
            let (elseifs, else_body) = self.parse_if_block_tail(opener)?;
            let close_span = self.consume_block_closer(Kw::EndIf, opener, "If");
            let span = opener.merge(close_span);
            let _ = self.consume_stmt_sep_or_terminator();
            Ok(self.alloc(
                Node::Stmt(Stmt::If {
                    cond,
                    then_body,
                    elseifs,
                    else_body,
                    form: IfForm::Block,
                }),
                span,
            ))
        } else {
            // Single-line form (§6.2): both the `Then` and `Else` arms can
            // chain multiple statements with `:`, terminated by `Else`,
            // end-of-line, or EOF. We read `self.last_term` (set by each
            // inner statement's `consume_stmt_sep_or_terminator`) to tell
            // "ended at `:` — keep going" from "ended at `Newline`/`Eof`/
            // block-end — stop".
            let mut then_body = Vec::new();
            // Guard: `If x Then :` / `If x Then\n` / `If x Then Else …` —
            // the Then body is empty. Recursing into `parse_single_line_body_stmt`
            // here would hit `parse_stmt_inner`'s terminator arm with a
            // confusing error. Emit E0215 and skip the body parse so recovery
            // continues cleanly. `Eof` is included for completeness even though
            // `parse_stmt` strips a trailing newline before re-entering.
            if matches!(
                self.cursor.peek(),
                TokenKind::Punct(Punct::Colon)
                    | TokenKind::Newline
                    | TokenKind::Eof
                    | TokenKind::Keyword(Kw::Else | Kw::ElseIf)
            ) {
                let here = self.cursor.current_span();
                self.diagnostics.push(
                    Diagnostic::error(
                        E_EMPTY_SINGLE_LINE_IF_BODY,
                        "single-line `If` requires at least one statement after `Then`",
                        Label::new(here),
                    )
                    .with_secondary(Label::with_message(opener, "single-line `If` opens here")),
                );
                // If we landed on a `:` here, eat it so the next statement
                // after the `If` doesn't get interpreted as a Then-body
                // statement that ran past the empty arm.
                if matches!(self.cursor.peek(), TokenKind::Punct(Punct::Colon)) {
                    self.cursor.bump();
                    self.last_term = LastTerm::Colon;
                }
            } else {
                loop {
                    then_body.push(self.parse_single_line_body_stmt()?);
                    if self.last_term != LastTerm::Colon {
                        break;
                    }
                    if matches!(
                        self.cursor.peek(),
                        TokenKind::Keyword(Kw::Else | Kw::ElseIf)
                            | TokenKind::Newline
                            | TokenKind::Eof
                    ) {
                        break;
                    }
                }
            }
            // Optional Else on the same line.
            let mut else_body: Option<Vec<NodeId>> = None;
            if matches!(self.cursor.peek(), TokenKind::Keyword(Kw::Else)) {
                self.cursor.bump();
                let mut body = Vec::new();
                // Same guard as the Then arm: an empty Else body (`Else :`,
                // `Else\n`, `Else <Eof>`) is E0215.
                if matches!(
                    self.cursor.peek(),
                    TokenKind::Punct(Punct::Colon) | TokenKind::Newline | TokenKind::Eof
                ) {
                    let here = self.cursor.current_span();
                    self.diagnostics.push(
                        Diagnostic::error(
                            E_EMPTY_SINGLE_LINE_IF_BODY,
                            "single-line `If` requires at least one statement after `Else`",
                            Label::new(here),
                        )
                        .with_secondary(Label::with_message(opener, "single-line `If` opens here")),
                    );
                    if matches!(self.cursor.peek(), TokenKind::Punct(Punct::Colon)) {
                        self.cursor.bump();
                        self.last_term = LastTerm::Colon;
                    }
                } else {
                    loop {
                        body.push(self.parse_single_line_body_stmt()?);
                        if self.last_term != LastTerm::Colon {
                            break;
                        }
                        if matches!(self.cursor.peek(), TokenKind::Newline | TokenKind::Eof) {
                            break;
                        }
                    }
                }
                else_body = Some(body);
            }
            // ElseIf is illegal in single-line form.
            if matches!(self.cursor.peek(), TokenKind::Keyword(Kw::ElseIf)) {
                let bad = self.cursor.peek_tok().span;
                self.diagnostics.push(
                    Diagnostic::error(
                        E_SINGLELINE_IF_DISALLOWS_ELSEIF,
                        "`ElseIf` is not allowed in a single-line `If`",
                        Label::new(bad),
                    )
                    .with_secondary(Label::with_message(opener, "single-line `If` opens here")),
                );
                // Recovery: consume up to end of line.
                while !matches!(self.cursor.peek(), TokenKind::Newline | TokenKind::Eof) {
                    self.cursor.bump();
                }
            }
            let last_span = self.last_body_span(&then_body, &else_body, opener);
            let span = opener.merge(last_span);
            // The body statements have already eaten their trailing terminator
            // via `consume_stmt_sep_or_terminator`. Don't error if we're at
            // EOF / closer.
            let _ = self.consume_stmt_sep_or_terminator();
            Ok(self.alloc(
                Node::Stmt(Stmt::If {
                    cond,
                    then_body,
                    elseifs: Vec::new(),
                    else_body,
                    form: IfForm::SingleLine,
                }),
                span,
            ))
        }
    }

    /// Parse one body statement of a single-line `If`. Reuses the regular
    /// statement parser — `consume_stmt_sep_or_terminator` already stops at
    /// `Else`/`ElseIf` (they're in `is_block_end_marker`).
    fn parse_single_line_body_stmt(&mut self) -> Result<NodeId, ParseError> {
        self.parse_stmt_inner()
    }

    fn parse_if_block_tail(
        &mut self,
        opener: Span,
    ) -> Result<(Vec<ElseIf>, Option<Vec<NodeId>>), ParseError> {
        let mut elseifs = Vec::new();
        let mut else_body: Option<Vec<NodeId>> = None;
        loop {
            match self.cursor.peek() {
                TokenKind::Keyword(Kw::ElseIf) => {
                    self.cursor.bump();
                    let cond = self.parse_expr_bp(0)?;
                    self.cursor
                        .expect_kw(Kw::Then, "after `ElseIf` condition")?;
                    self.require_newline_after_block_then("ElseIf");
                    self.cursor.eat_newlines();
                    let body =
                        self.parse_block_until(&[Kw::ElseIf, Kw::Else, Kw::EndIf], opener, "If");
                    elseifs.push(ElseIf { cond, body });
                }
                // Split: `Else If <cond> Then`
                TokenKind::Keyword(Kw::Else)
                    if matches!(self.cursor.peek_n(1), TokenKind::Keyword(Kw::If)) =>
                {
                    self.cursor.bump(); // Else
                    self.cursor.bump(); // If
                    let cond = self.parse_expr_bp(0)?;
                    self.cursor
                        .expect_kw(Kw::Then, "after `Else If` condition")?;
                    self.require_newline_after_block_then("Else If");
                    self.cursor.eat_newlines();
                    let body =
                        self.parse_block_until(&[Kw::ElseIf, Kw::Else, Kw::EndIf], opener, "If");
                    elseifs.push(ElseIf { cond, body });
                }
                TokenKind::Keyword(Kw::Else) => {
                    self.cursor.bump();
                    self.cursor.eat_newlines();
                    let body = self.parse_block_until(&[Kw::EndIf], opener, "If");
                    else_body = Some(body);
                    break;
                }
                _ => break, // Closer / EOF / wrong closer; handled by caller.
            }
        }
        Ok((elseifs, else_body))
    }

    fn require_newline_after_block_then(&mut self, opener_name: &str) {
        if !matches!(self.cursor.peek(), TokenKind::Newline | TokenKind::Eof) {
            let bad = self.cursor.peek_tok().span;
            self.diagnostics.push(Diagnostic::error(
                E_EXPECTED_TOKEN,
                format!("expected end of line after `{opener_name} … Then` (block form required)"),
                Label::new(bad),
            ));
        }
    }

    fn last_body_span(
        &self,
        then_body: &[NodeId],
        else_body: &Option<Vec<NodeId>>,
        fallback: Span,
    ) -> Span {
        if let Some(eb) = else_body
            && let Some(&id) = eb.last()
        {
            return self.arena.span_of(id);
        }
        if let Some(&id) = then_body.last() {
            return self.arena.span_of(id);
        }
        fallback
    }

    fn parse_while(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `While`
        let cond = self.parse_expr_bp(0)?;
        self.cursor.eat_newlines();
        let body = self.parse_block_until(&[Kw::Wend], opener, "While");
        let close_span = match self.cursor.peek() {
            TokenKind::Keyword(Kw::Wend) => self.cursor.bump().span,
            _ => {
                // Missing Wend — parse_block_until already emitted E0203 on
                // EOF. For any other terminator (e.g. wrong-closer that we
                // broke on), emit a diagnostic so the user sees what's wrong.
                let here = self.cursor.current_span();
                if !matches!(self.cursor.peek(), TokenKind::Eof) {
                    // Treat the wrong closer as a mismatched end.
                    let _ = self.consume_block_closer(Kw::Wend, opener, "While");
                }
                here
            }
        };
        let span = opener.merge(close_span);
        let _ = self.consume_stmt_sep_or_terminator();
        Ok(self.alloc(Node::Stmt(Stmt::While { cond, body }), span))
    }

    fn parse_repeat(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Repeat`
        self.cursor.eat_newlines();
        let body = self.parse_block_until(&[Kw::Forever, Kw::While], opener, "Repeat");
        match self.cursor.peek() {
            TokenKind::Keyword(Kw::Forever) => {
                let close_span = self.cursor.bump().span;
                let span = opener.merge(close_span);
                let _ = self.consume_stmt_sep_or_terminator();
                Ok(self.alloc(Node::Stmt(Stmt::RepeatForever { body }), span))
            }
            TokenKind::Keyword(Kw::While) => {
                self.cursor.bump();
                let cond = self.parse_expr_bp(0)?;
                let span = opener.merge(self.arena.span_of(cond));
                let _ = self.consume_stmt_sep_or_terminator();
                Ok(self.alloc(Node::Stmt(Stmt::RepeatWhile { body, cond }), span))
            }
            _ => {
                // Missing closer (likely EOF — E0203 already emitted by
                // parse_block_until). Fall back to RepeatForever so the AST
                // is usable.
                Ok(self.alloc(Node::Stmt(Stmt::RepeatForever { body }), opener))
            }
        }
    }

    fn parse_for(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `For`
        // Loop variable — must be an Ident.
        let var_tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil } = var_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected loop variable after `For`",
                    Label::new(var_tok.span),
                )),
                span: var_tok.span,
            });
        };
        self.cursor.bump();
        let name_span = bare_name_span(var_tok.span, sigil);
        let var = self.alloc(Node::Expr(Expr::Ident { name_span, sigil }), var_tok.span);
        // `=`
        if !matches!(self.cursor.peek(), TokenKind::Op(Op::Eq)) {
            let tok = self.cursor.peek_tok();
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected `=` after `For` loop variable",
                    Label::new(tok.span),
                )),
                span: tok.span,
            });
        }
        self.cursor.bump();

        // For Each?
        if matches!(self.cursor.peek(), TokenKind::Keyword(Kw::Each)) {
            self.cursor.bump();
            let source = self.parse_expr_bp(0)?;
            self.cursor.eat_newlines();
            let body = self.parse_block_until(&[Kw::Next], opener, "For Each");
            let close_span = match self.cursor.peek() {
                TokenKind::Keyword(Kw::Next) => self.cursor.bump().span,
                _ => {
                    let here = self.cursor.current_span();
                    if !matches!(self.cursor.peek(), TokenKind::Eof) {
                        let _ = self.consume_block_closer(Kw::Next, opener, "For Each");
                    }
                    here
                }
            };
            let next_name = self.parse_optional_next_name(sigil);
            let span = opener.merge(close_span);
            let _ = self.consume_stmt_sep_or_terminator();
            return Ok(self.alloc(
                Node::Stmt(Stmt::ForEach {
                    var,
                    source,
                    body,
                    next_name,
                }),
                span,
            ));
        }

        // Iterative form.
        let from = self.parse_expr_bp(0)?;
        self.cursor.expect_kw(Kw::To, "in `For` range")?;
        let to = self.parse_expr_bp(0)?;
        let step = if self.cursor.eat_kw(Kw::Step) {
            Some(self.parse_expr_bp(0)?)
        } else {
            None
        };
        self.cursor.eat_newlines();
        let body = self.parse_block_until(&[Kw::Next], opener, "For");
        let close_span = match self.cursor.peek() {
            TokenKind::Keyword(Kw::Next) => self.cursor.bump().span,
            _ => {
                let here = self.cursor.current_span();
                if !matches!(self.cursor.peek(), TokenKind::Eof) {
                    let _ = self.consume_block_closer(Kw::Next, opener, "For");
                }
                here
            }
        };
        let next_name = self.parse_optional_next_name(sigil);
        let span = opener.merge(close_span);
        let _ = self.consume_stmt_sep_or_terminator();
        Ok(self.alloc(
            Node::Stmt(Stmt::For {
                var,
                from,
                to,
                step,
                body,
                next_name,
            }),
            span,
        ))
    }

    /// Parse an optional name after `Next` (§6.3). Accepts sigilled idents
    /// (FD-004 #5); when the sigil differs from the loop variable's, emit
    /// `E0217` so the parser doesn't silently drop the user's name. The
    /// loop-var name match (e.g. `For i = … Next j`) is sema's job — only the
    /// sigil is checked here because the parser already has it.
    ///
    /// Returns the bare-name span (sigil byte excluded) on success.
    fn parse_optional_next_name(&mut self, loop_sigil: Option<Sigil>) -> Option<Span> {
        let tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil } = tok.kind else {
            return None;
        };
        self.cursor.bump();
        if sigil != loop_sigil {
            let msg = match (loop_sigil, sigil) {
                (Some(l), Some(n)) => format!(
                    "`Next` name has sigil `{}` but the loop variable has sigil `{}`",
                    n.as_char(),
                    l.as_char(),
                ),
                (Some(l), None) => format!(
                    "`Next` name has no sigil but the loop variable has sigil `{}`",
                    l.as_char(),
                ),
                (None, Some(n)) => format!(
                    "`Next` name has sigil `{}` but the loop variable has no sigil",
                    n.as_char(),
                ),
                (None, None) => unreachable!("sigil != loop_sigil but both are None"),
            };
            self.diagnostics
                .push(Diagnostic::error(E_NEXT_SIGIL_MISMATCH, msg, Label::new(tok.span)));
        }
        Some(bare_name_span(tok.span, sigil))
    }

    fn parse_select(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Select`
        let scrutinee = self.parse_expr_bp(0)?;
        self.cursor.eat_newlines();
        let mut arms = Vec::new();
        // FD-004 #10: track the span of the first `Default` arm so a second
        // one can be diagnosed with a secondary label pointing at it.
        let mut first_default_span: Option<Span> = None;
        loop {
            self.cursor.eat_newlines();
            match self.cursor.peek() {
                TokenKind::Keyword(Kw::EndSelect) => break,
                TokenKind::Keyword(Kw::End)
                    if matches!(self.cursor.peek_n(1), TokenKind::Keyword(Kw::Select)) =>
                {
                    break;
                }
                // Other End* / split End <X> shapes: stop here so the
                // closer-consumer can emit E0204.
                TokenKind::Keyword(kw) if is_end_kw(kw) => break,
                TokenKind::Keyword(Kw::End)
                    if split_end_to_joined(self.cursor.peek_n(1)).is_some() =>
                {
                    break;
                }
                TokenKind::Eof => {
                    self.diagnostics.push(
                        Diagnostic::error(
                            E_UNTERMINATED_BLOCK,
                            "unterminated `Select` block",
                            Label::new(self.cursor.current_span()),
                        )
                        .with_secondary(Label::with_message(opener, "`Select` opens here")),
                    );
                    break;
                }
                TokenKind::Keyword(Kw::Case) => arms.push(self.parse_case_arm()?),
                TokenKind::Keyword(Kw::Default) => {
                    let default_kw_span = self.cursor.peek_tok().span;
                    let arm_id = self.parse_default_arm()?;
                    if let Some(prev_span) = first_default_span {
                        self.diagnostics.push(
                            Diagnostic::error(
                                E_DUPLICATE_DEFAULT,
                                "`Select` has more than one `Default` arm",
                                Label::new(default_kw_span),
                            )
                            .with_secondary(Label::with_message(
                                prev_span,
                                "first `Default` arm here",
                            )),
                        );
                    } else {
                        first_default_span = Some(default_kw_span);
                    }
                    arms.push(arm_id);
                }
                _ => {
                    // Stray token — emit a diagnostic and consume one to
                    // make progress.
                    let tok = self.cursor.peek_tok();
                    self.diagnostics.push(Diagnostic::error(
                        E_UNEXPECTED_TOKEN,
                        "expected `Case`, `Default`, or `EndSelect` inside `Select`",
                        Label::new(tok.span),
                    ));
                    self.cursor.bump();
                }
            }
        }
        let close_span = if matches!(self.cursor.peek(), TokenKind::Eof) {
            self.cursor.current_span()
        } else {
            self.consume_block_closer(Kw::EndSelect, opener, "Select")
        };
        let span = opener.merge(close_span);
        let _ = self.consume_stmt_sep_or_terminator();
        Ok(self.alloc(Node::Stmt(Stmt::Select { scrutinee, arms }), span))
    }

    fn parse_case_arm(&mut self) -> Result<NodeId, ParseError> {
        let start = self.cursor.bump().span; // `Case`
        let mut values = Vec::new();
        values.push(self.parse_expr_bp(0)?);
        while matches!(self.cursor.peek(), TokenKind::Punct(Punct::Comma)) {
            self.cursor.bump();
            values.push(self.parse_expr_bp(0)?);
        }
        self.cursor.eat_newlines();
        let body = self.parse_block_until(&[Kw::Case, Kw::Default, Kw::EndSelect], start, "Case");
        let span = if let Some(&last) = body.last() {
            start.merge(self.arena.span_of(last))
        } else if let Some(&v) = values.last() {
            start.merge(self.arena.span_of(v))
        } else {
            start
        };
        Ok(self.alloc(Node::CaseArm(CaseArm::Case { values, body }), span))
    }

    fn parse_default_arm(&mut self) -> Result<NodeId, ParseError> {
        let start = self.cursor.bump().span; // `Default`
        self.cursor.eat_newlines();
        let body =
            self.parse_block_until(&[Kw::Case, Kw::Default, Kw::EndSelect], start, "Default");
        let span = if let Some(&last) = body.last() {
            start.merge(self.arena.span_of(last))
        } else {
            start
        };
        Ok(self.alloc(Node::CaseArm(CaseArm::Default { body }), span))
    }

    // ──────────────────────────────────────────────────────────────────────
    // W6: declaration statements.
    // ──────────────────────────────────────────────────────────────────────

    /// Parse `Function name(params) [As ret] body EndFunction`. The
    /// subroutine form (no return type) is allowed; the return-type sigil on
    /// the name (e.g. `Function area#(…)`) is captured on `return_sigil`.
    fn parse_function(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Function`
        let name_tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil: name_sigil } = name_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected function name after `Function`",
                    Label::new(name_tok.span),
                )),
                span: name_tok.span,
            });
        };
        self.cursor.bump();
        let name_span = bare_name_span(name_tok.span, name_sigil);

        self.cursor
            .expect_punct(Punct::LParen, "after function name")?;
        let mut params: Vec<NodeId> = Vec::new();
        if !matches!(self.cursor.peek(), TokenKind::Punct(Punct::RParen)) {
            loop {
                params.push(self.parse_decl_param()?);
                if !self.cursor.eat_punct(Punct::Comma) {
                    break;
                }
            }
        }
        self.cursor
            .expect_punct(Punct::RParen, "in function parameter list")?;

        // Optional `As <return_type>`. The name sigil (e.g. `Function area#`)
        // already supplies the return type via §7.2; sema reconciles the two.
        let return_ty = if self.cursor.eat_kw(Kw::As) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        self.cursor.eat_newlines();
        let body = self.parse_block_until(&[Kw::EndFunction], opener, "Function");
        let close_span = self.consume_block_closer(Kw::EndFunction, opener, "Function");
        let span = opener.merge(close_span);
        let _ = self.consume_stmt_sep_or_terminator();
        Ok(self.alloc(
            Node::Stmt(Stmt::Function {
                name_span,
                return_sigil: name_sigil,
                params,
                return_ty,
                body,
            }),
            span,
        ))
    }

    /// Parameter in a function *declaration* — name required (unlike type
    /// position). Defaults allowed on trailing parameters (§7.2).
    fn parse_decl_param(&mut self) -> Result<NodeId, ParseError> {
        let start = self.cursor.current_span();
        let name_tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil } = name_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected parameter name",
                    Label::new(name_tok.span),
                )),
                span: name_tok.span,
            });
        };
        self.cursor.bump();
        let name_span = Some(bare_name_span(name_tok.span, sigil));

        let ty = if self.cursor.eat_kw(Kw::As) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        let default = if matches!(self.cursor.peek(), TokenKind::Op(Op::Eq)) {
            self.cursor.bump();
            Some(self.parse_expr_bp(0)?)
        } else {
            None
        };

        let end = if let Some(d) = default {
            self.arena.span_of(d)
        } else if let Some(t) = ty {
            self.arena.span_of(t)
        } else {
            name_tok.span
        };
        let span = start.merge(end);
        Ok(self.alloc(
            Node::Param(Param {
                name_span,
                sigil,
                ty,
                default,
            }),
            span,
        ))
    }

    /// Parse `Type <Name> … EndType`.
    fn parse_type_decl(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Type`
        let name_tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil: None } = name_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected type name after `Type`",
                    Label::new(name_tok.span),
                )),
                span: name_tok.span,
            });
        };
        self.cursor.bump();
        let name_span = name_tok.span;

        self.cursor.eat_newlines();
        let fields = self.parse_record_body(&[Kw::EndType], opener, "Type");
        let close_span = self.consume_block_closer(Kw::EndType, opener, "Type");
        let span = opener.merge(close_span);
        let _ = self.consume_stmt_sep_or_terminator();
        Ok(self.alloc(Node::Stmt(Stmt::Type { name_span, fields }), span))
    }

    /// Parse `Struct <Name> … EndStruct`.
    fn parse_struct_decl(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Struct`
        let name_tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil: None } = name_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected struct name after `Struct`",
                    Label::new(name_tok.span),
                )),
                span: name_tok.span,
            });
        };
        self.cursor.bump();
        let name_span = name_tok.span;

        self.cursor.eat_newlines();
        let fields = self.parse_record_body(&[Kw::EndStruct], opener, "Struct");
        let close_span = self.consume_block_closer(Kw::EndStruct, opener, "Struct");
        let span = opener.merge(close_span);
        let _ = self.consume_stmt_sep_or_terminator();
        Ok(self.alloc(Node::Stmt(Stmt::Struct { name_span, fields }), span))
    }

    /// Body of a `Type`/`Struct`: zero or more `Field <name> [As <type>]`
    /// lines. Anything else in the body emits a diagnostic and is consumed
    /// for recovery.
    fn parse_record_body(
        &mut self,
        closers: &[Kw],
        opener: Span,
        opener_name: &str,
    ) -> Vec<NodeId> {
        let mut fields = Vec::new();
        loop {
            self.cursor.eat_newlines();
            match self.cursor.peek() {
                TokenKind::Eof => {
                    self.diagnostics.push(
                        Diagnostic::error(
                            E_UNTERMINATED_BLOCK,
                            format!("unterminated `{}` block", opener_name),
                            Label::new(self.cursor.current_span()),
                        )
                        .with_secondary(Label::with_message(
                            opener,
                            format!("`{}` opens here", opener_name),
                        )),
                    );
                    return fields;
                }
                TokenKind::Keyword(kw) if closers.contains(&kw) => return fields,
                // Any other `End*` shape — let the outer consumer decide.
                TokenKind::Keyword(kw) if is_end_kw(kw) => return fields,
                TokenKind::Keyword(Kw::End)
                    if split_end_to_joined(self.cursor.peek_n(1)).is_some() =>
                {
                    return fields;
                }
                TokenKind::Keyword(Kw::Field) => match self.parse_field_decl_in_body() {
                    Ok(id) => fields.push(id),
                    Err(err) => {
                        self.diagnostics.push(*err.diag);
                        self.sync_to_stmt_boundary();
                    }
                },
                _ => {
                    let tok = self.cursor.peek_tok();
                    self.diagnostics.push(Diagnostic::error(
                        E_BAD_STATEMENT,
                        format!(
                            "only `Field` declarations are allowed inside `{}` body",
                            opener_name
                        ),
                        Label::new(tok.span),
                    ));
                    self.sync_to_stmt_boundary();
                }
            }
        }
    }

    /// Parse a `Field <name> [As <type>]` line inside a `Type`/`Struct` body.
    fn parse_field_decl_in_body(&mut self) -> Result<NodeId, ParseError> {
        let start = self.cursor.bump().span; // `Field`
        let name_tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil } = name_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected field name after `Field`",
                    Label::new(name_tok.span),
                )),
                span: name_tok.span,
            });
        };
        self.cursor.bump();
        let name_span = bare_name_span(name_tok.span, sigil);

        // Reject the multi-name form `Field x, y As Int` with E0210.
        if matches!(self.cursor.peek(), TokenKind::Punct(Punct::Comma)) {
            let comma_span = self.cursor.peek_tok().span;
            self.diagnostics.push(
                Diagnostic::error(
                    E_MULTI_NAME_NOT_ALLOWED,
                    "`Field` declares exactly one name; write one `Field` line per name",
                    Label::new(comma_span),
                )
                .with_note("write one declaration per name"),
            );
            // Eat the rest of the line for recovery.
            while !matches!(
                self.cursor.peek(),
                TokenKind::Newline | TokenKind::Eof | TokenKind::Punct(Punct::Colon)
            ) {
                self.cursor.bump();
            }
        }

        let ty = if self.cursor.eat_kw(Kw::As) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        let span = if let Some(t) = ty {
            start.merge(self.arena.span_of(t))
        } else {
            start.merge(name_tok.span)
        };
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(
            Node::Stmt(Stmt::FieldDecl {
                name_span,
                sigil,
                ty,
            }),
            span,
        ))
    }

    /// Parse `Dim …`. Delegates to `parse_dim_like(global=false)`.
    fn parse_dim(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Dim`
        self.parse_dim_like(opener, /*global=*/ false)
    }

    /// Parse `Global …`. Delegates to `parse_dim_like(global=true)`.
    fn parse_global(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Global`
        self.parse_dim_like(opener, /*global=*/ true)
    }

    /// Common body for `Dim` and `Global`. Multi-name is allowed when there
    /// is no initializer (§4.1); `Dim a, b As Int = 0` emits E0210.
    fn parse_dim_like(&mut self, opener: Span, global: bool) -> Result<NodeId, ParseError> {
        let (names, first_comma) = self.parse_dim_name_list()?;

        let ty = if self.cursor.eat_kw(Kw::As) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        let init = if matches!(self.cursor.peek(), TokenKind::Op(Op::Eq)) {
            self.cursor.bump();
            Some(self.parse_expr_bp(0)?)
        } else {
            None
        };

        if names.len() > 1 && init.is_some() {
            // Primary label points at the first comma (the offending shape);
            // secondary points at the opener for context.
            let primary_span = first_comma.unwrap_or(opener);
            self.diagnostics.push(
                Diagnostic::error(
                    E_MULTI_NAME_NOT_ALLOWED,
                    format!(
                        "multi-name `{}` does not accept an initializer; use single-name form",
                        if global { "Global" } else { "Dim" }
                    ),
                    Label::new(primary_span),
                )
                .with_secondary(Label::with_message(
                    opener,
                    format!("`{}` opens here", if global { "Global" } else { "Dim" }),
                ))
                .with_note("write one declaration per name"),
            );
        }

        let end_span = init
            .map(|i| self.arena.span_of(i))
            .or_else(|| ty.map(|t| self.arena.span_of(t)))
            .unwrap_or(opener);
        let span = opener.merge(end_span);
        self.consume_stmt_sep_or_terminator()?;

        let stmt = if global {
            Stmt::Global { names, ty, init }
        } else {
            Stmt::Dim { names, ty, init }
        };
        Ok(self.alloc(Node::Stmt(stmt), span))
    }

    /// Parse a comma-separated list of `Dim`/`Global` names. Returns the
    /// names and, when a comma appeared (i.e. multi-name form), the span of
    /// the first comma so a downstream E0210 can point at it.
    fn parse_dim_name_list(&mut self) -> Result<(Vec<DimName>, Option<Span>), ParseError> {
        let mut names = Vec::new();
        let mut first_comma: Option<Span> = None;
        loop {
            let tok = self.cursor.peek_tok();
            let TokenKind::Ident { sigil } = tok.kind else {
                return Err(ParseError {
                    diag: Box::new(Diagnostic::error(
                        E_EXPECTED_TOKEN,
                        "expected variable name",
                        Label::new(tok.span),
                    )),
                    span: tok.span,
                });
            };
            self.cursor.bump();
            let name_span = bare_name_span(tok.span, sigil);
            names.push(DimName { name_span, sigil });
            if self.cursor.at_punct(Punct::Comma) {
                if first_comma.is_none() {
                    first_comma = Some(self.cursor.peek_tok().span);
                }
                self.cursor.bump();
                continue;
            }
            break;
        }
        Ok((names, first_comma))
    }

    /// Parse `Const x [As T] = expr`. Multi-name (`Const A = 1, B = 2`) is
    /// rejected with E0210. `global` should be true for `Global Const …`.
    fn parse_const(&mut self, global: bool) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Const`
        let name_tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil } = name_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected constant name after `Const`",
                    Label::new(name_tok.span),
                )),
                span: name_tok.span,
            });
        };
        self.cursor.bump();
        let name_span = bare_name_span(name_tok.span, sigil);

        let ty = if self.cursor.eat_kw(Kw::As) {
            Some(self.parse_type_expr()?)
        } else {
            None
        };

        if !matches!(self.cursor.peek(), TokenKind::Op(Op::Eq)) {
            let tok = self.cursor.peek_tok();
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "`Const` requires an initializer",
                    Label::new(tok.span),
                )),
                span: tok.span,
            });
        }
        self.cursor.bump(); // `=`
        let value = self.parse_expr_bp(0)?;

        // `Const A = 1, B = 2` → E0210.
        if matches!(self.cursor.peek(), TokenKind::Punct(Punct::Comma)) {
            let comma_span = self.cursor.peek_tok().span;
            self.diagnostics.push(
                Diagnostic::error(
                    E_MULTI_NAME_NOT_ALLOWED,
                    "`Const` declares exactly one name; write one `Const` line per name",
                    Label::new(comma_span),
                )
                .with_note("write one declaration per name"),
            );
            while !matches!(
                self.cursor.peek(),
                TokenKind::Newline | TokenKind::Eof | TokenKind::Punct(Punct::Colon)
            ) {
                self.cursor.bump();
            }
        }

        let span = opener.merge(self.arena.span_of(value));
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(
            Node::Stmt(Stmt::Const {
                name_span,
                sigil,
                ty,
                value,
                is_global: global,
            }),
            span,
        ))
    }

    /// Parse `Redim <target> As <elem-ty>[<dim>, …]`. The `[…]` here is a
    /// runtime dim-list, *not* a type-rank marker, so we parse the bracket
    /// contents as expressions.
    fn parse_redim(&mut self) -> Result<NodeId, ParseError> {
        let opener = self.cursor.bump().span; // `Redim`
        // Target is a bare Ident expression; sema validates lvalue-ness.
        let name_tok = self.cursor.peek_tok();
        let TokenKind::Ident { sigil } = name_tok.kind else {
            return Err(ParseError {
                diag: Box::new(Diagnostic::error(
                    E_EXPECTED_TOKEN,
                    "expected variable name after `Redim`",
                    Label::new(name_tok.span),
                )),
                span: name_tok.span,
            });
        };
        self.cursor.bump();
        let name_span = bare_name_span(name_tok.span, sigil);
        let target = self.alloc(Node::Expr(Expr::Ident { name_span, sigil }), name_tok.span);

        self.cursor.expect_kw(Kw::As, "after `Redim` target")?;
        // Element type — `parse_type_expr` so the user can carry rank markers
        // (`Integer[]`) on the element. The bracket-with-expression that
        // follows (`[N, M]`) is the dim-list, which `parse_array_brackets`
        // refuses to consume (it only matches empty/comma-only brackets).
        // FD-004 #8: this was previously `parse_type_atom`, which rejected
        // `Redim arr As Integer[][10]`.
        let elem_ty = self.parse_type_expr()?;
        self.cursor
            .expect_punct(Punct::LBracket, "after `Redim` element type")?;
        let mut dims = Vec::new();
        dims.push(self.parse_expr_bp(0)?);
        while self.cursor.eat_punct(Punct::Comma) {
            dims.push(self.parse_expr_bp(0)?);
        }
        let rbrack = self
            .cursor
            .expect_punct(Punct::RBracket, "in `Redim` dimensions")?;
        let span = opener.merge(rbrack.span);
        self.consume_stmt_sep_or_terminator()?;
        Ok(self.alloc(
            Node::Stmt(Stmt::Redim {
                target,
                elem_ty,
                dims,
            }),
            span,
        ))
    }

    /// After a statement, expect a `Newline`, `:`, end-of-input, or a
    /// block-end keyword (which is NOT consumed — it belongs to the parent
    /// block parser). Anything else is a syntax error.
    ///
    /// Records which terminator was seen on `self.last_term`. Callers that
    /// chain multiple statements on one line (e.g. the single-line `If` body
    /// per §6.2) read this to decide whether to continue or stop.
    fn consume_stmt_sep_or_terminator(&mut self) -> Result<(), ParseError> {
        match self.cursor.peek() {
            TokenKind::Newline => {
                self.cursor.bump();
                self.last_term = LastTerm::Newline;
                Ok(())
            }
            TokenKind::Punct(Punct::Colon) => {
                self.cursor.bump();
                self.last_term = LastTerm::Colon;
                Ok(())
            }
            TokenKind::Eof => {
                self.last_term = LastTerm::Eof;
                Ok(())
            }
            TokenKind::Keyword(kw) if is_block_end_marker(kw) => {
                self.last_term = LastTerm::BlockEnd;
                Ok(())
            }
            _ => {
                let tok = self.cursor.peek_tok();
                Err(ParseError {
                    diag: Box::new(Diagnostic::error(
                        E_EXPECTED_TOKEN,
                        "expected end of line, `:`, or end of input after statement",
                        Label::new(tok.span),
                    )),
                    span: tok.span,
                })
            }
        }
    }

    /// Bump tokens until a sync point; does NOT consume the sync token (except
    /// `Newline` / `Colon`, which ARE consumed as the terminator).
    fn sync_to_stmt_boundary(&mut self) {
        loop {
            match self.cursor.peek() {
                TokenKind::Eof => return,
                TokenKind::Newline => {
                    self.cursor.bump();
                    return;
                }
                TokenKind::Punct(Punct::Colon) => {
                    self.cursor.bump();
                    return;
                }
                TokenKind::Keyword(kw) if is_sync_kw(kw) => return,
                _ => {
                    self.cursor.bump();
                }
            }
        }
    }
}

/// True for any `End*` keyword (joined form). Used by `parse_block_until` to
/// break on the closer regardless of which one was expected, so the closer-
/// consumer can emit `E0204` for a mismatch.
fn is_end_kw(kw: Kw) -> bool {
    matches!(
        kw,
        Kw::EndIf | Kw::EndFunction | Kw::EndSelect | Kw::EndStruct | Kw::EndType
    )
}

/// Map the keyword token that follows `End ` to the equivalent joined-closer
/// keyword. E.g. `End If` → `Kw::EndIf`. Returns `None` for anything else.
fn split_end_to_joined(next: TokenKind) -> Option<Kw> {
    let TokenKind::Keyword(kw) = next else {
        return None;
    };
    Some(match kw {
        Kw::If => Kw::EndIf,
        Kw::Function => Kw::EndFunction,
        Kw::Select => Kw::EndSelect,
        Kw::Struct => Kw::EndStruct,
        Kw::Type => Kw::EndType,
        _ => return None,
    })
}

/// User-facing spelling of a closer keyword.
fn kw_close_str(kw: Kw) -> &'static str {
    match kw {
        Kw::EndIf => "EndIf",
        Kw::EndFunction => "EndFunction",
        Kw::EndSelect => "EndSelect",
        Kw::EndStruct => "EndStruct",
        Kw::EndType => "EndType",
        Kw::Wend => "Wend",
        Kw::Next => "Next",
        Kw::Forever => "Forever",
        other => other.as_str(),
    }
}

/// Whether a [`TokenKind`] can syntactically start an expression. Used by
/// the paren-less call heuristic in `parse_expr_or_assign_stmt`.
fn is_expr_start(k: TokenKind) -> bool {
    matches!(
        k,
        TokenKind::IntLit(_)
            | TokenKind::FloatLit(_)
            | TokenKind::StrLit(_)
            | TokenKind::Ident { .. }
            | TokenKind::Punct(Punct::LParen)
            | TokenKind::Op(Op::Plus | Op::Minus)
            | TokenKind::Keyword(Kw::True | Kw::False | Kw::Null | Kw::Not | Kw::BinNot | Kw::New)
    )
}

/// Keywords that close a block. `consume_stmt_sep_or_terminator` MUST leave
/// these in place so the parent block parser can see them.
fn is_block_end_marker(kw: Kw) -> bool {
    matches!(
        kw,
        Kw::EndIf
            | Kw::EndFunction
            | Kw::EndSelect
            | Kw::EndStruct
            | Kw::EndType
            | Kw::End
            | Kw::Else
            | Kw::ElseIf
            | Kw::Wend
            | Kw::Next
            | Kw::Forever
            | Kw::Case
            | Kw::Default
            // `While` here is the closer of a `Repeat … While` block.
            | Kw::While
    )
}

/// Statement-level synchronisation keywords. The recovery loop stops *at*
/// these (without consuming them), so the next `parse_stmt` call can handle
/// them as a block-opener / block-end marker.
fn is_sync_kw(kw: Kw) -> bool {
    is_block_end_marker(kw)
        || matches!(
            kw,
            Kw::Function
                | Kw::Type
                | Kw::Struct
                | Kw::If
                | Kw::While
                | Kw::Repeat
                | Kw::For
                | Kw::Select
                | Kw::Dim
                | Kw::Const
                | Kw::Global
                | Kw::Redim
                | Kw::Return
                | Kw::Goto
                | Kw::Include
                | Kw::Break
                | Kw::Continue
                | Kw::Field
                | Kw::Then
        )
}

/// Compute the bare-name span for an identifier token. The lexer includes
/// the sigil byte in the token's span; the bare name is the span minus that
/// trailing byte. All sigils are single ASCII bytes (`%#$!`).
fn bare_name_span(tok_span: Span, sigil: Option<Sigil>) -> Span {
    let trim = if sigil.is_some() { 1 } else { 0 };
    Span::new(
        tok_span.start,
        tok_span.end.saturating_sub(trim),
        tok_span.file,
    )
}

/// Whether a keyword names one of the primitive types accepted by the
/// type-expression parser. `Int`/`Integer` and `UInt`/`UInteger` are both
/// accepted here as spelling-preserving aliases (FD-004 #3); sema treats
/// the alias pairs as equivalent.
fn is_primitive_type_kw(kw: Kw) -> bool {
    matches!(
        kw,
        Kw::Byte
            | Kw::Short
            | Kw::Int
            | Kw::Integer
            | Kw::UInt
            | Kw::UInteger
            | Kw::Long
            | Kw::ULong
            | Kw::Float
            | Kw::Bool
            | Kw::String
    )
}

/// Minimum binding power for the LHS of a statement-level expression /
/// assignment. Set one above the comparison binding power (16) so the Pratt
/// loop refuses to consume `=` (and the other comparison operators) into
/// the LHS — letting `parse_expr_or_assign_stmt` see a top-level `=` and
/// dispatch to assignment. See the doc on that function for the design
/// rationale.
const STMT_LHS_MIN_BP: u8 = 17;

/// Binding powers for infix operators. Returns `(left_bp, right_bp)`; when
/// `right > left` the operator is right-associative.
///
/// The levels follow `docs/cb_syntax.md` §5.1; numeric values are chosen so
/// every level is strictly above the one below it and unary / postfix slot
/// in cleanly (see `prefix_bp` and `postfix_bp`).
fn infix_bp(kind: &TokenKind) -> Option<(u8, u8)> {
    Some(match kind {
        TokenKind::Keyword(Kw::Or) => (10, 11),
        TokenKind::Keyword(Kw::Xor) => (12, 13),
        TokenKind::Keyword(Kw::And) => (14, 15),
        TokenKind::Op(Op::Eq | Op::NotEq | Op::Lt | Op::Gt | Op::LtEq | Op::GtEq) => (16, 17),
        TokenKind::Keyword(Kw::BinOr) => (18, 19),
        TokenKind::Keyword(Kw::BinXor) => (20, 21),
        TokenKind::Keyword(Kw::BinAnd) => (22, 23),
        TokenKind::Keyword(Kw::Shl | Kw::Shr | Kw::Sar) => (24, 25),
        TokenKind::Op(Op::Plus | Op::Minus) => (26, 27),
        TokenKind::Op(Op::Star | Op::Slash | Op::BackSlash) => (28, 29),
        TokenKind::Keyword(Kw::Mod) => (28, 29),
        // `**` is right-associative: right_bp < left_bp.
        TokenKind::Op(Op::StarStar) => (31, 30),
        _ => return None,
    })
}

/// Binding power for a prefix unary operator.
///
/// Set to 30, **one below** `**`'s right-bp (also 30) so that `-2 ** 2`
/// parses as `-(2 ** 2)`, matching §5.1 (unary tighter than every infix
/// except `**`, which extracts its right operand "around" the unary). The
/// original FD-002 table had this at 32 — that produced `(-2) ** 2`, which
/// contradicts the spec. See the FD-002 plan §B.4 for the trace; recap:
///
///   `-2 ** 2`:
///     outer `parse_expr_bp(0)` sees `-`, recurses with min_bp = 30.
///     inner sees `2`, then `**` with infix_bp = (31, 30). `31 >= 30`, so
///     pow is consumed inside the unary; result = `Pow(2, 2)`.
///     outer wraps: `Neg(Pow(2, 2))`.  ✓ matches §5.1
fn prefix_bp(kind: &TokenKind) -> Option<u8> {
    Some(match kind {
        TokenKind::Op(Op::Plus | Op::Minus) => 30,
        TokenKind::Keyword(Kw::Not | Kw::BinNot) => 30,
        _ => return None,
    })
}

/// Binding power for postfix operators (`(`, `[`, `.`). Set above every
/// infix and the prefix unary so `f(x)`, `arr[i]`, and `obj.field` always
/// bind their callee/array/target before any surrounding operator.
fn postfix_bp(kind: &TokenKind) -> Option<u8> {
    Some(match kind {
        TokenKind::Punct(Punct::LParen) => 34,
        TokenKind::Punct(Punct::LBracket) => 34,
        TokenKind::Punct(Punct::Dot) => 34,
        _ => return None,
    })
}

/// Map an operator/keyword token to its [`BinOp`] variant. Returns `None`
/// for tokens that aren't binary operators.
fn binop_from(kind: &TokenKind) -> Option<BinOp> {
    Some(match kind {
        TokenKind::Op(Op::Plus) => BinOp::Add,
        TokenKind::Op(Op::Minus) => BinOp::Sub,
        TokenKind::Op(Op::Star) => BinOp::Mul,
        TokenKind::Op(Op::Slash) => BinOp::Div,
        TokenKind::Op(Op::BackSlash) => BinOp::IntDiv,
        TokenKind::Op(Op::StarStar) => BinOp::Pow,
        TokenKind::Op(Op::Eq) => BinOp::Eq,
        TokenKind::Op(Op::NotEq) => BinOp::NotEq,
        TokenKind::Op(Op::Lt) => BinOp::Lt,
        TokenKind::Op(Op::Gt) => BinOp::Gt,
        TokenKind::Op(Op::LtEq) => BinOp::LtEq,
        TokenKind::Op(Op::GtEq) => BinOp::GtEq,
        TokenKind::Keyword(Kw::And) => BinOp::And,
        TokenKind::Keyword(Kw::Or) => BinOp::Or,
        TokenKind::Keyword(Kw::Xor) => BinOp::Xor,
        TokenKind::Keyword(Kw::Mod) => BinOp::Mod,
        TokenKind::Keyword(Kw::BinAnd) => BinOp::BinAnd,
        TokenKind::Keyword(Kw::BinOr) => BinOp::BinOr,
        TokenKind::Keyword(Kw::BinXor) => BinOp::BinXor,
        TokenKind::Keyword(Kw::Shl) => BinOp::Shl,
        TokenKind::Keyword(Kw::Shr) => BinOp::Shr,
        TokenKind::Keyword(Kw::Sar) => BinOp::Sar,
        _ => return None,
    })
}

/// Map an operator/keyword token to its [`UnOp`] variant. Returns `None`
/// for tokens that aren't unary operators.
fn unop_from(kind: &TokenKind) -> Option<UnOp> {
    Some(match kind {
        TokenKind::Op(Op::Plus) => UnOp::Plus,
        TokenKind::Op(Op::Minus) => UnOp::Neg,
        TokenKind::Keyword(Kw::Not) => UnOp::Not,
        TokenKind::Keyword(Kw::BinNot) => UnOp::BinNot,
        _ => return None,
    })
}

/// Parse a token stream into a [`ParseResult`].
///
/// Drives [`Parser::parse_stmt`] in a loop until end-of-input. Each call to
/// `parse_stmt` either returns a statement node (possibly `Stmt::Error` after
/// recovery) or `None` at EOF.
pub fn parse(tokens: &[Token], src: &str, file: FileId) -> ParseResult {
    let mut parser = Parser::new(tokens, src, file);
    let mut program = Vec::new();
    while let Some(id) = parser.parse_stmt() {
        program.push(id);
    }
    let mut result = parser.finish();
    result.program = program;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::FileId;
    use crate::{LexerOptions, tokenize};

    #[test]
    fn parses_empty_file_to_empty_program() {
        let (tokens, lex_diags) = tokenize("", FileId(0), LexerOptions::default());
        assert!(lex_diags.is_empty());
        let result = parse(&tokens, "", FileId(0));
        assert_eq!(result.program, Vec::<NodeId>::new());
        assert!(result.diagnostics.is_empty());
        assert!(result.arena.is_empty());
    }

    #[test]
    fn parses_only_newlines_to_empty_program() {
        let src = "\n\n\n";
        let (tokens, _) = tokenize(src, FileId(0), LexerOptions::default());
        let result = parse(&tokens, src, FileId(0));
        assert!(result.program.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn unknown_token_at_stmt_start_recovers() {
        // `*` cannot start a statement; W2 stub recovers by allocating
        // Stmt::Error and bumping. Real dispatch will replace this in W4.
        let src = "*";
        let (tokens, _) = tokenize(src, FileId(0), LexerOptions::default());
        let result = parse(&tokens, src, FileId(0));
        assert_eq!(result.program.len(), 1);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, Some(E_UNEXPECTED_TOKEN));
    }
}

#[cfg(test)]
mod expr_tests {
    //! Unit tests for [`Parser::parse_expr_bp`] — the Pratt parser. These
    //! drive the parser directly (no statement-level dispatch needed) and
    //! assert the resulting AST shape against expected operator nestings.

    use super::*;
    use crate::ast::{BinOp, Expr, NewKind, Node, NodeId, TypeExpr, UnOp};
    use crate::span::FileId;
    use crate::token::StrLitKind;
    use crate::{LexerOptions, tokenize};

    /// Parse `src` as a single expression. Returns the populated arena, the
    /// root expression's NodeId, and any diagnostics produced.
    fn parse_expr(src: &str) -> (Arena, NodeId, Vec<Diagnostic>) {
        let (tokens, lex_diags) = tokenize(src, FileId(0), LexerOptions::default());
        assert!(lex_diags.is_empty(), "lex diags: {lex_diags:?}");
        let mut parser = Parser::new(&tokens, src, FileId(0));
        let id = parser.parse_expr_bp(0).expect("expression parse failed");
        let result = parser.finish();
        (result.arena, id, result.diagnostics)
    }

    fn expr_of(arena: &Arena, id: NodeId) -> &Expr {
        match &arena[id] {
            Node::Expr(e) => e,
            other => panic!("expected Expr, got {other:?}"),
        }
    }

    fn assert_int(arena: &Arena, id: NodeId, expected: u64) {
        match expr_of(arena, id) {
            Expr::IntLit(v) => assert_eq!(*v, expected, "wrong int literal"),
            other => panic!("expected IntLit({expected}), got {other:?}"),
        }
    }

    fn assert_ident(arena: &Arena, id: NodeId, src: &str, expected: &str) {
        match expr_of(arena, id) {
            Expr::Ident { name_span, .. } => {
                assert_eq!(name_span.slice(src), expected);
            }
            other => panic!("expected Ident `{expected}`, got {other:?}"),
        }
    }

    fn binary(arena: &Arena, id: NodeId) -> (BinOp, NodeId, NodeId) {
        match expr_of(arena, id) {
            Expr::Binary { op, lhs, rhs } => (*op, *lhs, *rhs),
            other => panic!("expected Binary, got {other:?}"),
        }
    }

    fn unary(arena: &Arena, id: NodeId) -> (UnOp, NodeId) {
        match expr_of(arena, id) {
            Expr::Unary { op, operand } => (*op, *operand),
            other => panic!("expected Unary, got {other:?}"),
        }
    }

    #[test]
    fn precedence_pow_right_assoc() {
        // 2 ** 3 ** 2 → Pow(2, Pow(3, 2))
        let src = "2 ** 3 ** 2";
        let (arena, root, diags) = parse_expr(src);
        assert!(diags.is_empty());
        let (op, lhs, rhs) = binary(&arena, root);
        assert_eq!(op, BinOp::Pow);
        assert_int(&arena, lhs, 2);
        let (op2, lhs2, rhs2) = binary(&arena, rhs);
        assert_eq!(op2, BinOp::Pow);
        assert_int(&arena, lhs2, 3);
        assert_int(&arena, rhs2, 2);
    }

    #[test]
    fn precedence_unary_neg_then_pow() {
        // -2 ** 2 → Neg(Pow(2, 2)) per §5.1 (unary tighter than every infix
        // except **; pow "extracts" its right operand around the unary).
        // This is why prefix_bp = 30, one below pow's right-bp.
        let src = "-2 ** 2";
        let (arena, root, diags) = parse_expr(src);
        assert!(diags.is_empty());
        let (uop, operand) = unary(&arena, root);
        assert_eq!(uop, UnOp::Neg);
        let (bop, lhs, rhs) = binary(&arena, operand);
        assert_eq!(bop, BinOp::Pow);
        assert_int(&arena, lhs, 2);
        assert_int(&arena, rhs, 2);
    }

    #[test]
    fn precedence_add_mul_left() {
        // a + b * c → Add(a, Mul(b, c))
        let src = "a + b * c";
        let (arena, root, _) = parse_expr(src);
        let (op, lhs, rhs) = binary(&arena, root);
        assert_eq!(op, BinOp::Add);
        assert_ident(&arena, lhs, src, "a");
        let (op2, l2, r2) = binary(&arena, rhs);
        assert_eq!(op2, BinOp::Mul);
        assert_ident(&arena, l2, src, "b");
        assert_ident(&arena, r2, src, "c");
    }

    #[test]
    fn precedence_mul_add_left() {
        // a * b + c → Add(Mul(a, b), c)
        let src = "a * b + c";
        let (arena, root, _) = parse_expr(src);
        let (op, lhs, rhs) = binary(&arena, root);
        assert_eq!(op, BinOp::Add);
        let (op2, l2, r2) = binary(&arena, lhs);
        assert_eq!(op2, BinOp::Mul);
        assert_ident(&arena, l2, src, "a");
        assert_ident(&arena, r2, src, "b");
        assert_ident(&arena, rhs, src, "c");
    }

    #[test]
    fn paren_overrides() {
        // (a + b) * c → Mul(Paren(Add(a, b)), c)
        let src = "(a + b) * c";
        let (arena, root, _) = parse_expr(src);
        let (op, lhs, rhs) = binary(&arena, root);
        assert_eq!(op, BinOp::Mul);
        let inner = match expr_of(&arena, lhs) {
            Expr::Paren { inner } => *inner,
            other => panic!("expected Paren, got {other:?}"),
        };
        let (op2, l2, r2) = binary(&arena, inner);
        assert_eq!(op2, BinOp::Add);
        assert_ident(&arena, l2, src, "a");
        assert_ident(&arena, r2, src, "b");
        assert_ident(&arena, rhs, src, "c");
    }

    #[test]
    fn bitwise_below_arith() {
        // a + b BinAnd mask → BinAnd(Add(a, b), mask)
        let src = "a + b BinAnd mask";
        let (arena, root, _) = parse_expr(src);
        let (op, lhs, rhs) = binary(&arena, root);
        assert_eq!(op, BinOp::BinAnd);
        let (op2, l2, r2) = binary(&arena, lhs);
        assert_eq!(op2, BinOp::Add);
        assert_ident(&arena, l2, src, "a");
        assert_ident(&arena, r2, src, "b");
        assert_ident(&arena, rhs, src, "mask");
    }

    #[test]
    fn cmp_below_and() {
        // a = b And c = d → And(Eq(a, b), Eq(c, d))
        let src = "a = b And c = d";
        let (arena, root, _) = parse_expr(src);
        let (op, lhs, rhs) = binary(&arena, root);
        assert_eq!(op, BinOp::And);
        let (lop, ll, lr) = binary(&arena, lhs);
        assert_eq!(lop, BinOp::Eq);
        assert_ident(&arena, ll, src, "a");
        assert_ident(&arena, lr, src, "b");
        let (rop, rl, rr) = binary(&arena, rhs);
        assert_eq!(rop, BinOp::Eq);
        assert_ident(&arena, rl, src, "c");
        assert_ident(&arena, rr, src, "d");
    }

    #[test]
    fn cmp_non_chaining() {
        // 1 < x < 10 → Lt(Lt(1, x), 10) — falls out of (16, 17) left-assoc.
        let src = "1 < x < 10";
        let (arena, root, _) = parse_expr(src);
        let (op, lhs, rhs) = binary(&arena, root);
        assert_eq!(op, BinOp::Lt);
        let (op2, l2, r2) = binary(&arena, lhs);
        assert_eq!(op2, BinOp::Lt);
        assert_int(&arena, l2, 1);
        assert_ident(&arena, r2, src, "x");
        assert_int(&arena, rhs, 10);
    }

    #[test]
    fn unary_not_then_and() {
        // Not a And b → And(Not(a), b)
        let src = "Not a And b";
        let (arena, root, _) = parse_expr(src);
        let (op, lhs, rhs) = binary(&arena, root);
        assert_eq!(op, BinOp::And);
        let (uop, inner) = unary(&arena, lhs);
        assert_eq!(uop, UnOp::Not);
        assert_ident(&arena, inner, src, "a");
        assert_ident(&arena, rhs, src, "b");
    }

    #[test]
    fn call_postfix() {
        // f(1, 2, 3) → Call(f, [1, 2, 3])
        let src = "f(1, 2, 3)";
        let (arena, root, _) = parse_expr(src);
        match expr_of(&arena, root) {
            Expr::Call { callee, args } => {
                assert_ident(&arena, *callee, src, "f");
                assert_eq!(args.len(), 3);
                assert_int(&arena, args[0], 1);
                assert_int(&arena, args[1], 2);
                assert_int(&arena, args[2], 3);
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn index_postfix() {
        // arr[i, j] → Index(arr, [i, j])
        let src = "arr[i, j]";
        let (arena, root, _) = parse_expr(src);
        match expr_of(&arena, root) {
            Expr::Index { array, indices } => {
                assert_ident(&arena, *array, src, "arr");
                assert_eq!(indices.len(), 2);
                assert_ident(&arena, indices[0], src, "i");
                assert_ident(&arena, indices[1], src, "j");
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    #[test]
    fn field_chain() {
        // a.b.c → Field(Field(a, b), c)
        let src = "a.b.c";
        let (arena, root, _) = parse_expr(src);
        let (outer_target, outer_name_span) = match expr_of(&arena, root) {
            Expr::Field { target, name_span } => (*target, *name_span),
            other => panic!("expected Field, got {other:?}"),
        };
        assert_eq!(outer_name_span.slice(src), "c");
        let (inner_target, inner_name_span) = match expr_of(&arena, outer_target) {
            Expr::Field { target, name_span } => (*target, *name_span),
            other => panic!("expected inner Field, got {other:?}"),
        };
        assert_ident(&arena, inner_target, src, "a");
        assert_eq!(inner_name_span.slice(src), "b");
    }

    #[test]
    fn mixed_postfix() {
        // f()[0] → Index(Call(f, []), [0])
        let src = "f()[0]";
        let (arena, root, _) = parse_expr(src);
        match expr_of(&arena, root) {
            Expr::Index { array, indices } => {
                match expr_of(&arena, *array) {
                    Expr::Call { callee, args } => {
                        assert_ident(&arena, *callee, src, "f");
                        assert!(args.is_empty());
                    }
                    other => panic!("expected Call, got {other:?}"),
                }
                assert_eq!(indices.len(), 1);
                assert_int(&arena, indices[0], 0);
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    #[test]
    fn new_type() {
        // New MyType → New(NewKind::Type(Named("MyType")))
        let src = "New MyType";
        let (arena, root, _) = parse_expr(src);
        match expr_of(&arena, root) {
            Expr::New(NewKind::Type(inner)) => match &arena[*inner] {
                Node::TypeExpr(TypeExpr::Named { name_span }) => {
                    assert_eq!(name_span.slice(src), "MyType");
                }
                other => panic!("expected Named type, got {other:?}"),
            },
            other => panic!("expected New(Type), got {other:?}"),
        }
    }

    #[test]
    fn new_array() {
        // New Integer[10, 20] → New(Array { elem: Integer, dims: [10, 20] })
        let src = "New Integer[10, 20]";
        let (arena, root, _) = parse_expr(src);
        match expr_of(&arena, root) {
            Expr::New(NewKind::Array { elem, dims }) => {
                match &arena[*elem] {
                    Node::TypeExpr(TypeExpr::Primitive { kw }) => {
                        assert_eq!(*kw, Kw::Integer);
                    }
                    other => panic!("expected Primitive(Integer), got {other:?}"),
                }
                assert_eq!(dims.len(), 2);
                assert_int(&arena, dims[0], 10);
                assert_int(&arena, dims[1], 20);
            }
            other => panic!("expected New(Array), got {other:?}"),
        }
    }

    #[test]
    fn str_lit_plain() {
        let src = "\"hi\"";
        let (arena, root, diags) = parse_expr(src);
        assert!(diags.is_empty());
        match expr_of(&arena, root) {
            Expr::StrLit { value, kind } => {
                assert_eq!(value, "hi");
                assert_eq!(*kind, StrLitKind::Plain);
            }
            other => panic!("expected StrLit, got {other:?}"),
        }
    }

    #[test]
    fn str_lit_escaped() {
        let src = "\"a\\nb\"";
        let (arena, root, diags) = parse_expr(src);
        assert!(diags.is_empty());
        match expr_of(&arena, root) {
            Expr::StrLit { value, kind } => {
                assert_eq!(value, "a\nb");
                assert_eq!(*kind, StrLitKind::Escaped);
            }
            other => panic!("expected StrLit, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod stmt_tests {
    //! Unit tests for [`parse`] — full statement-level parsing through the
    //! top-level `parse` entry point. Each test asserts the program node
    //! count, individual statement shapes, and (where relevant) the
    //! diagnostic codes produced.

    use super::*;
    use crate::ast::{Expr, Node, NodeId, Stmt};
    use crate::span::FileId;
    use crate::token::StrLitKind;
    use crate::{LexerOptions, tokenize};

    fn parse_src(src: &str) -> ParseResult {
        let (tokens, lex_diags) = tokenize(src, FileId(0), LexerOptions::default());
        assert!(lex_diags.is_empty(), "lex diags: {lex_diags:?}");
        parse(&tokens, src, FileId(0))
    }

    /// Same as `parse_src` but tolerates lexer diagnostics — used by the
    /// "lexical error at statement start" recovery test.
    fn parse_src_lossy(src: &str) -> ParseResult {
        let (tokens, _lex_diags) = tokenize(src, FileId(0), LexerOptions::default());
        parse(&tokens, src, FileId(0))
    }

    fn stmt_of(arena: &Arena, id: NodeId) -> &Stmt {
        match &arena[id] {
            Node::Stmt(s) => s,
            other => panic!("expected Stmt, got {other:?}"),
        }
    }

    fn expr_of(arena: &Arena, id: NodeId) -> &Expr {
        match &arena[id] {
            Node::Expr(e) => e,
            other => panic!("expected Expr, got {other:?}"),
        }
    }

    fn assert_ident(arena: &Arena, id: NodeId, src: &str, expected: &str) {
        match expr_of(arena, id) {
            Expr::Ident { name_span, .. } => {
                assert_eq!(name_span.slice(src), expected);
            }
            other => panic!("expected Ident `{expected}`, got {other:?}"),
        }
    }

    fn assert_int(arena: &Arena, id: NodeId, expected: u64) {
        match expr_of(arena, id) {
            Expr::IntLit(v) => assert_eq!(*v, expected, "wrong int literal"),
            other => panic!("expected IntLit({expected}), got {other:?}"),
        }
    }

    #[test]
    fn assign_simple() {
        let src = "x = 1\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Assign { target, value } => {
                assert_ident(&r.arena, *target, src, "x");
                assert_int(&r.arena, *value, 1);
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn assign_to_index() {
        let src = "arr[i, j] = 42\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Assign { target, value } => {
                match expr_of(&r.arena, *target) {
                    Expr::Index { array, indices } => {
                        assert_ident(&r.arena, *array, src, "arr");
                        assert_eq!(indices.len(), 2);
                        assert_ident(&r.arena, indices[0], src, "i");
                        assert_ident(&r.arena, indices[1], src, "j");
                    }
                    other => panic!("expected Index target, got {other:?}"),
                }
                assert_int(&r.arena, *value, 42);
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn assign_to_field() {
        let src = "node.f = v\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Assign { target, value } => {
                match expr_of(&r.arena, *target) {
                    Expr::Field { target: t, name_span } => {
                        assert_ident(&r.arena, *t, src, "node");
                        assert_eq!(name_span.slice(src), "f");
                    }
                    other => panic!("expected Field target, got {other:?}"),
                }
                assert_ident(&r.arena, *value, src, "v");
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn assign_not_chainable() {
        // `a = b = 5` parses as `Assign(a, Eq(b, 5))`. The first `=` is the
        // top-level statement assignment; the second `=` is a comparison op
        // inside the rhs expression.
        let src = "a = b = 5\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Assign { target, value } => {
                assert_ident(&r.arena, *target, src, "a");
                match expr_of(&r.arena, *value) {
                    Expr::Binary { op, lhs, rhs } => {
                        assert_eq!(*op, BinOp::Eq);
                        assert_ident(&r.arena, *lhs, src, "b");
                        assert_int(&r.arena, *rhs, 5);
                    }
                    other => panic!("expected Binary Eq, got {other:?}"),
                }
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn subroutine_call_no_parens() {
        let src = "Print \"x\"\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::ExprStmt { expr } => match expr_of(&r.arena, *expr) {
                Expr::Call { callee, args } => {
                    assert_ident(&r.arena, *callee, src, "Print");
                    assert_eq!(args.len(), 1);
                    match expr_of(&r.arena, args[0]) {
                        Expr::StrLit { value, kind } => {
                            assert_eq!(value, "x");
                            assert_eq!(*kind, StrLitKind::Plain);
                        }
                        other => panic!("expected StrLit arg, got {other:?}"),
                    }
                }
                other => panic!("expected Call, got {other:?}"),
            },
            other => panic!("expected ExprStmt, got {other:?}"),
        }
    }

    #[test]
    fn subroutine_call_with_parens() {
        let src = "Print(\"x\")\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::ExprStmt { expr } => match expr_of(&r.arena, *expr) {
                Expr::Call { callee, args } => {
                    assert_ident(&r.arena, *callee, src, "Print");
                    assert_eq!(args.len(), 1);
                }
                other => panic!("expected Call, got {other:?}"),
            },
            other => panic!("expected ExprStmt, got {other:?}"),
        }
    }

    #[test]
    fn subroutine_call_multi_arg_no_parens() {
        let src = "MySub 0.42, \"Hello\", \"World\"\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::ExprStmt { expr } => match expr_of(&r.arena, *expr) {
                Expr::Call { callee, args } => {
                    assert_ident(&r.arena, *callee, src, "MySub");
                    assert_eq!(args.len(), 3);
                    match expr_of(&r.arena, args[0]) {
                        Expr::FloatLit(v) => assert!((v.to_f64() - 0.42).abs() < 1e-9),
                        other => panic!("expected FloatLit, got {other:?}"),
                    }
                    match expr_of(&r.arena, args[1]) {
                        Expr::StrLit { value, .. } => assert_eq!(value, "Hello"),
                        other => panic!("expected StrLit, got {other:?}"),
                    }
                    match expr_of(&r.arena, args[2]) {
                        Expr::StrLit { value, .. } => assert_eq!(value, "World"),
                        other => panic!("expected StrLit, got {other:?}"),
                    }
                }
                other => panic!("expected Call, got {other:?}"),
            },
            other => panic!("expected ExprStmt, got {other:?}"),
        }
    }

    #[test]
    fn return_no_value() {
        let src = "Return\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Return { value } => assert!(value.is_none()),
            other => panic!("expected Return, got {other:?}"),
        }
    }

    #[test]
    fn return_value() {
        let src = "Return 42\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Return { value } => {
                let v = value.expect("Return should have a value");
                assert_int(&r.arena, v, 42);
            }
            other => panic!("expected Return, got {other:?}"),
        }
    }

    #[test]
    fn goto_simple() {
        let src = "Goto cleanup\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Goto { label_span } => {
                assert_eq!(label_span.slice(src), "cleanup");
            }
            other => panic!("expected Goto, got {other:?}"),
        }
    }

    #[test]
    fn include_simple() {
        let src = "Include \"utils.cb\"\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Include { path } => match expr_of(&r.arena, *path) {
                Expr::StrLit { value, .. } => assert_eq!(value, "utils.cb"),
                other => panic!("expected StrLit path, got {other:?}"),
            },
            other => panic!("expected Include, got {other:?}"),
        }
    }

    #[test]
    fn break_no_count() {
        let src = "Break\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Break { count } => assert!(count.is_none()),
            other => panic!("expected Break, got {other:?}"),
        }
    }

    #[test]
    fn break_with_count() {
        let src = "Break 2\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Break { count } => assert_eq!(*count, Some(2)),
            other => panic!("expected Break, got {other:?}"),
        }
    }

    #[test]
    fn break_bad_count_neg() {
        let src = "Break -1\n";
        let r = parse_src(src);
        // E0213 is recorded but AST still has Stmt::Break (with no count).
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_BREAK_COUNT_NOT_POSITIVE_INT_LITERAL)),
            "expected E0213 diagnostic, got {:?}",
            r.diagnostics
        );
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Break { count } => assert!(count.is_none()),
            other => panic!("expected Break, got {other:?}"),
        }
    }

    #[test]
    fn break_bad_count_ident() {
        let src = "Break n\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_BREAK_COUNT_NOT_POSITIVE_INT_LITERAL)),
            "expected E0213 diagnostic, got {:?}",
            r.diagnostics
        );
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Break { count } => assert!(count.is_none()),
            other => panic!("expected Break, got {other:?}"),
        }
    }

    #[test]
    fn continue_simple() {
        let src = "Continue\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Continue => {}
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn label_alone() {
        let src = "cleanup:\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Label { name_span } => {
                assert_eq!(name_span.slice(src), "cleanup");
            }
            other => panic!("expected Label, got {other:?}"),
        }
    }

    #[test]
    fn label_with_sigil_error() {
        let src = "foo$:\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_LABEL_HAS_SIGIL)),
            "expected E0214 diagnostic, got {:?}",
            r.diagnostics
        );
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Label { name_span } => {
                // name_span is the bare name (sigil byte trimmed).
                assert_eq!(name_span.slice(src), "foo");
            }
            other => panic!("expected Label, got {other:?}"),
        }
    }

    #[test]
    fn label_then_stmt() {
        // `cleanup: Print "x"` — per §6.4 a label is `Ident : Newline`;
        // otherwise the `:` is a statement separator. So this parses as
        // two statements: `ExprStmt(Ident("cleanup"))` then
        // `ExprStmt(Call(Print, ["x"]))`.
        let src = "cleanup: Print \"x\"\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 2);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::ExprStmt { expr } => assert_ident(&r.arena, *expr, src, "cleanup"),
            other => panic!("expected ExprStmt(Ident), got {other:?}"),
        }
        match stmt_of(&r.arena, r.program[1]) {
            Stmt::ExprStmt { expr } => match expr_of(&r.arena, *expr) {
                Expr::Call { callee, args } => {
                    assert_ident(&r.arena, *callee, src, "Print");
                    assert_eq!(args.len(), 1);
                }
                other => panic!("expected Call, got {other:?}"),
            },
            other => panic!("expected ExprStmt(Call), got {other:?}"),
        }
    }

    #[test]
    fn multi_stmt_one_line() {
        let src = "x = 1 : y = 2 : Print x + y\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 3);
        assert!(matches!(
            stmt_of(&r.arena, r.program[0]),
            Stmt::Assign { .. }
        ));
        assert!(matches!(
            stmt_of(&r.arena, r.program[1]),
            Stmt::Assign { .. }
        ));
        match stmt_of(&r.arena, r.program[2]) {
            Stmt::ExprStmt { expr } => match expr_of(&r.arena, *expr) {
                Expr::Call { callee, args } => {
                    assert_ident(&r.arena, *callee, src, "Print");
                    assert_eq!(args.len(), 1);
                    // The single arg is `x + y`.
                    match expr_of(&r.arena, args[0]) {
                        Expr::Binary { op, .. } => assert_eq!(*op, BinOp::Add),
                        other => panic!("expected Add, got {other:?}"),
                    }
                }
                other => panic!("expected Call, got {other:?}"),
            },
            other => panic!("expected ExprStmt, got {other:?}"),
        }
    }

    #[test]
    fn blank_lines_between_stmts() {
        let src = "x = 1\n\n\ny = 2\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 2);
        assert!(matches!(
            stmt_of(&r.arena, r.program[0]),
            Stmt::Assign { .. }
        ));
        assert!(matches!(
            stmt_of(&r.arena, r.program[1]),
            Stmt::Assign { .. }
        ));
    }

    #[test]
    fn recovery_garbage_then_stmt() {
        // `@` lexes as Error(UnexpectedChar). The parser recovers via
        // sync_to_stmt_boundary, then parses `x = 1`. There should be at
        // least one diagnostic.
        let src = "@ x = 1\n";
        let r = parse_src_lossy(src);
        assert!(!r.diagnostics.is_empty(), "expected at least one diag");
        assert_eq!(r.program.len(), 2);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Error => {}
            other => panic!("expected Stmt::Error, got {other:?}"),
        }
        match stmt_of(&r.arena, r.program[1]) {
            Stmt::Assign { target, value } => {
                assert_ident(&r.arena, *target, src, "x");
                assert_int(&r.arena, *value, 1);
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod block_tests {
    //! Unit tests for the W5 block-statement parsers: `If` (single-line and
    //! block), `While`, `Repeat`, `For`, `For Each`, and `Select`.

    use super::*;
    use crate::ast::{CaseArm, Expr, IfForm, Node, NodeId, Stmt};
    use crate::span::FileId;
    use crate::{LexerOptions, tokenize};

    fn parse_src(src: &str) -> ParseResult {
        let (tokens, lex_diags) = tokenize(src, FileId(0), LexerOptions::default());
        assert!(lex_diags.is_empty(), "lex diags: {lex_diags:?}");
        parse(&tokens, src, FileId(0))
    }

    fn stmt_of(arena: &Arena, id: NodeId) -> &Stmt {
        match &arena[id] {
            Node::Stmt(s) => s,
            other => panic!("expected Stmt, got {other:?}"),
        }
    }

    fn expr_of(arena: &Arena, id: NodeId) -> &Expr {
        match &arena[id] {
            Node::Expr(e) => e,
            other => panic!("expected Expr, got {other:?}"),
        }
    }

    fn case_arm_of(arena: &Arena, id: NodeId) -> &CaseArm {
        match &arena[id] {
            Node::CaseArm(c) => c,
            other => panic!("expected CaseArm, got {other:?}"),
        }
    }

    fn assert_ident(arena: &Arena, id: NodeId, src: &str, expected: &str) {
        match expr_of(arena, id) {
            Expr::Ident { name_span, .. } => {
                assert_eq!(name_span.slice(src), expected);
            }
            other => panic!("expected Ident `{expected}`, got {other:?}"),
        }
    }

    // ─── If ──────────────────────────────────────────────────────────────

    #[test]
    fn if_block_simple() {
        let src = "If x > 0 Then\n  Print \"y\"\nEndIf\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                then_body,
                elseifs,
                else_body,
                form,
                ..
            } => {
                assert_eq!(*form, IfForm::Block);
                assert_eq!(then_body.len(), 1);
                assert!(elseifs.is_empty());
                assert!(else_body.is_none());
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_block_else() {
        let src = "If c Then\n  A\nElse\n  B\nEndIf\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                then_body,
                elseifs,
                else_body,
                form,
                ..
            } => {
                assert_eq!(*form, IfForm::Block);
                assert_eq!(then_body.len(), 1);
                assert!(elseifs.is_empty());
                let eb = else_body.as_ref().expect("else_body");
                assert_eq!(eb.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_block_elseif() {
        let src = "If c1 Then\n  a\nElseIf c2 Then\n  b\nElse\n  c\nEndIf\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                then_body,
                elseifs,
                else_body,
                form,
                ..
            } => {
                assert_eq!(*form, IfForm::Block);
                assert_eq!(then_body.len(), 1);
                assert_eq!(elseifs.len(), 1);
                assert_eq!(elseifs[0].body.len(), 1);
                let eb = else_body.as_ref().expect("else_body");
                assert_eq!(eb.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_block_else_if_split() {
        let src = "If c1 Then\n  a\nElse If c2 Then\n  b\nElse\n  c\nEndIf\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                then_body,
                elseifs,
                else_body,
                form,
                ..
            } => {
                assert_eq!(*form, IfForm::Block);
                assert_eq!(then_body.len(), 1);
                assert_eq!(elseifs.len(), 1);
                assert!(else_body.is_some());
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_block_end_if_split() {
        let src = "If c Then\n  a\nEnd If\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                form, then_body, ..
            } => {
                assert_eq!(*form, IfForm::Block);
                assert_eq!(then_body.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_single_line() {
        let src = "If x > 0 Then Print \"y\"\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                then_body,
                elseifs,
                else_body,
                form,
                ..
            } => {
                assert_eq!(*form, IfForm::SingleLine);
                assert_eq!(then_body.len(), 1);
                assert!(elseifs.is_empty());
                assert!(else_body.is_none());
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_single_line_else() {
        let src = "If r Then start() Else stop()\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                then_body,
                elseifs,
                else_body,
                form,
                ..
            } => {
                assert_eq!(*form, IfForm::SingleLine);
                assert_eq!(then_body.len(), 1);
                assert!(elseifs.is_empty());
                let eb = else_body.as_ref().expect("else_body");
                assert_eq!(eb.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_single_line_chained() {
        // §6.2: a single-line `If`'s `Then` arm can chain multiple statements
        // with `:`. Both `a = 1` and `b = 2` belong to the then-branch.
        let src = "If x Then a = 1 : b = 2\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                then_body, form, ..
            } => {
                assert_eq!(*form, IfForm::SingleLine);
                assert_eq!(then_body.len(), 2);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_single_line_chained_else() {
        // From cb_syntax.md §6.2 directly.
        let src = "If x > 0 Then a = 1 : b = 2 Else c = 3 : d = 4\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                then_body,
                else_body,
                form,
                ..
            } => {
                assert_eq!(*form, IfForm::SingleLine);
                assert_eq!(then_body.len(), 2);
                let eb = else_body.as_ref().expect("else body");
                assert_eq!(eb.len(), 2);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn if_single_line_elseif_errors() {
        let src = "If x Then a ElseIf y Then b\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_SINGLELINE_IF_DISALLOWS_ELSEIF)),
            "expected E0212, got {:?}",
            r.diagnostics
        );
        assert!(!r.program.is_empty());
    }

    #[test]
    fn if_block_missing_endif() {
        let src = "If x Then\n  Print 1\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_UNTERMINATED_BLOCK)),
            "expected E0203, got {:?}",
            r.diagnostics
        );
        // The E0203 should carry a secondary label at the `If` opener.
        let e0203 = r
            .diagnostics
            .iter()
            .find(|d| d.code == Some(E_UNTERMINATED_BLOCK))
            .expect("E0203 diagnostic");
        assert!(
            !e0203.secondary.is_empty(),
            "E0203 should have a secondary label at the opener, got: {e0203:?}"
        );
    }

    #[test]
    fn if_block_nested() {
        let src = "If a Then\n  If b Then\n    x = 1\n  EndIf\nEndIf\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::If {
                then_body, form, ..
            } => {
                assert_eq!(*form, IfForm::Block);
                assert_eq!(then_body.len(), 1);
                match stmt_of(&r.arena, then_body[0]) {
                    Stmt::If {
                        form: inner_form,
                        then_body: inner_body,
                        ..
                    } => {
                        assert_eq!(*inner_form, IfForm::Block);
                        assert_eq!(inner_body.len(), 1);
                    }
                    other => panic!("expected nested If, got {other:?}"),
                }
            }
            other => panic!("expected outer If, got {other:?}"),
        }
    }

    // ─── While ───────────────────────────────────────────────────────────

    #[test]
    fn while_simple() {
        let src = "While x\n  y\nWend\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::While { body, .. } => {
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected While, got {other:?}"),
        }
    }

    // ─── Repeat ──────────────────────────────────────────────────────────

    #[test]
    fn repeat_forever() {
        let src = "Repeat\n  a\nForever\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::RepeatForever { body } => {
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected RepeatForever, got {other:?}"),
        }
    }

    #[test]
    fn repeat_while() {
        let src = "Repeat\n  a\nWhile c\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::RepeatWhile { body, cond } => {
                assert_eq!(body.len(), 1);
                assert_ident(&r.arena, *cond, src, "c");
            }
            other => panic!("expected RepeatWhile, got {other:?}"),
        }
    }

    // ─── For ─────────────────────────────────────────────────────────────

    #[test]
    fn for_simple() {
        let src = "For i = 0 To 10\n  a\nNext i\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::For {
                body,
                step,
                next_name,
                ..
            } => {
                assert_eq!(body.len(), 1);
                assert!(step.is_none());
                assert_eq!(next_name.expect("next_name").slice(src), "i");
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    #[test]
    fn for_step() {
        let src = "For i = 0 To 10 Step 2\n  a\nNext i\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::For { step, .. } => {
                let s = step.expect("step");
                match expr_of(&r.arena, s) {
                    Expr::IntLit(v) => assert_eq!(*v, 2),
                    other => panic!("expected IntLit step, got {other:?}"),
                }
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    #[test]
    fn for_neg_step() {
        let src = "For i = 10 To 0 Step -1\n  a\nNext\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::For {
                step, next_name, ..
            } => {
                let s = step.expect("step");
                match expr_of(&r.arena, s) {
                    Expr::Unary { op, .. } => assert_eq!(*op, UnOp::Neg),
                    other => panic!("expected Unary Neg step, got {other:?}"),
                }
                assert!(next_name.is_none());
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    #[test]
    fn for_no_next_name() {
        let src = "For i = 0 To 10\n  a\nNext\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::For { next_name, .. } => assert!(next_name.is_none()),
            other => panic!("expected For, got {other:?}"),
        }
    }

    #[test]
    fn for_each_array() {
        let src = "For v = Each arr\n  a\nNext v\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::ForEach {
                var,
                source,
                body,
                next_name,
            } => {
                assert_ident(&r.arena, *var, src, "v");
                assert_ident(&r.arena, *source, src, "arr");
                assert_eq!(body.len(), 1);
                assert_eq!(next_name.expect("next_name").slice(src), "v");
            }
            other => panic!("expected ForEach, got {other:?}"),
        }
    }

    #[test]
    fn for_each_type() {
        let src = "For n = Each MyType\n  a\nNext n\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::ForEach {
                var,
                source,
                next_name,
                ..
            } => {
                assert_ident(&r.arena, *var, src, "n");
                assert_ident(&r.arena, *source, src, "MyType");
                assert_eq!(next_name.expect("next_name").slice(src), "n");
            }
            other => panic!("expected ForEach, got {other:?}"),
        }
    }

    // ─── Select ──────────────────────────────────────────────────────────

    #[test]
    fn select_basic() {
        let src = "Select x\n  Case 1\n    a\n  Case 2\n    b\nEndSelect\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Select { arms, .. } => {
                assert_eq!(arms.len(), 2);
                for arm_id in arms {
                    match case_arm_of(&r.arena, *arm_id) {
                        CaseArm::Case { body, .. } => assert_eq!(body.len(), 1),
                        other => panic!("expected Case arm, got {other:?}"),
                    }
                }
            }
            other => panic!("expected Select, got {other:?}"),
        }
    }

    #[test]
    fn select_with_default() {
        let src = "Select x\n  Case 1\n    a\n  Default\n    c\nEndSelect\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Select { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(
                    case_arm_of(&r.arena, arms[0]),
                    CaseArm::Case { .. }
                ));
                assert!(matches!(
                    case_arm_of(&r.arena, arms[1]),
                    CaseArm::Default { .. }
                ));
            }
            other => panic!("expected Select, got {other:?}"),
        }
    }

    #[test]
    fn select_default_first() {
        let src = "Select x\n  Default\n    c\n  Case 1\n    a\nEndSelect\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Select { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(
                    case_arm_of(&r.arena, arms[0]),
                    CaseArm::Default { .. }
                ));
                assert!(matches!(
                    case_arm_of(&r.arena, arms[1]),
                    CaseArm::Case { .. }
                ));
            }
            other => panic!("expected Select, got {other:?}"),
        }
    }

    #[test]
    fn select_continue_inside_case() {
        let src =
            "Select x\n  Case 30\n    Print \"x\"\n    Continue\n  Case 40\n    a\nEndSelect\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Select { arms, .. } => {
                assert_eq!(arms.len(), 2);
                match case_arm_of(&r.arena, arms[0]) {
                    CaseArm::Case { body, .. } => {
                        assert_eq!(body.len(), 2);
                        assert!(matches!(stmt_of(&r.arena, body[1]), Stmt::Continue));
                    }
                    other => panic!("expected Case arm, got {other:?}"),
                }
            }
            other => panic!("expected Select, got {other:?}"),
        }
    }

    // ─── Mismatched closers ──────────────────────────────────────────────

    #[test]
    fn mismatched_endif_for_while() {
        // `While x\n y\n EndIf\n`: the `EndIf` is wrong. Our parse_block_until
        // breaks on any `End*` keyword regardless of the expected closer, so
        // `parse_while`'s `consume_block_closer(Wend, …)` runs and emits E0204
        // mismatched-end.
        let src = "While x\n  y\nEndIf\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_MISMATCHED_END_KEYWORD)),
            "expected E0204 mismatched-end, got {:?}",
            r.diagnostics
        );
        // We still produce a Stmt::While node despite the mismatch.
        assert_eq!(r.program.len(), 1);
        assert!(matches!(
            stmt_of(&r.arena, r.program[0]),
            Stmt::While { .. }
        ));
    }
}

#[cfg(test)]
mod type_expr_tests {
    //! Unit tests for [`Parser::parse_type_expr`] — the type-expression
    //! grammar landed in W6.

    use super::*;
    use crate::ast::{Node, NodeId, Param, TypeExpr};
    use crate::span::FileId;
    use crate::{LexerOptions, tokenize};

    fn parse_type(src: &str) -> (Arena, NodeId, Vec<Diagnostic>) {
        let (tokens, lex_diags) = tokenize(src, FileId(0), LexerOptions::default());
        assert!(lex_diags.is_empty(), "lex diags: {lex_diags:?}");
        let mut parser = Parser::new(&tokens, src, FileId(0));
        let id = parser.parse_type_expr().expect("type parse failed");
        let result = parser.finish();
        (result.arena, id, result.diagnostics)
    }

    fn ty_of(arena: &Arena, id: NodeId) -> &TypeExpr {
        match &arena[id] {
            Node::TypeExpr(t) => t,
            other => panic!("expected TypeExpr, got {other:?}"),
        }
    }

    fn param_of(arena: &Arena, id: NodeId) -> &Param {
        match &arena[id] {
            Node::Param(p) => p,
            other => panic!("expected Param, got {other:?}"),
        }
    }

    fn assert_primitive(arena: &Arena, id: NodeId, expected: Kw) {
        match ty_of(arena, id) {
            TypeExpr::Primitive { kw } => assert_eq!(*kw, expected, "wrong primitive"),
            other => panic!("expected Primitive({expected:?}), got {other:?}"),
        }
    }

    #[test]
    fn primitive_integer() {
        let (arena, root, diags) = parse_type("Integer");
        assert!(diags.is_empty());
        assert_primitive(&arena, root, Kw::Integer);
    }

    #[test]
    fn primitive_float_etc() {
        for (src, expected) in [
            ("Byte", Kw::Byte),
            ("Short", Kw::Short),
            ("Integer", Kw::Integer),
            ("UInteger", Kw::UInteger),
            ("Long", Kw::Long),
            ("ULong", Kw::ULong),
            ("Float", Kw::Float),
            ("Bool", Kw::Bool),
            ("String", Kw::String),
        ] {
            let (arena, root, diags) = parse_type(src);
            assert!(diags.is_empty(), "{src}: {diags:?}");
            assert_primitive(&arena, root, expected);
        }
    }

    #[test]
    fn named_user_type() {
        let src = "MyType";
        let (arena, root, diags) = parse_type(src);
        assert!(diags.is_empty());
        match ty_of(&arena, root) {
            TypeExpr::Named { name_span } => assert_eq!(name_span.slice(src), "MyType"),
            other => panic!("expected Named, got {other:?}"),
        }
    }

    #[test]
    fn array_1d() {
        let (arena, root, diags) = parse_type("Integer[]");
        assert!(diags.is_empty());
        match ty_of(&arena, root) {
            TypeExpr::Array { elem, rank } => {
                assert_eq!(*rank, 1);
                assert_primitive(&arena, *elem, Kw::Integer);
            }
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn array_2d() {
        let (arena, root, diags) = parse_type("Float[,]");
        assert!(diags.is_empty());
        match ty_of(&arena, root) {
            TypeExpr::Array { elem, rank } => {
                assert_eq!(*rank, 2);
                assert_primitive(&arena, *elem, Kw::Float);
            }
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn array_3d() {
        let (arena, root, diags) = parse_type("String[,,]");
        assert!(diags.is_empty());
        match ty_of(&arena, root) {
            TypeExpr::Array { elem, rank } => {
                assert_eq!(*rank, 3);
                assert_primitive(&arena, *elem, Kw::String);
            }
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn paren_type() {
        let (arena, root, diags) = parse_type("(Integer)");
        assert!(diags.is_empty());
        match ty_of(&arena, root) {
            TypeExpr::Paren { inner } => assert_primitive(&arena, *inner, Kw::Integer),
            other => panic!("expected Paren, got {other:?}"),
        }
    }

    #[test]
    fn fn_ptr_no_ret() {
        let (arena, root, diags) = parse_type("Function()");
        assert!(diags.is_empty());
        match ty_of(&arena, root) {
            TypeExpr::FnPtr { params, ret } => {
                assert!(params.is_empty());
                assert!(ret.is_none());
            }
            other => panic!("expected FnPtr, got {other:?}"),
        }
    }

    #[test]
    fn fn_ptr_one_param_with_ret() {
        let (arena, root, diags) = parse_type("Function(Integer) As Float");
        assert!(diags.is_empty(), "{diags:?}");
        match ty_of(&arena, root) {
            TypeExpr::FnPtr { params, ret } => {
                assert_eq!(params.len(), 1);
                let p = param_of(&arena, params[0]);
                assert!(p.name_span.is_none());
                let ty = p.ty.expect("param ty");
                assert_primitive(&arena, ty, Kw::Integer);
                let r = ret.expect("ret");
                assert_primitive(&arena, r, Kw::Float);
            }
            other => panic!("expected FnPtr, got {other:?}"),
        }
    }

    #[test]
    fn fn_ptr_named_param() {
        let src = "Function(text As String, length As Float) As String";
        let (arena, root, diags) = parse_type(src);
        assert!(diags.is_empty(), "{diags:?}");
        match ty_of(&arena, root) {
            TypeExpr::FnPtr { params, ret } => {
                assert_eq!(params.len(), 2);
                let p0 = param_of(&arena, params[0]);
                assert_eq!(p0.name_span.expect("name").slice(src), "text");
                assert_primitive(&arena, p0.ty.unwrap(), Kw::String);
                let p1 = param_of(&arena, params[1]);
                assert_eq!(p1.name_span.expect("name").slice(src), "length");
                assert_primitive(&arena, p1.ty.unwrap(), Kw::Float);
                assert_primitive(&arena, ret.unwrap(), Kw::String);
            }
            other => panic!("expected FnPtr, got {other:?}"),
        }
    }

    #[test]
    fn fn_ptr_right_assoc_nested() {
        // Function(Integer) As Function(Float) As String
        // → outer FnPtr(Int → FnPtr(Float → String))
        let (arena, root, diags) = parse_type("Function(Integer) As Function(Float) As String");
        assert!(diags.is_empty(), "{diags:?}");
        let outer_ret = match ty_of(&arena, root) {
            TypeExpr::FnPtr { params, ret } => {
                assert_eq!(params.len(), 1);
                assert_primitive(&arena, param_of(&arena, params[0]).ty.unwrap(), Kw::Integer);
                ret.expect("outer ret")
            }
            other => panic!("expected outer FnPtr, got {other:?}"),
        };
        // Outer ret is itself FnPtr(Float → String).
        match ty_of(&arena, outer_ret) {
            TypeExpr::FnPtr { params, ret } => {
                assert_eq!(params.len(), 1);
                assert_primitive(&arena, param_of(&arena, params[0]).ty.unwrap(), Kw::Float);
                let r = ret.expect("inner ret");
                assert_primitive(&arena, r, Kw::String);
            }
            other => panic!("expected inner FnPtr, got {other:?}"),
        }
    }

    #[test]
    fn fn_ptr_paren_around_ret() {
        // Function(Integer) As (Function(Float) As String) — ret wrapped in Paren.
        let (arena, root, diags) = parse_type("Function(Integer) As (Function(Float) As String)");
        assert!(diags.is_empty(), "{diags:?}");
        let outer_ret = match ty_of(&arena, root) {
            TypeExpr::FnPtr { ret, .. } => ret.expect("outer ret"),
            other => panic!("expected outer FnPtr, got {other:?}"),
        };
        match ty_of(&arena, outer_ret) {
            TypeExpr::Paren { inner } => match ty_of(&arena, *inner) {
                TypeExpr::FnPtr { .. } => {}
                other => panic!("expected inner FnPtr, got {other:?}"),
            },
            other => panic!("expected Paren ret, got {other:?}"),
        }
    }

    #[test]
    fn fn_ptr_returning_array() {
        // Function(Integer) As Float[] → ret is Array(Float, 1).
        let (arena, root, diags) = parse_type("Function(Integer) As Float[]");
        assert!(diags.is_empty(), "{diags:?}");
        let ret = match ty_of(&arena, root) {
            TypeExpr::FnPtr { ret, .. } => ret.expect("ret"),
            other => panic!("expected FnPtr, got {other:?}"),
        };
        match ty_of(&arena, ret) {
            TypeExpr::Array { elem, rank } => {
                assert_eq!(*rank, 1);
                assert_primitive(&arena, *elem, Kw::Float);
            }
            other => panic!("expected Array ret, got {other:?}"),
        }
    }

    #[test]
    fn array_of_fn_ptrs() {
        // (Function(Integer) As Float)[] → Array(Paren(FnPtr(...)), 1).
        let (arena, root, diags) = parse_type("(Function(Integer) As Float)[]");
        assert!(diags.is_empty(), "{diags:?}");
        match ty_of(&arena, root) {
            TypeExpr::Array { elem, rank } => {
                assert_eq!(*rank, 1);
                match ty_of(&arena, *elem) {
                    TypeExpr::Paren { inner } => match ty_of(&arena, *inner) {
                        TypeExpr::FnPtr { .. } => {}
                        other => panic!("expected inner FnPtr, got {other:?}"),
                    },
                    other => panic!("expected Paren elem, got {other:?}"),
                }
            }
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn fn_ptr_anonymous_two_params() {
        let (arena, root, diags) = parse_type("Function(Integer, Float)");
        assert!(diags.is_empty(), "{diags:?}");
        match ty_of(&arena, root) {
            TypeExpr::FnPtr { params, ret } => {
                assert_eq!(params.len(), 2);
                assert!(ret.is_none());
                assert!(param_of(&arena, params[0]).name_span.is_none());
                assert_primitive(&arena, param_of(&arena, params[0]).ty.unwrap(), Kw::Integer);
                assert!(param_of(&arena, params[1]).name_span.is_none());
                assert_primitive(&arena, param_of(&arena, params[1]).ty.unwrap(), Kw::Float);
            }
            other => panic!("expected FnPtr, got {other:?}"),
        }
    }

    #[test]
    fn fn_ptr_sigil_only_params() {
        let src = "Function(text$, length#)";
        let (arena, root, diags) = parse_type(src);
        assert!(diags.is_empty(), "{diags:?}");
        match ty_of(&arena, root) {
            TypeExpr::FnPtr { params, ret } => {
                assert_eq!(params.len(), 2);
                assert!(ret.is_none());
                let p0 = param_of(&arena, params[0]);
                assert_eq!(p0.name_span.expect("name").slice(src), "text");
                assert_eq!(p0.sigil, Some(Sigil::String));
                assert!(p0.ty.is_none());
                let p1 = param_of(&arena, params[1]);
                assert_eq!(p1.name_span.expect("name").slice(src), "length");
                assert_eq!(p1.sigil, Some(Sigil::Float));
                assert!(p1.ty.is_none());
            }
            other => panic!("expected FnPtr, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod decl_tests {
    //! Unit tests for declaration statements (W6): `Function`, `Type`,
    //! `Struct`, `Dim`, `Global`, `Const`, `Redim`, and stray `Field`.

    use super::*;
    use crate::ast::{Expr, Node, NodeId, Param, Stmt, TypeExpr};
    use crate::span::FileId;
    use crate::token::StrLitKind;
    use crate::{LexerOptions, tokenize};

    fn parse_src(src: &str) -> ParseResult {
        let (tokens, lex_diags) = tokenize(src, FileId(0), LexerOptions::default());
        assert!(lex_diags.is_empty(), "lex diags: {lex_diags:?}");
        parse(&tokens, src, FileId(0))
    }

    fn stmt_of(arena: &Arena, id: NodeId) -> &Stmt {
        match &arena[id] {
            Node::Stmt(s) => s,
            other => panic!("expected Stmt, got {other:?}"),
        }
    }

    fn expr_of(arena: &Arena, id: NodeId) -> &Expr {
        match &arena[id] {
            Node::Expr(e) => e,
            other => panic!("expected Expr, got {other:?}"),
        }
    }

    fn ty_of(arena: &Arena, id: NodeId) -> &TypeExpr {
        match &arena[id] {
            Node::TypeExpr(t) => t,
            other => panic!("expected TypeExpr, got {other:?}"),
        }
    }

    fn param_of(arena: &Arena, id: NodeId) -> &Param {
        match &arena[id] {
            Node::Param(p) => p,
            other => panic!("expected Param, got {other:?}"),
        }
    }

    fn assert_primitive(arena: &Arena, id: NodeId, expected: Kw) {
        match ty_of(arena, id) {
            TypeExpr::Primitive { kw } => assert_eq!(*kw, expected),
            other => panic!("expected Primitive({expected:?}), got {other:?}"),
        }
    }

    fn assert_int(arena: &Arena, id: NodeId, expected: u64) {
        match expr_of(arena, id) {
            Expr::IntLit(v) => assert_eq!(*v, expected),
            other => panic!("expected IntLit({expected}), got {other:?}"),
        }
    }

    // ─── Dim / Global ────────────────────────────────────────────────────

    #[test]
    fn dim_simple() {
        let src = "Dim x As Integer\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Dim { names, ty, init } => {
                assert_eq!(names.len(), 1);
                assert_eq!(names[0].name_span.slice(src), "x");
                assert!(names[0].sigil.is_none());
                assert_primitive(&r.arena, ty.expect("ty"), Kw::Integer);
                assert!(init.is_none());
            }
            other => panic!("expected Dim, got {other:?}"),
        }
    }

    #[test]
    fn dim_with_sigil() {
        let src = "Dim count% As Integer\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Dim { names, ty, init } => {
                assert_eq!(names.len(), 1);
                assert_eq!(names[0].sigil, Some(Sigil::Integer));
                assert_eq!(names[0].name_span.slice(src), "count");
                assert_primitive(&r.arena, ty.expect("ty"), Kw::Integer);
                assert!(init.is_none());
            }
            other => panic!("expected Dim, got {other:?}"),
        }
    }

    #[test]
    fn dim_with_initializer() {
        let src = "Dim total# = 0.0\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Dim { names, ty, init } => {
                assert_eq!(names.len(), 1);
                assert_eq!(names[0].sigil, Some(Sigil::Float));
                assert!(ty.is_none());
                let init = init.expect("init");
                match expr_of(&r.arena, init) {
                    Expr::FloatLit(v) => assert!((v.to_f64() - 0.0).abs() < 1e-9),
                    other => panic!("expected FloatLit, got {other:?}"),
                }
            }
            other => panic!("expected Dim, got {other:?}"),
        }
    }

    #[test]
    fn dim_multi_name() {
        let src = "Dim a, b, c As Integer\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Dim { names, ty, init } => {
                assert_eq!(names.len(), 3);
                assert_eq!(names[0].name_span.slice(src), "a");
                assert_eq!(names[1].name_span.slice(src), "b");
                assert_eq!(names[2].name_span.slice(src), "c");
                assert_primitive(&r.arena, ty.expect("ty"), Kw::Integer);
                assert!(init.is_none());
            }
            other => panic!("expected Dim, got {other:?}"),
        }
    }

    #[test]
    fn dim_multi_name_with_init_errors() {
        let src = "Dim a, b As Integer = 0\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_MULTI_NAME_NOT_ALLOWED)),
            "expected E0210, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn global_simple() {
        let src = "Global score As Integer\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Global { names, ty, init } => {
                assert_eq!(names.len(), 1);
                assert_eq!(names[0].name_span.slice(src), "score");
                assert_primitive(&r.arena, ty.expect("ty"), Kw::Integer);
                assert!(init.is_none());
            }
            other => panic!("expected Global, got {other:?}"),
        }
    }

    #[test]
    fn global_multi_name() {
        let src = "Global a, b As Float\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Global { names, .. } => assert_eq!(names.len(), 2),
            other => panic!("expected Global, got {other:?}"),
        }
    }

    // ─── Const ──────────────────────────────────────────────────────────

    #[test]
    fn const_simple() {
        // Use 2.5 rather than 3.14… to dodge clippy::approx_constant.
        let src = "Const Pi# = 2.5\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Const {
                name_span,
                sigil,
                ty,
                value,
                is_global,
            } => {
                assert!(!*is_global);
                assert_eq!(name_span.slice(src), "Pi");
                assert_eq!(*sigil, Some(Sigil::Float));
                assert!(ty.is_none());
                match expr_of(&r.arena, *value) {
                    Expr::FloatLit(v) => assert!((v.to_f64() - 2.5).abs() < 1e-9),
                    other => panic!("expected FloatLit, got {other:?}"),
                }
            }
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn const_global() {
        let src = "Global Const Version$ = \"1.0.0\"\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Const {
                name_span,
                sigil,
                is_global,
                value,
                ..
            } => {
                assert!(*is_global);
                assert_eq!(name_span.slice(src), "Version");
                assert_eq!(*sigil, Some(Sigil::String));
                match expr_of(&r.arena, *value) {
                    Expr::StrLit { value, kind } => {
                        assert_eq!(value, "1.0.0");
                        assert_eq!(*kind, StrLitKind::Plain);
                    }
                    other => panic!("expected StrLit, got {other:?}"),
                }
            }
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn const_multi_name_errors() {
        let src = "Const A = 1, B = 2\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_MULTI_NAME_NOT_ALLOWED)),
            "expected E0210, got {:?}",
            r.diagnostics
        );
    }

    // ─── Redim ──────────────────────────────────────────────────────────

    #[test]
    fn redim_simple() {
        let src = "Redim arr As Float[100]\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Redim {
                target,
                elem_ty,
                dims,
            } => {
                match expr_of(&r.arena, *target) {
                    Expr::Ident { name_span, .. } => {
                        assert_eq!(name_span.slice(src), "arr")
                    }
                    other => panic!("expected Ident target, got {other:?}"),
                }
                assert_primitive(&r.arena, *elem_ty, Kw::Float);
                assert_eq!(dims.len(), 1);
                assert_int(&r.arena, dims[0], 100);
            }
            other => panic!("expected Redim, got {other:?}"),
        }
    }

    #[test]
    fn redim_2d() {
        let src = "Redim m As Float[3, 4]\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Redim { elem_ty, dims, .. } => {
                assert_primitive(&r.arena, *elem_ty, Kw::Float);
                assert_eq!(dims.len(), 2);
                assert_int(&r.arena, dims[0], 3);
                assert_int(&r.arena, dims[1], 4);
            }
            other => panic!("expected Redim, got {other:?}"),
        }
    }

    // ─── Field outside body ─────────────────────────────────────────────

    #[test]
    fn field_outside_type_body() {
        let src = "Field x As Integer\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_FIELD_OUTSIDE_TYPE_BODY)),
            "expected E0211, got {:?}",
            r.diagnostics
        );
    }

    /// FD-004 #7: the stray-`Field` recovery loop stops at `:` so a trailing
    /// statement on the same line still parses. The loop must NOT consume
    /// past the `:` separator.
    #[test]
    fn field_outside_type_body_stops_at_colon() {
        let src = "Field x : Print y\n";
        let r = parse_src(src);
        // One E0211 for the stray Field…
        assert_eq!(
            r.diagnostics
                .iter()
                .filter(|d| d.code == Some(E_FIELD_OUTSIDE_TYPE_BODY))
                .count(),
            1,
            "expected exactly one E0211, got {:?}",
            r.diagnostics,
        );
        // …and `Print y` recovered as a sibling ExprStmt (paren-less call).
        let print_stmt_id = r
            .program
            .iter()
            .find_map(|&id| match &r.arena[id] {
                Node::Stmt(Stmt::ExprStmt { expr }) => Some(*expr),
                _ => None,
            })
            .expect("expected a recovered ExprStmt for `Print y`");
        match &r.arena[print_stmt_id] {
            Node::Expr(Expr::Call { callee, args }) => {
                let callee_span = r.arena.span_of(*callee);
                assert_eq!(callee_span.slice(src), "Print");
                assert_eq!(args.len(), 1);
                let arg_span = r.arena.span_of(args[0]);
                assert_eq!(arg_span.slice(src), "y");
            }
            other => panic!("expected Call(Print, [y]), got {other:?}"),
        }
    }

    #[test]
    fn field_multi_name_in_body() {
        let src = "Type Foo\n  Field x, y As Integer\nEndType\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_MULTI_NAME_NOT_ALLOWED)),
            "expected E0210, got {:?}",
            r.diagnostics
        );
    }

    // ─── Function ────────────────────────────────────────────────────────

    #[test]
    fn function_simple() {
        let src = "Function f() As Integer\n  Return 0\nEndFunction\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Function {
                name_span,
                return_sigil,
                params,
                return_ty,
                body,
            } => {
                assert_eq!(name_span.slice(src), "f");
                assert!(return_sigil.is_none());
                assert!(params.is_empty());
                assert_primitive(&r.arena, return_ty.expect("ret"), Kw::Integer);
                assert_eq!(body.len(), 1);
                assert!(matches!(stmt_of(&r.arena, body[0]), Stmt::Return { .. }));
            }
            other => panic!("expected Function, got {other:?}"),
        }
    }

    #[test]
    fn function_with_params_and_ret() {
        let src = "Function area#(r As Float)\n  Return r * r\nEndFunction\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Function {
                name_span,
                return_sigil,
                params,
                return_ty,
                body,
            } => {
                assert_eq!(name_span.slice(src), "area");
                assert_eq!(*return_sigil, Some(Sigil::Float));
                assert_eq!(params.len(), 1);
                let p = param_of(&r.arena, params[0]);
                assert_eq!(p.name_span.expect("name").slice(src), "r");
                assert_primitive(&r.arena, p.ty.unwrap(), Kw::Float);
                assert!(return_ty.is_none()); // sigil supplies the return type
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected Function, got {other:?}"),
        }
    }

    #[test]
    fn function_default_value() {
        // Note: `step` is a keyword (used in `For … Step …`), so we name the
        // function `walk` instead. Defaults are allowed on trailing params.
        let src = "Function walk(distance#, count = 1) As Float\n  Return distance * count\nEndFunction\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Function { params, .. } => {
                assert_eq!(params.len(), 2);
                let p0 = param_of(&r.arena, params[0]);
                assert_eq!(p0.sigil, Some(Sigil::Float));
                assert!(p0.default.is_none());
                let p1 = param_of(&r.arena, params[1]);
                assert!(p1.sigil.is_none());
                let d = p1.default.expect("default");
                assert_int(&r.arena, d, 1);
            }
            other => panic!("expected Function, got {other:?}"),
        }
    }

    #[test]
    fn function_subroutine_form() {
        let src = "Function MySub(a#)\n  Print a\nEndFunction\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Function {
                return_ty,
                return_sigil,
                params,
                ..
            } => {
                assert!(return_ty.is_none());
                assert!(return_sigil.is_none());
                assert_eq!(params.len(), 1);
            }
            other => panic!("expected Function, got {other:?}"),
        }
    }

    #[test]
    fn function_end_function_split() {
        let src = "Function f()\n  Return\nEnd Function\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert!(matches!(
            stmt_of(&r.arena, r.program[0]),
            Stmt::Function { .. }
        ));
    }

    #[test]
    fn function_mismatched_end() {
        let src = "Function f()\nEndType\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_MISMATCHED_END_KEYWORD)),
            "expected E0204, got {:?}",
            r.diagnostics
        );
    }

    // ─── Type / Struct ──────────────────────────────────────────────────

    #[test]
    fn type_decl_simple() {
        let src = "Type Pt\n  Field x As Integer\n  Field y As Integer\nEndType\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.len(), 1);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Type { name_span, fields } => {
                assert_eq!(name_span.slice(src), "Pt");
                assert_eq!(fields.len(), 2);
                for &f in fields {
                    assert!(matches!(stmt_of(&r.arena, f), Stmt::FieldDecl { .. }));
                }
            }
            other => panic!("expected Type, got {other:?}"),
        }
    }

    #[test]
    fn type_decl_end_type_split() {
        let src = "Type Pt\n  Field x As Integer\nEnd Type\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert!(matches!(stmt_of(&r.arena, r.program[0]), Stmt::Type { .. }));
    }

    #[test]
    fn struct_decl_simple() {
        let src = "Struct V\n  Field x As Float\n  Field y As Float\nEndStruct\n";
        let r = parse_src(src);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        match stmt_of(&r.arena, r.program[0]) {
            Stmt::Struct { name_span, fields } => {
                assert_eq!(name_span.slice(src), "V");
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected Struct, got {other:?}"),
        }
    }

    #[test]
    fn type_decl_bad_body_stmt() {
        // `Print "x"` is not a `Field`; expect a diagnostic and recovery so
        // the surrounding Type node still parses.
        let src = "Type Foo\n  Print \"x\"\nEndType\n";
        let r = parse_src(src);
        assert!(
            !r.diagnostics.is_empty(),
            "expected diagnostic for non-Field in Type body"
        );
        assert!(matches!(stmt_of(&r.arena, r.program[0]), Stmt::Type { .. }));
    }
}

#[cfg(test)]
mod recovery_tests {
    //! W7 error-recovery integration tests. These exercise the cases the
    //! recovery loop is most likely to mishandle: a sync-target keyword
    //! appearing at top level (used to loop forever — see the forced-progress
    //! guard in `parse_stmt`), a missing block terminator, a missing `Then`
    //! after `If`, and a leading run of garbage tokens before a real
    //! statement.

    use super::*;
    use crate::ast::{Node, NodeId, Stmt};
    use crate::span::FileId;
    use crate::{LexerOptions, tokenize};

    fn parse_src(src: &str) -> ParseResult {
        let (tokens, _lex_diags) = tokenize(src, FileId(0), LexerOptions::default());
        parse(&tokens, src, FileId(0))
    }

    fn stmt_of(arena: &Arena, id: NodeId) -> &Stmt {
        match &arena[id] {
            Node::Stmt(s) => s,
            other => panic!("expected Stmt, got {other:?}"),
        }
    }

    #[test]
    fn recovery_endfunction_at_top_level_does_not_loop() {
        // `EndFunction` is in the statement-level sync set (it's a block
        // closer). Before the W7 forced-progress guard, when it appeared at
        // top level `parse_stmt_inner` would error on it, `sync_to_stmt_boundary`
        // would stop *at* it (without consuming, since `consume_block_closer`
        // is the rightful consumer), and the next `parse_stmt` call would do
        // the same thing forever. The guard now bumps the offending token once
        // so the loop terminates and the rest of the file is still parsed.
        let src = "EndFunction\nx = 1\n";
        let r = parse_src(src);
        assert!(!r.diagnostics.is_empty(), "expected at least one diag");
        // The `x = 1` statement should still appear.
        assert!(
            r.program
                .iter()
                .any(|&id| matches!(stmt_of(&r.arena, id), Stmt::Assign { .. })),
            "expected an Assign node after recovery, got program: {:?}",
            r.program
        );
    }

    #[test]
    fn recovery_missing_then_in_if() {
        // `If x Print "y"` — `Then` is missing. `expect_kw(Then)` raises an
        // E0201, the parser syncs at the next stmt boundary, and `z = 1`
        // parses cleanly on the next line.
        let src = "If x Print \"y\"\nz = 1\n";
        let r = parse_src(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == Some(E_EXPECTED_TOKEN)),
            "expected E0201 (missing `Then`), got {:?}",
            r.diagnostics
        );
        // `z = 1` must still parse cleanly.
        let program_kinds: Vec<&Stmt> = r.program.iter().map(|&id| stmt_of(&r.arena, id)).collect();
        assert!(
            program_kinds
                .iter()
                .any(|s| matches!(s, Stmt::Assign { .. })),
            "expected an Assign after recovery; program statements were {program_kinds:?}",
        );
    }

    #[test]
    fn recovery_unterminated_function_at_eof() {
        // `Function f() … <EOF>` — `EndFunction` is missing. `parse_block_until`
        // hits EOF, emits E0203 with a secondary label at the `Function` opener,
        // and the surrounding `Stmt::Function` is still produced.
        let src = "Function f()\n  Print 1\n";
        let r = parse_src(src);
        let e0203 = r
            .diagnostics
            .iter()
            .find(|d| d.code == Some(E_UNTERMINATED_BLOCK))
            .expect("expected E0203 unterminated-block diagnostic");
        assert!(
            !e0203.secondary.is_empty(),
            "E0203 should carry a secondary label at the `Function` opener, got {e0203:?}"
        );
        // The function node still landed in `program`.
        assert!(
            r.program
                .iter()
                .any(|&id| matches!(stmt_of(&r.arena, id), Stmt::Function { .. })),
            "expected a Function node despite the missing EndFunction"
        );
    }

    #[test]
    fn recovery_garbage_then_function() {
        // Leading lexical / unexpected tokens before a real `Function` decl.
        // Parser emits at least one diagnostic and the `Function f` declaration
        // still ends up in `program`.
        let src = "# %% ! garbage tokens\nFunction f()\n  Return 0\nEndFunction\n";
        let r = parse_src(src);
        assert!(
            !r.diagnostics.is_empty(),
            "expected at least one diagnostic for garbage tokens"
        );
        assert!(
            r.program
                .iter()
                .any(|&id| matches!(stmt_of(&r.arena, id), Stmt::Function { .. })),
            "expected a Function node after recovery; program was {:?}",
            r.program
        );
    }
}
