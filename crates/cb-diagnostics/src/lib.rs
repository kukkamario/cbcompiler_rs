//! Structured diagnostics for the CoolBasic compiler.
//!
//! Frontend code produces [`Diagnostic`]s; renderers (CLI today, LSP/JSON later)
//! consume them via the [`Renderer`] trait. The crate has no dependency on
//! `cb-frontend`; both frontend and renderers depend on this.

pub mod diagnostic;
pub mod render;
pub mod source;

pub use diagnostic::{Diagnostic, Label, Severity, Span};
pub use render::{CliRenderer, Renderer};
pub use source::{FileId, LineIndex, Source, SourceMap};
