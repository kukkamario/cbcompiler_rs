//! CoolBasic frontend: lexer, parser, AST, semantic analysis.
//!
//! Backend-agnostic. Lowering to IR happens in [`cb_ir`].

pub mod ast;
pub mod keywords;
pub mod lexer;
pub mod parser;
pub mod span;
mod string_value;
pub mod token;

pub use ast::{
    Arena, BinOp, CaseArm, DimName, ElseIf, Expr, IfForm, NewKind, Node, NodeId, Param, Stmt,
    TypeExpr, UnOp,
};
pub use lexer::{LexerOptions, tokenize};
pub use parser::{ParseResult, parse};
pub use span::{FileId, Span, SpanExt};
pub use token::{
    CommentKind, FloatBits, Kw, LexErrorKind, Op, Punct, Sigil, StrLitKind, Token, TokenKind,
};
