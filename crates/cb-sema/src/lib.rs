//! Semantic analysis for CoolBasic: name resolution, type checking, and
//! implicit conversion insertion.
//!
//! Consumes a parsed AST ([`cb_frontend::Arena`] + program) and produces a
//! [`SemaResult`] with type annotations, symbol tables, and diagnostics.

use std::collections::HashMap;

use cb_diagnostics::{Diagnostic, FileId};
use cb_frontend::{Arena, NodeId};

mod check;
mod convert;
mod diagnostics;
mod scope;
mod types;

pub use convert::{Conversion, ConversionTable};
pub use scope::{
    ConstValue, DeclKind, Declaration, FieldInfo, ParamInfo, ScopeId, ScopeKind, SymbolTable,
};
pub use types::Type;

/// Whether a Delete operand is an lvalue (variable/field/index) or rvalue.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DeleteClass {
    Lvalue,
    Rvalue,
}

/// Run semantic analysis on a parsed program.
///
/// Returns type annotations, symbol tables, implicit conversions, and any
/// diagnostics (errors and warnings). The caller should check
/// `result.has_errors()` before proceeding to IR lowering.
pub fn analyze(
    arena: &Arena,
    program: &[NodeId],
    source: &str,
    file_id: FileId,
) -> SemaResult {
    check::Checker::run(arena, program, source, file_id)
}

/// Result of semantic analysis.
pub struct SemaResult {
    /// Resolved type for each expression and variable node.
    pub types: TypeTable,
    /// Scope tree with all declarations.
    pub symbols: SymbolTable,
    /// Implicit conversions inserted by the type checker.
    pub conversions: ConversionTable,
    /// Delete lvalue/rvalue classification for each `Stmt::Delete` node.
    pub delete_classes: HashMap<NodeId, DeleteClass>,
    /// Diagnostics produced during analysis.
    pub diagnostics: Vec<Diagnostic>,
}

impl SemaResult {
    /// Whether any error-severity diagnostics were produced.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| matches!(d.severity, cb_diagnostics::Severity::Error))
    }
}

/// Maps AST nodes to their resolved types.
pub struct TypeTable {
    entries: HashMap<NodeId, Type>,
}

impl TypeTable {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, id: NodeId, ty: Type) {
        self.entries.insert(id, ty);
    }

    /// Look up the resolved type of a node.
    pub fn get(&self, id: NodeId) -> Option<&Type> {
        self.entries.get(&id)
    }
}
