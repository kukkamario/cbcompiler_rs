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
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// The kind of a [`Token`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TokenKind {
    /// Identifier; lexeme recovered from source via `span`. `sigil` is the
    /// trailing type sigil (`%`, `#`, `$`, `!`) if any. The sigil's bytes are
    /// INCLUDED in `Token::span`; consumers can subtract the sigil byte from
    /// the span end to get the bare-name span.
    Ident {
        sigil: Option<Sigil>,
    },
    Keyword(Kw),
    /// Unsigned-magnitude integer literal value. The lexer is intentionally
    /// type-agnostic — range-checking against the inferred signed/unsigned
    /// target type is sema's job (§3.4 of `cb_syntax.md`).
    IntLit(u64),
    /// Float literal stored as raw IEEE-754 bits so [`Token`] can be `Eq`.
    /// Bit-equality treats two NaNs with the same payload as equal, which is
    /// the right thing for parser-side token comparisons.
    FloatLit(FloatBits),
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

/// Bit-pattern wrapper around an IEEE-754 `f64`. Used by
/// [`TokenKind::FloatLit`] so [`Token`] can be `Eq` and `Hash` — equality is
/// raw-bit equality, which treats two NaNs with the same payload as equal
/// (and two NaNs with different payloads as unequal). This is what callers
/// comparing tokens actually want; the IEEE "NaN != NaN" rule is the wrong
/// default at the lexer level.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct FloatBits(u64);

impl FloatBits {
    pub const fn from_f64(v: f64) -> Self {
        Self(v.to_bits())
    }

    pub const fn to_f64(self) -> f64 {
        f64::from_bits(self.0)
    }

    pub const fn to_bits(self) -> u64 {
        self.0
    }
}

impl std::fmt::Debug for FloatBits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Use Debug for the inner `f64` so scientific notation survives for
        // very large / very small values (e.g. `1.24e24`, `1e-7`).
        write!(f, "FloatBits({:?})", self.to_f64())
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
    /// Numeric literal is structurally malformed *or* exceeds the lexer's
    /// representable range (`u64` for integers, finite `f64` for floats).
    /// Range-checking against the inferred signed target type is sema's job;
    /// the lexer only flags values no type could represent.
    MalformedNumber,
    /// Bare character the scanner doesn't recognize, or a character that
    /// can't start any token in CoolBasic.
    UnexpectedChar,
}

#[cfg(test)]
mod tests {}
