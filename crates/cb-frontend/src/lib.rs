//! CoolBasic frontend: lexer, parser, AST, semantic analysis.
//!
//! Backend-agnostic. Lowering to IR happens in [`cb_ir`].

pub mod keywords;
pub mod lexer;
pub mod span;
pub mod token;

pub use lexer::{LexerOptions, tokenize};
pub use span::{FileId, Span, SpanExt};
pub use token::{CommentKind, Kw, LexErrorKind, Op, Punct, Sigil, StrLitKind, Token, TokenKind};
