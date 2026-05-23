//! IR instruction kinds, operators, terminators, and traps.

use cb_diagnostics::Symbol;

use crate::types::IrType;
use crate::{BlockId, LocalId, Reg};

/// The operation performed by an instruction.
#[derive(Clone, Debug, PartialEq)]
pub enum InstKind {
    // ── Arithmetic & Logic ──────────────────────────────────────────
    BinOp { op: IrBinOp, lhs: Reg, rhs: Reg },
    UnOp { op: IrUnOp, operand: Reg },

    // ── Memory & Variables ──────────────────────────────────────────
    LoadLocal { local: LocalId },
    StoreLocal { local: LocalId, value: Reg },

    NewType { type_name: Symbol },
    NewArray { elem_type: IrType, dims: Vec<Reg> },

    GetField { object: Reg, field: Symbol, field_type: IrType },
    SetField { object: Reg, field: Symbol, value: Reg },

    GetElement { array: Reg, indices: Vec<Reg> },
    SetElement { array: Reg, indices: Vec<Reg>, value: Reg },

    // ── Type-Linked-List Operations ─────────────────────────────────
    First { type_name: Symbol },
    Last { type_name: Symbol },
    Next { object: Reg },
    Previous { object: Reg },

    DeleteLvalue { local: LocalId },
    DeleteRvalue { value: Reg },

    // ── Compiler Intrinsics ─────────────────────────────────────────
    Len { array: Reg, dim: Option<Reg> },
    ConvertExplicit { value: Reg, target: IrType },
    Convert { value: Reg, from: IrType, to: IrType },

    // ── Function Calls ──────────────────────────────────────────────
    Call { callee: Symbol, args: Vec<Reg> },
    CallIndirect { callee: Reg, args: Vec<Reg> },

    // ── Constants ───────────────────────────────────────────────────
    ConstInt(i64),
    ConstFloat(f64),
    ConstBool(bool),
    ConstString(String),
    ConstNull,

    // ── Array ───────────────────────────────────────────────────────
    Redim { local: LocalId, elem_type: IrType, dims: Vec<Reg> },
}

/// Binary operators in the IR.
///
/// String operations are distinct opcodes (not overloaded on `Add`/`Eq`)
/// because sema has already resolved operand types.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IrBinOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    IntDiv,
    Mod,
    Pow,
    // Bitwise
    BinAnd,
    BinOr,
    BinXor,
    Shl,
    Shr,
    Sar,
    // Comparison (result is always Bool)
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    // String
    StrConcat,
    StrEq,
    StrNotEq,
    StrLt,
    StrGt,
    StrLtEq,
    StrGtEq,
}

/// Unary operators in the IR.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IrUnOp {
    Neg,
    Plus,
    Not,
    BinNot,
}

/// Every basic block ends with exactly one terminator.
#[derive(Clone, Debug, PartialEq)]
pub enum Terminator {
    Goto(BlockId),
    BranchIf {
        cond: Reg,
        then_block: BlockId,
        else_block: BlockId,
    },
    Return {
        value: Option<Reg>,
    },
    Trap(TrapKind),
}

/// Runtime trap kinds (unreachable in correct programs).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TrapKind {
    NullDeref,
    DeletedAccess,
    DivisionByZero,
    IndexOutOfBounds,
    NullFnPtr,
    DoubleDelete,
}
