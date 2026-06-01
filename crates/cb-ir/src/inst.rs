//! IR instruction kinds, operators, terminators, and traps.

use cb_diagnostics::Symbol;

use crate::types::IrType;
use crate::{BlockId, FuncId, GlobalId, LocalId, Reg, TypeDefId};

/// The operation performed by an instruction.
#[derive(Clone, Debug, PartialEq)]
pub enum InstKind {
    // ── Arithmetic & Logic ──────────────────────────────────────────
    BinOp { op: IrBinOp, lhs: Reg, rhs: Reg },
    UnOp { op: IrUnOp, operand: Reg },

    // ── Memory & Variables ──────────────────────────────────────────
    LoadLocal { local: LocalId },
    StoreLocal { local: LocalId, value: Reg },
    LoadGlobal { global: GlobalId },
    StoreGlobal { global: GlobalId, value: Reg },

    NewType { type_def: TypeDefId },
    NewArray { elem_type: IrType, dims: Vec<Reg> },

    GetField { object: Reg, field: Symbol, field_type: IrType },

    GetElement { array: Reg, indices: Vec<Reg> },

    /// Store `value` into the place rooted at a local/global variable,
    /// following a chain of field/index projections and mutating in place.
    ///
    /// This is the single write path for any assignment target more complex
    /// than a bare variable (`s.x = v`, `s.a.b = v`, `arr[i] = v`,
    /// `arr[i].field = v`, `obj.field = v`). Because value-type structs are
    /// stored inline, a register-based `SetField`/`SetElement` could only
    /// mutate a throwaway copy; addressing the owning slot is what makes
    /// value-struct field writes persist. Array and type-instance steps along
    /// the path are reference types and are mutated through their handles.
    StorePlace { root: PlaceRoot, path: Vec<Projection>, value: Reg },

    // ── Type-Linked-List Operations ─────────────────────────────────
    First { type_def: TypeDefId },
    Last { type_def: TypeDefId },
    Next { object: Reg },
    Previous { object: Reg },

    DeleteLvalue { local: LocalId },
    DeleteLvalueGlobal { global: GlobalId },
    DeleteRvalue { value: Reg },

    // ── Compiler Intrinsics ─────────────────────────────────────────
    Len { array: Reg, dim: Option<Reg> },
    /// `Len(s$)` — length of a string in Unicode codepoints. Distinct from
    /// `Len` (arrays) because the operand and length semantics differ.
    StrLen { s: Reg },
    ConvertExplicit { value: Reg, target: IrType },
    Convert { value: Reg, from: IrType, to: IrType },

    // ── Function Calls ──────────────────────────────────────────────
    Call { callee: FuncId, args: Vec<Reg> },
    CallIndirect { callee: Reg, args: Vec<Reg> },

    // ── Constants ───────────────────────────────────────────────────
    ConstInt(i64),
    ConstLong(i64),
    ConstFloat(f64),
    ConstBool(bool),
    ConstString(String),
    ConstNull,

    // ── Array ───────────────────────────────────────────────────────
    Redim { local: LocalId, elem_type: IrType, dims: Vec<Reg> },
    RedimGlobal { global: GlobalId, elem_type: IrType, dims: Vec<Reg> },
}

/// The owning storage a [`InstKind::StorePlace`] path is rooted at. Every
/// CoolBasic lvalue bottoms out at a local or global variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaceRoot {
    Local(LocalId),
    Global(GlobalId),
}

/// One step of a [`InstKind::StorePlace`] access path, applied left-to-right
/// from the root toward the value being written.
#[derive(Clone, Debug, PartialEq)]
pub enum Projection {
    /// `.field` — into a struct value or a type-instance's field.
    Field(Symbol),
    /// `[i, j, ...]` — into an array element (multi-dimensional index).
    Index(Vec<Reg>),
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
    /// Terminate the whole program with the given process exit code. Lowered
    /// from the `End` statement (code 0) and `MakeError` (code 1). Distinct
    /// from `Return` (leaves one function) — `Halt` stops execution entirely.
    Halt {
        code: i32,
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
