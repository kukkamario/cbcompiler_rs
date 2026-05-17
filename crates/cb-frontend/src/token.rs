//! Token types produced by the lexer.
//!
//! Design notes (see FD-001 and `docs/cb_syntax.md`):
//! - A trailing type sigil (`%`, `#`, `$`, `!`) is folded into the `Ident`
//!   token via [`TokenKind::Ident`]'s `sigil` field, not emitted as a
//!   separate token. The sigil's byte is included in [`Token::span`].
//! - Comments cover `//` and `REM` (line) and `/* … */` (block, nested).
//! - Keyword operators (`And`, `Or`, `Mod`, `Shl`, …) lex as
//!   [`TokenKind::Keyword`], not [`TokenKind::Op`]; the parser handles their
//!   operator role.

use crate::span::Span;

/// A lexed token: a kind plus the source span it came from.
///
/// Not `Eq` because [`TokenKind::FloatLit`] carries an `f64`; use
/// `PartialEq` and treat NaN as not equal to itself.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// The kind of a [`Token`].
///
/// Not `Eq` (see [`Token`]).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// Identifier; lexeme recovered from source via `span`. `sigil` is the
    /// trailing type sigil (`%`, `#`, `$`, `!`) if any. The sigil's bytes are
    /// INCLUDED in `Token::span`; consumers can subtract the sigil byte from
    /// the span end to get the bare-name span.
    Ident {
        sigil: Option<Sigil>,
    },
    Keyword(Kw),
    IntLit(i64),
    FloatLit(f64),
    StrLit(StrLitKind),
    Punct(Punct),
    Op(Op),
    /// `\n`, `\r`, or `\r\n` — significant as a statement terminator.
    Newline,
    /// `\` followed by optional whitespace and a line ending. Usually
    /// suppressed; emitted only in trivia-preserving mode.
    Continuation,
    /// Whitespace run (spaces and tabs). Trivia, only emitted in
    /// trivia-preserving mode.
    Whitespace,
    Comment(CommentKind),
    Error(LexErrorKind),
    Eof,
}

/// Type sigil that may trail an identifier (§1.4 of `cb_syntax.md`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Sigil {
    /// `%` — Integer
    Integer,
    /// `#` — Float
    Float,
    /// `$` — String
    String,
    /// `!` — Bool
    Bool,
}

impl Sigil {
    /// The single-character spelling of the sigil.
    pub const fn as_char(self) -> char {
        match self {
            Sigil::Integer => '%',
            Sigil::Float => '#',
            Sigil::String => '$',
            Sigil::Bool => '!',
        }
    }
}

/// CoolBasic keyword. See §1.5 of `cb_syntax.md` for the full list.
///
/// `Int`/`Integer` are aliases that both map to [`Kw::Integer`]; same for
/// `UInt`/`UInteger` → [`Kw::UInteger`]. `REM` is NOT a keyword here — it
/// is recognised as a line-comment marker by the lexer (§1.2).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Kw {
    And,
    As,
    BinAnd,
    BinNot,
    BinOr,
    BinXor,
    Bool,
    Break,
    Byte,
    Case,
    Const,
    Continue,
    Default,
    Dim,
    Each,
    Else,
    ElseIf,
    End,
    EndFunction,
    EndIf,
    EndSelect,
    EndStruct,
    EndType,
    False,
    Field,
    Float,
    For,
    Forever,
    Function,
    Global,
    Goto,
    If,
    Include,
    Integer,
    Long,
    Mod,
    New,
    Next,
    Not,
    Null,
    Or,
    Redim,
    Repeat,
    Return,
    Sar,
    Select,
    Shl,
    Short,
    Shr,
    Step,
    String,
    Struct,
    Then,
    To,
    True,
    Type,
    UInteger,
    ULong,
    Wend,
    While,
    Xor,
}

impl Kw {
    /// Canonical lowercase spelling, suitable for diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Kw::And => "and",
            Kw::As => "as",
            Kw::BinAnd => "binand",
            Kw::BinNot => "binnot",
            Kw::BinOr => "binor",
            Kw::BinXor => "binxor",
            Kw::Bool => "bool",
            Kw::Break => "break",
            Kw::Byte => "byte",
            Kw::Case => "case",
            Kw::Const => "const",
            Kw::Continue => "continue",
            Kw::Default => "default",
            Kw::Dim => "dim",
            Kw::Each => "each",
            Kw::Else => "else",
            Kw::ElseIf => "elseif",
            Kw::End => "end",
            Kw::EndFunction => "endfunction",
            Kw::EndIf => "endif",
            Kw::EndSelect => "endselect",
            Kw::EndStruct => "endstruct",
            Kw::EndType => "endtype",
            Kw::False => "false",
            Kw::Field => "field",
            Kw::Float => "float",
            Kw::For => "for",
            Kw::Forever => "forever",
            Kw::Function => "function",
            Kw::Global => "global",
            Kw::Goto => "goto",
            Kw::If => "if",
            Kw::Include => "include",
            Kw::Integer => "integer",
            Kw::Long => "long",
            Kw::Mod => "mod",
            Kw::New => "new",
            Kw::Next => "next",
            Kw::Not => "not",
            Kw::Null => "null",
            Kw::Or => "or",
            Kw::Redim => "redim",
            Kw::Repeat => "repeat",
            Kw::Return => "return",
            Kw::Sar => "sar",
            Kw::Select => "select",
            Kw::Shl => "shl",
            Kw::Short => "short",
            Kw::Shr => "shr",
            Kw::Step => "step",
            Kw::String => "string",
            Kw::Struct => "struct",
            Kw::Then => "then",
            Kw::To => "to",
            Kw::True => "true",
            Kw::Type => "type",
            Kw::UInteger => "uinteger",
            Kw::ULong => "ulong",
            Kw::Wend => "wend",
            Kw::While => "while",
            Kw::Xor => "xor",
        }
    }
}

/// Non-keyword operator tokens. Keyword operators (`And`, `Or`, `Xor`,
/// `Not`, `Mod`, `BinAnd`, `BinOr`, `BinXor`, `BinNot`, `Shl`, `Shr`,
/// `Sar`) lex as [`TokenKind::Keyword`], not [`Op`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Op {
    Plus,
    Minus,
    Star,
    Slash,
    BackSlash,
    StarStar,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

/// Punctuation tokens. The statement separator `:` is [`Punct::Colon`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Punct {
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semicolon,
    Dot,
}

/// String-literal flavour. Drives the unescape / value-extraction step
/// downstream of the lexer.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StrLitKind {
    /// Single-line `"..."`, no `\` seen — the value is the body verbatim.
    Plain,
    /// Single-line `"..."`, contains at least one `\` — needs an unescape pass.
    Escaped,
    /// Triple-quoted `"""…"""`, raw multi-line — no escapes, indent stripping
    /// deferred to the parser/sema.
    Raw,
}

/// Comment flavour.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CommentKind {
    /// `//` or `REM` to end of line.
    Line,
    /// `/* ... */`, nested.
    Block,
}

/// Lexical error attached to an [`TokenKind::Error`] token. The lexer
/// recovers locally and keeps producing tokens; the driver collects these
/// for diagnostic emission.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LexErrorKind {
    /// Newline appeared inside `"..."`. Recover by closing the string at the newline.
    NewlineInString,
    /// `"..."` or `"""..."""` reached EOF without a closer.
    UnterminatedString,
    /// `/* ... */` reached EOF with depth > 0.
    UnterminatedBlockComment,
    /// Underscore at an invalid position in a numeric literal (adjacent to a
    /// prefix, doubled, or trailing).
    InvalidDigitSeparator,
    /// Numeric literal value exceeds `i64`/`f64` range.
    NumberOverflow,
    /// Numeric literal is structurally malformed (e.g. exponent with no
    /// digits — `1e`, `1e+`). Distinct from `NumberOverflow` (value out of
    /// range) and `InvalidDigitSeparator` (underscore placement).
    MalformedNumber,
    /// Bare character the scanner doesn't recognize.
    UnexpectedChar,
    /// A character that can never start a token in CoolBasic (e.g. `@`, `~`).
    /// Distinct from UnexpectedChar in case we want a different message later.
    InvalidChar,
}

#[cfg(test)]
mod tests {}
