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

pub use inst::{InstKind, IrBinOp, IrUnOp, PlaceRoot, Projection, Terminator, TrapKind};
pub use types::{FnSig, IrType};

// ── Function identity ──────────────────────────────────────────────────

/// Numeric handle for a function (user-defined or runtime-provided).
/// Indexes into `Program::func_table`.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct FuncId(pub u32);

impl fmt::Display for FuncId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "func{}", self.0)
    }
}

/// Declaration of a function known to the program.
#[derive(Clone, Debug)]
pub struct FuncDecl {
    pub name: Symbol,
    pub sig: FnSig,
    pub kind: FuncKind,
}

/// Whether a function is user-defined (has an IR body) or runtime-provided.
#[derive(Clone, Debug)]
pub enum FuncKind {
    UserDefined { body_index: usize },
    Runtime {
        symbol: String,
        /// Statically-linked address of the runtime function. The interpreter
        /// dispatches through this; the LLVM backend uses `symbol` for `declare`/`call`.
        fn_ptr: unsafe extern "C" fn(),
    },
}

// ── Runtime catalog types ──────────────────────────────────────────────

/// Description of a runtime-provided function, produced by the catalog loader
/// and consumed by sema. Uses [`IrType`] so that both the FFI crate
/// (`cb-runtime-sys`) and the semantic analysis crate (`cb-sema`) can share
/// these types without depending on each other.
pub struct FuncDesc {
    pub name: String,
    pub c_symbol: String,
    /// Statically-linked address of the runtime function. Populated by the
    /// catalog loader; used by the interpreter for libffi dispatch.
    pub fn_ptr: unsafe extern "C" fn(),
    pub params: Vec<FuncParamDesc>,
    pub return_ty: IrType,
}

/// A parameter in a runtime function description.
pub struct FuncParamDesc {
    pub name: Option<String>,
    pub ty: IrType,
}

/// Description of an opaque type declared by the runtime catalog.
pub struct RuntimeTypeDesc {
    pub name: String,
    pub tag: u32,
}

/// The full runtime catalog: type declarations and function descriptors.
pub struct RuntimeCatalog {
    pub types: Vec<RuntimeTypeDesc>,
    pub functions: Vec<FuncDesc>,
}

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

/// Index into `Program::type_defs`.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct TypeDefId(pub u32);

/// Index into `Program::globals`.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct GlobalId(pub u32);

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

impl fmt::Display for TypeDefId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "typedef{}", self.0)
    }
}

impl fmt::Display for GlobalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "global{}", self.0)
    }
}

// ── IR structures ───────────────────────────────────────────────────────

/// A local variable slot (parameters + Dim declarations).
#[derive(Clone, Debug)]
pub struct Local {
    pub name: Symbol,
    pub ty: IrType,
    pub is_param: bool,
}

/// A global variable slot. Indexed by `GlobalId`.
#[derive(Clone, Debug)]
pub struct Global {
    pub name: Symbol,
    pub ty: IrType,
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
    pub terminator_span: Span,
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
    /// All known functions (user-defined + runtime). `FuncId` indexes this.
    pub func_table: Vec<FuncDecl>,
    /// Bodies of user-defined functions. `FuncKind::UserDefined::body_index`
    /// indexes this.
    pub functions: Vec<Function>,
    /// Program-wide global variables. `GlobalId` indexes this.
    pub globals: Vec<Global>,
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
