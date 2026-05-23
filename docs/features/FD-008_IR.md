# FD-008: Intermediate Representation

**Status:** Open
**Priority:** High
**Effort:** High (> 4 hours)
**Impact:** Defines the shared representation that both backends consume — the central seam of the compiler.

## Problem

After semantic analysis (FD-007) we have a typed, name-resolved AST. Backends need a lower-level representation that:

- Is flat (no tree recursion) — easy to interpret instruction-by-instruction and straightforward to map to LLVM IR
- Makes control flow explicit (basic blocks + branches, not nested `If`/`While`)
- Makes implicit operations visible (conversions, allocations, linked-list threading)
- Is backend-agnostic — no LLVM types, no interpreter-specific constructs
- Carries enough type information for backends to emit correct code without re-analyzing

This FD covers the IR data structures and the AST→IR lowering pass.

## Design Decisions (from discussion)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| IR style | Three-address / register-based | Balance: natural to interpret with stepping, maps cleanly to LLVM |
| SSA? | No strict SSA | Registers can be reassigned; avoids phi nodes that complicate the interpreter. LLVM backend can run mem2reg. |
| Input | `SemaResult` (FD-007) | Types already resolved; lowering is mechanical |

## Solution

### IR structure overview

```
Program
 └─ Function[]
     ├─ name, params, return_type
     ├─ locals: Vec<Local>          // all local variables (incl. params)
     └─ body: Vec<BasicBlock>
         ├─ label: BlockId
         ├─ instructions: Vec<Inst>
         └─ terminator: Terminator
```

Top-level code (outside any function) is wrapped in a synthetic `@main` function during lowering.

### Registers and values

```rust
/// Virtual register — assigned by instructions, consumed by operands.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct Reg(u32);

/// A local variable slot (parameters + Dim/Global declarations).
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct LocalId(u32);

pub struct Local {
    pub name: Symbol,       // for debug/inspection
    pub ty: IrType,
    pub is_global: bool,
    pub is_param: bool,
}

/// Block identifier within a function.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct BlockId(u32);
```

Registers are function-scoped temporaries produced by instructions. Local variable slots represent named variables from the source; instructions load from / store to them explicitly.

### Type representation in IR

```rust
pub enum IrType {
    Byte,
    Short,
    Int,
    UInt,
    Long,
    ULong,
    Float,
    Bool,
    String,
    Array { elem: Box<IrType>, rank: u8 },
    TypeRef(Symbol),        // pointer to heap-allocated Type instance
    StructVal(Symbol),      // inline value-type Struct
    FnPtr(FnSig),
    Null,
    Void,
}

pub struct FnSig {
    pub params: Vec<IrType>,
    pub ret: Box<IrType>,   // Void for Subs
}
```

### Instruction set

Each instruction produces at most one result register.

```rust
pub struct Inst {
    pub result: Option<Reg>,    // None for stores, traps, etc.
    pub kind: InstKind,
    pub span: Span,             // source location for diagnostics/debugging
}
```

#### Arithmetic & logic

```rust
// Binary: result = lhs op rhs
BinOp { op: IrBinOp, lhs: Reg, rhs: Reg }

// Unary: result = op operand  
UnOp { op: IrUnOp, operand: Reg }

pub enum IrBinOp {
    // Arithmetic
    Add, Sub, Mul, Div, IntDiv, Mod, Pow,
    // Bitwise
    BinAnd, BinOr, BinXor, Shl, Shr, Sar,
    // Comparison (result is always Bool)
    Eq, NotEq, Lt, Gt, LtEq, GtEq,
    // String
    StrConcat,
    StrEq, StrNotEq, StrLt, StrGt, StrLtEq, StrGtEq,
}

pub enum IrUnOp {
    Neg, Plus, Not, BinNot,
}
```

String operations are distinct opcodes (not overloaded on `Add`/`Eq`) since sema has already resolved operand types. Short-circuit `And`/`Or` are lowered to conditional branches, not binary ops.

#### Memory & variables

```rust
// Load a local variable into a register
LoadLocal { local: LocalId }

// Store a register into a local variable
StoreLocal { local: LocalId, value: Reg }

// Load a global variable (globals are also LocalId slots, but
// the backend may need to distinguish storage)
// Handled via Local::is_global flag on LoadLocal/StoreLocal.

// Allocate a new Type instance (threads into global linked list)
NewType { type_name: Symbol }

// Allocate a new array
NewArray { elem_type: IrType, dims: Vec<Reg> }

// Field access on a Type or Struct
GetField { object: Reg, field: Symbol, field_type: IrType }

// Field mutation
SetField { object: Reg, field: Symbol, value: Reg }

// Array element access (multi-dimensional)
GetElement { array: Reg, indices: Vec<Reg> }

// Array element mutation
SetElement { array: Reg, indices: Vec<Reg>, value: Reg }
```

#### Type-linked-list operations

```rust
// Built-in Type list navigation — each returns TypeRef or Null
First { type_name: Symbol }
Last { type_name: Symbol }
Next { object: Reg }
Previous { object: Reg }

// Delete (two forms distinguished by sema's lvalue/rvalue classification)
DeleteLvalue { local: LocalId }   // rewind variable, mark deleted, unlink, free
DeleteRvalue { value: Reg }       // free only, no rewind
```

#### Compiler intrinsics

```rust
// Array length query — Len(arr) or Len(arr, dim)
Len { array: Reg, dim: Option<Reg> }

// Explicit type conversion via intrinsic function (Int(), Float(), Str(), Bool())
// Distinct from Convert: handles runtime string parsing (e.g. Int("123") → 123,
// parse failure → 0) and other conversions that are not implicit widening.
ConvertExplicit { value: Reg, target: IrType }
```

#### Type conversions

```rust
// Implicit conversion (sema inserted these based on conversion table)
Convert { value: Reg, from: IrType, to: IrType }
```

#### Function calls

```rust
// Direct call to a known function
Call { callee: Symbol, args: Vec<Reg> }

// Indirect call through a function pointer
CallIndirect { callee: Reg, args: Vec<Reg> }
```

#### Constants

```rust
ConstInt(i64)
ConstFloat(f64)
ConstBool(bool)
ConstString(String)
ConstNull
```

#### Redim

```rust
// Reallocate an array variable with new dimensions
Redim { local: LocalId, elem_type: IrType, dims: Vec<Reg> }
```

### Terminators

Every basic block ends with exactly one terminator:

```rust
pub enum Terminator {
    // Unconditional jump
    Goto(BlockId),

    // Conditional branch
    BranchIf { cond: Reg, then_block: BlockId, else_block: BlockId },

    // Return from function
    Return { value: Option<Reg> },

    // Runtime trap (unreachable in correct programs)
    Trap(TrapKind),
}

pub enum TrapKind {
    NullDeref,
    DeletedAccess,
    DivisionByZero,
    IndexOutOfBounds,
    NullFnPtr,
    DoubleDelete,
}
```

### Control flow lowering

Source-level control flow is desugared into basic blocks:

| Source construct | IR lowering |
|-----------------|-------------|
| `If / ElseIf / Else` | `BranchIf` chain → then/elseif/else blocks → merge block |
| `While … Wend` | cond block → `BranchIf` → body block → `Goto cond` / exit block |
| `Repeat … Forever` | body block → `Goto body` (Break → exit block) |
| `Repeat … Until/While` | body block → cond block → `BranchIf` → body / exit |
| `For` | init → cond block → `BranchIf` → body → step → `Goto cond` / exit |
| `For Each` (Type) | `First(T)` → cond → `BranchIf null` → body → `Next` → `Goto cond` |
| `For Each` (Array) | index init → bounds check → body → increment → `Goto check` |
| `Select / Case` | chain of `BranchIf` blocks (value comparisons) → arm blocks → merge |
| `And` / `Or` (short-circuit) | `BranchIf` → evaluate rhs block / short-circuit block → merge with result |
| `Break n` | `Goto` to the exit block of the n-th enclosing loop |
| `Continue` (in loop) | `Goto` to the loop's continue block (step/cond) |
| `Continue` (in `Select/Case`) | `Goto` to the next case arm's body block (fall-through, §6.2) |
| `Goto label` | `Goto` to the block starting at that label |

### AST→IR lowering pass

Lives in `cb-sema`. Consumes `SemaResult` + AST, produces `ir::Program`.

```rust
pub fn lower(
    arena: &Arena,
    program: &[NodeId],
    sema: &SemaResult,
) -> ir::Program;
```

The lowerer:
1. Creates `@main` function for top-level statements
2. Lowers each function body into basic blocks
3. Assigns registers for each sub-expression (bottom-up)
4. Inserts `Convert` instructions where `ConversionTable` has entries
5. Desugars control flow into blocks + terminators
6. Inserts trap checks before null-dereference / index / delete operations

### IR validation (debug builds)

A `verify()` pass that asserts structural invariants:
- Every block ends with exactly one terminator
- All register uses are dominated by their definitions
- All `BlockId` targets exist within the function
- Type consistency: instruction result types match their use sites

## Scope & non-scope

**In scope for FD-008:**
- All IR data structures (`Program`, `Function`, `BasicBlock`, `Inst`, `Terminator`, etc.)
- Compiler intrinsic instructions (`Len`, `ConvertExplicit`)
- AST→IR lowering pass
- Control flow desugaring (all loop/branch forms, `Continue` in both loops and `Select/Case`)
- Short-circuit And/Or lowering
- IR validation pass (debug-only)
- IR pretty-printer (text dump for `--dump-ir` flag)

**Deferred:**
- Optimization passes (constant folding, dead code elimination, etc.)
- LLVM-specific lowering (FD for LLVM backend)
- Interpreter execution of IR (FD-009)
- `Goto` across structured control flow (rare edge case — sema now validates Goto-into-For as E0321, but Goto from inside a loop to outside may need special IR handling)

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-ir/src/lib.rs` | MODIFY | IR data structures: `Program`, `Function`, `BasicBlock`, `Inst`, `Terminator` |
| `crates/cb-ir/src/types.rs` | CREATE | `IrType`, `FnSig` |
| `crates/cb-ir/src/inst.rs` | CREATE | `InstKind`, `IrBinOp`, `IrUnOp`, `TrapKind` |
| `crates/cb-ir/src/print.rs` | CREATE | Text-format IR printer (`--dump-ir`) |
| `crates/cb-ir/src/verify.rs` | CREATE | Debug-mode structural validation |
| `crates/cb-ir/Cargo.toml` | MODIFY | Add dependency on `cb-diagnostics` (for `Span`, `FileId`, `Symbol`) |
| `crates/cb-sema/src/lower.rs` | CREATE | AST→IR lowering pass |
| `crates/cb-sema/Cargo.toml` | MODIFY | Add dependency on `cb-ir` |
| `crates/cb-driver/src/main.rs` | MODIFY | Add `--dump-ir` flag, call lowering after sema |

## Verification

1. **Round-trip tests:** Parse → sema → lower → print IR → snapshot (insta). Cover each statement/expression form.
2. **Verify pass:** Run `ir::verify()` on all lowered programs in tests; must not panic.
3. **Control flow tests:** Each loop form, nested loops with Break/Continue, Select with Default.
4. **Short-circuit tests:** `And`/`Or` produce correct branch structure.
5. **`--dump-ir` smoke test:** `cargo run -p cb-driver -- --dump-ir file.cb` produces readable output.
6. **All existing tests pass:** `cargo test --workspace`

## Related

- [FD-007](FD-007_SEMANTIC_ANALYSIS.md) — Semantic analysis (produces the typed AST that lowering consumes)
- [FD-002](archive/FD-002_PARSER.md) — Parser (AST structure)
- `docs/cb_syntax.md` — Language semantics that drive lowering decisions
- Future FD-009: Interpreter backend (first consumer of this IR)
