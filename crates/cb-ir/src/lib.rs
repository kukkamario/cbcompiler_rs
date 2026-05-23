//! Backend-agnostic intermediate representation for CoolBasic.
//!
//! Both [`cb_backend_interp`] and [`cb_backend_llvm`] consume this IR.
//! Do not leak backend-specific types (LLVM, etc.) into this crate.

use std::fmt;

use cb_diagnostics::{Span, Symbol};

pub mod inst;
pub mod print;
pub mod types;
pub mod verify;

pub use inst::{InstKind, IrBinOp, IrUnOp, Terminator, TrapKind};
pub use types::{FnSig, IrType};

// ── ID newtypes ─────────────────────────────────────────────────────────

/// Virtual register — assigned by instructions, consumed by operands.
/// Function-scoped.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Reg(pub u32);

/// Index into a function's `locals` vector.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct LocalId(pub u32);

/// Block identifier within a function.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct BlockId(pub u32);

impl fmt::Display for Reg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

impl fmt::Display for LocalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "local{}", self.0)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

// ── IR structures ───────────────────────────────────────────────────────

/// A local variable slot (parameters + Dim/Global declarations).
#[derive(Clone, Debug)]
pub struct Local {
    pub name: Symbol,
    pub ty: IrType,
    pub is_global: bool,
    pub is_param: bool,
}

/// A single IR instruction.
#[derive(Clone, Debug)]
pub struct Inst {
    /// Register that receives the result, if any.
    pub result: Option<Reg>,
    pub kind: InstKind,
    pub span: Span,
}

/// A basic block: a straight-line sequence of instructions ending with a
/// terminator.
#[derive(Clone, Debug)]
pub struct BasicBlock {
    pub id: BlockId,
    pub insts: Vec<Inst>,
    pub terminator: Option<Terminator>,
}

/// An IR function. Top-level code is wrapped in a synthetic `@main`.
#[derive(Clone, Debug)]
pub struct Function {
    pub name: Symbol,
    pub params: Vec<IrType>,
    pub return_type: IrType,
    pub locals: Vec<Local>,
    pub blocks: Vec<BasicBlock>,
}

/// Top-level IR program.
#[derive(Clone, Debug)]
pub struct Program {
    pub functions: Vec<Function>,
    pub type_defs: Vec<TypeDefInfo>,
    pub struct_defs: Vec<StructDefInfo>,
}

/// Metadata about a user-defined `Type` (for backend use).
#[derive(Clone, Debug)]
pub struct TypeDefInfo {
    pub name: Symbol,
    pub fields: Vec<(Symbol, IrType)>,
}

/// Metadata about a user-defined `Struct` (for backend use).
#[derive(Clone, Debug)]
pub struct StructDefInfo {
    pub name: Symbol,
    pub fields: Vec<(Symbol, IrType)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_ids() {
        assert_eq!(Reg(0).to_string(), "r0");
        assert_eq!(Reg(42).to_string(), "r42");
        assert_eq!(LocalId(3).to_string(), "local3");
        assert_eq!(BlockId(7).to_string(), "bb7");
    }

    #[test]
    fn ir_type_predicates() {
        assert!(IrType::Int.is_numeric());
        assert!(IrType::Float.is_numeric());
        assert!(IrType::Byte.is_integer());
        assert!(!IrType::Float.is_integer());
        assert!(!IrType::String.is_numeric());
        assert!(!IrType::Bool.is_numeric());
    }
}
