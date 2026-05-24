# FD-010: Interpreter Backend Implementation

**Status:** Complete
**Completed:** 2026-05-24
**Priority:** High
**Effort:** High (> 4 hours)
**Impact:** Enables end-to-end program execution, serves as the reference implementation for verifying LLVM backend correctness, and provides the debuggability foundation (stepping, inspection, tracing).

## Problem

The compiler pipeline is complete through IR lowering (lexer -> parser -> sema -> IR), but there is no execution backend. `cb-backend-interp` is an empty crate. We need a working interpreter that:

1. Executes `cb_ir::Program` by walking basic blocks and dispatching instructions.
2. Serves as the **reference implementation** — correctness and debuggability over speed.
3. Calls C runtime functions (Print, Abs, etc.) through the `cb-runtime-sys` FFI catalog.
4. Supports future debugging features (stepping, local inspection, execution tracing).

## Solution

### Architecture Overview

```
Interpreter<O: Observer>
  program: &Program
  globals: Vec<Slot>                    // shared across all frames
  call_stack: Vec<Frame>                // execution stack
  type_lists: Vec<TypeList>             // indexed by TypeDefId.0
  heap: Slab<TypeInstanceObj>           // Type instance storage
  runtime: &RuntimeDispatch             // FFI bridge (from cb-runtime-sys)
  observer: O                           // debug hooks (generic, zero-cost when no-op)

Frame
  func_id: FuncId
  registers: Vec<Value>     // indexed by Reg.0, append-only (pooled for reuse)
  locals: Vec<Slot>         // indexed by LocalId.0, read-write (pooled for reuse)
  current_block: BlockId
  pc: usize                 // instruction index within current block

Slot = (Value, bool)        // value + "deleted" flag for DeleteLvalue
```

### Value Representation

```rust
enum Value {
    // Primitives (Copy-weight)
    Byte(u8), Short(i16), Int(i32), UInt(u32),
    Long(i64), ULong(u64), Float(f64), Bool(bool),
    // Heap types
    String(Rc<str>),                        // immutable sharing; concat creates new Rc
    Array(Rc<RefCell<ArrayObj>>),            // reference semantics
    TypeInstance(TypeInstanceId),            // handle into Slab (reference semantics)
    Struct(Box<StructObj>),                  // value semantics (cloned on copy)
    FnPtr(Option<FuncId>),                  // None = null fn ptr
    Null,
}
```

**Key design decisions:**
- **Slab for Type instances** (not `Rc<RefCell<>>`): Type instances form doubly-linked lists. Mutating linked lists with `Rc<RefCell<>>` causes borrow conflicts when unlinking a node (need to borrow node + predecessor + successor simultaneously). A slab with `TypeInstanceId(u32)` handles avoids this entirely. Each entry stores fields, prev/next handles, a `freed` flag, and the type name.
- **`Rc<RefCell<>>` for arrays**: Arrays don't have linked-list complications. Reference semantics via Rc, interior mutability via RefCell for SetElement/Redim.
- **`Rc<str>` for strings**: Immutable sharing. StrConcat allocates a new `Rc<str>`. Simple and correct.
- **`Box<StructObj>` for structs**: Value semantics — `Value::clone()` deep-copies the struct. Assignment = copy.
- **`Slot = (Value, bool)`**: The `bool` tracks the "deleted" flag for `DeleteLvalue`. `DeleteRvalue` sets the `freed` flag on the heap object instead.

### Execution Loop

```
1. Find @main in func_table (last UserDefined entry). Push initial Frame.
2. Loop:
   a. Fetch frame.current_block.insts[frame.pc]
   b. observer.before_inst(...)
   c. Match on InstKind (~22 arms, grouped):
      - Constants: ConstInt/ConstFloat/ConstBool/ConstString/ConstNull -> create Value
      - Locals: LoadLocal -> read locals[id], StoreLocal -> write locals[id]
      - Arithmetic: BinOp/UnOp -> match on operator, compute result
      - Type ops: NewType -> allocate in slab + append to type list
                  GetField/SetField -> access slab entry's fields
                  First/Last/Next/Previous -> traverse type list
                  DeleteLvalue -> set local slot deleted flag + rewind to prev
                  DeleteRvalue -> set slab entry freed flag + unlink
      - Array ops: NewArray -> create ArrayObj, GetElement/SetElement, Redim
      - Calls: Call -> check FuncKind:
                 UserDefined -> push new Frame, jump to body
                 Runtime -> call RuntimeDispatch::call(symbol, args)
               CallIndirect -> read FnPtr from register, dispatch
      - Intrinsics: Len, Convert, ConvertExplicit
   d. If inst.result.is_some(), store computed Value into registers[reg.0]
   e. observer.after_inst(...)
   f. Advance pc. When pc == insts.len(), execute terminator:
      - Goto(id) -> set current_block = id, pc = 0
      - BranchIf { cond, then, else } -> read cond register, pick target
      - Return { value } -> pop frame, store return value in caller's result reg
      - Trap(kind) -> return Err(InterpError) with stack trace
3. When @main returns, execution is complete.
```

### Runtime FFI Bridge

All FFI lives in `cb-runtime-sys` (which already has `unsafe_code = "allow"`), keeping `cb-backend-interp` pure safe Rust.

Add to `cb-runtime-sys`:
```rust
pub struct RuntimeDispatch { /* symbol -> typed dispatch fn */ }

impl RuntimeDispatch {
    pub fn from_catalog(catalog: &[FuncDesc]) -> Self;
    pub fn call(&self, symbol: &str, args: &[Value]) -> Result<Value, RuntimeError>;
}
```

Internally, `call()` matches on the symbol string, marshals `Value` arguments to C types (e.g., `Value::String` -> `CString` -> `*const c_char`), calls the C function, and wraps the return value. All `unsafe` is confined here.

The interpreter's `Value` type must be visible to `cb-runtime-sys` for marshalling. Options:
- Define `Value` in `cb-ir` (both crates depend on it) -- cleanest
- Define `Value` in `cb-backend-interp` and have `cb-runtime-sys` depend on it -- circular
- Define a minimal `RuntimeValue` in `cb-runtime-sys` and convert -- extra conversion step

**Recommended:** Define `Value` in `cb-ir` since it is a representation of IR-level values and both the interpreter and runtime dispatch need it.

### Observer Trait (Debuggability)

```rust
pub trait Observer {
    fn before_inst(&mut self, _frame: &Frame, _inst: &Inst, _program: &Program) {}
    fn after_inst(&mut self, _frame: &Frame, _inst: &Inst, _result: Option<&Value>, _program: &Program) {}
    fn on_call(&mut self, _caller: &Frame, _callee_id: FuncId, _args: &[Value], _program: &Program) {}
    fn on_return(&mut self, _frame: &Frame, _value: Option<&Value>, _program: &Program) {}
    fn on_trap(&mut self, _frame: &Frame, _kind: TrapKind, _program: &Program) {}
}
```

- Generic parameter `Interpreter<O: Observer>` enables monomorphization -- zero-cost when `O = NoopObserver`.
- `NoopObserver` is zero-sized with all default (empty) methods -- compiler eliminates the calls entirely.
- Future implementations: `TracingObserver` (logs every instruction + value for miscompile reproduction), `SteppingObserver` (yields control for debugger integration).

### Error Handling

Traps return `Result<(), InterpError>` where:
```rust
struct InterpError {
    kind: TrapKind,
    message: String,
    stack_trace: Vec<StackFrame>,
}

struct StackFrame {
    func_name: Symbol,
    span: Span,     // from the current instruction
}
```

### Globals

**IR change: Introduce `GlobalId` and explicit `LoadGlobal`/`StoreGlobal` instructions.**

Currently globals are `Local` entries with `is_global: true`, and `LoadLocal`/`StoreLocal` are overloaded for both local and global access. This forces the interpreter to maintain a runtime redirect table to distinguish them. Instead:

1. Add `GlobalId(u32)` newtype to `cb-ir` (alongside `Reg`, `LocalId`, `BlockId`, `FuncId`).
2. Add `InstKind::LoadGlobal { global: GlobalId }` and `InstKind::StoreGlobal { global: GlobalId, value: Reg }`.
3. Add `Program::globals: Vec<Global>` where `Global { name: Symbol, ty: IrType }`.
4. The lowerer allocates `GlobalId`s for `Global`-declared variables and emits `LoadGlobal`/`StoreGlobal` instead of `LoadLocal`/`StoreLocal` for those variables. Remove `is_global` from `Local`.

This makes global access explicit in the IR — cleaner for both backends and eliminates any interpreter-side redirect logic. The interpreter stores globals in `Interpreter::globals: Vec<Slot>` indexed by `GlobalId.0`.

### Frame Pooling

When a function returns, its `registers: Vec<Value>` and `locals: Vec<Slot>` buffers are pushed to a freelist (`Interpreter::frame_pool: Vec<(Vec<Value>, Vec<Slot>)>`). On the next `Call`, pop from the pool and `.clear()` + resize instead of allocating fresh. Cheap win for recursive programs and hot call paths.

### Prerequisite IR Changes

Three IR changes before the interpreter can be cleanly implemented:

1. **Add `Span` to `Terminator`**: Currently `Terminator` has no span. When a `Trap` fires, the interpreter can't point to source. Add a `span: Span` field to each variant (or to `BasicBlock`). Update lowerer, printer, verifier.

2. **Add `TypeDefId(u32)`**: Currently `NewType`, `First`, `Last` use `Symbol` (type name). Add `TypeDefId(u32)` newtype that indexes into `Program::type_defs`. Change these instructions to use `TypeDefId` instead of `Symbol`. The lowerer resolves type names to indices. This lets the interpreter use `Vec<TypeList>` instead of `HashMap<Symbol, TypeList>`, matching the pattern of `FuncId`, `Reg`, etc.

3. **Add `GlobalId` + `LoadGlobal`/`StoreGlobal`**: As described in the Globals section above. Replace the `is_global` flag on `Local` with explicit instructions and a `Program::globals` table.

## Implementation Plan

### Phase 1: Core Execution (MVP)

Get a minimal program running end-to-end.

1. **IR changes**: Add `Span` to `Terminator`, `TypeDefId`, `GlobalId` + `LoadGlobal`/`StoreGlobal` (update lowerer, printer, verifier).
2. **Value enum** in `cb-ir`: primitives + String + Null only (no heap types yet).
3. **Frame + execution loop**: Constants, LoadLocal/StoreLocal, LoadGlobal/StoreGlobal, BinOp/UnOp, terminators. Frame pooling.
4. **RuntimeDispatch** in `cb-runtime-sys`: support `Print` only.
5. **Wire into driver**: `--backend interp` executes the program.
6. **Milestone**: `Print "Hello, World!"` runs and prints output.

### Phase 2: Control Flow + Functions

7. All terminators working (Goto, BranchIf, Return, Trap).
8. User-defined function calls (Call with UserDefined).
9. Globals (shared across frames).
10. **Milestone**: Programs with If/Else, loops, and function calls execute correctly.

### Phase 3: Heap Types

11. Type instances: Slab, NewType, GetField/SetField.
12. Type linked lists: First/Last/Next/Previous, DeleteLvalue/DeleteRvalue.
13. Arrays: NewArray, GetElement/SetElement, Redim.
14. Structs: value-copy semantics.
15. **Milestone**: Programs with Types, arrays, and structs execute correctly.

### Phase 4: Completeness

16. CallIndirect (function pointers).
17. Convert/ConvertExplicit (type conversions).
18. Len intrinsic.
19. All runtime functions (Abs variants).
20. Observer trait integration.
21. **Milestone**: All IR instructions implemented; observer hooks operational.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-ir/src/inst.rs` | MODIFY | Add `Span` to `Terminator` variants; add `LoadGlobal`/`StoreGlobal`; change `NewType`/`First`/`Last` from `Symbol` to `TypeDefId` |
| `crates/cb-ir/src/lib.rs` | MODIFY | Add `TypeDefId`, `GlobalId` newtypes; add `Program::globals`; add `Value` enum (or new `value.rs` module) |
| `crates/cb-ir/src/print.rs` | MODIFY | Update for terminator span changes |
| `crates/cb-ir/src/verify.rs` | MODIFY | Update for terminator span changes |
| `crates/cb-sema/src/lower.rs` | MODIFY | Pass spans to terminators |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | Add `RuntimeDispatch` |
| `crates/cb-runtime-sys/Cargo.toml` | MODIFY | Add dependency on `cb-ir` (for Value) |
| `crates/cb-backend-interp/src/lib.rs` | MODIFY | Core interpreter: Interpreter struct, Frame, execution loop |
| `crates/cb-backend-interp/src/value.rs` | CREATE | Value enum (if not in cb-ir) |
| `crates/cb-backend-interp/src/heap.rs` | CREATE | Slab, TypeInstanceObj, TypeList, ArrayObj |
| `crates/cb-backend-interp/src/observer.rs` | CREATE | Observer trait + NoopObserver |
| `crates/cb-backend-interp/Cargo.toml` | MODIFY | Add dependency on `cb-runtime-sys` |
| `crates/cb-driver/src/main.rs` | MODIFY | Wire `--backend interp` to actual execution |

## Verification

1. **Unit tests** (`cargo test -p cb-backend-interp`):
   - Individual instruction execution (arithmetic, constants, locals, control flow)
   - Type instance lifecycle (create, link, traverse, delete)
   - Array operations (create, index, redim)
   - Trap generation for each TrapKind

2. **Integration tests** (driver-level):
   - `cb run hello.cb` prints "Hello, World!"
   - Programs with loops, functions, recursion
   - Type-linked-list iteration patterns
   - Runtime function calls (Print, Abs)

3. **Snapshot tests** (insta):
   - Execution traces via TracingObserver for known programs
   - Error messages for each trap kind

4. **Future cross-check**:
   - When LLVM backend exists, run identical programs through both and compare output

## Related

- [FD-008](archive/FD-008_IR.md) -- IR design (what the interpreter consumes)
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) -- Runtime library (FFI catalog the interpreter calls)
- [FD-007](archive/FD-007_Semantic_Analysis.md) -- Sema (produces the IR the interpreter runs)
- `docs/cb_syntax.md` -- Language reference (Type linked-list semantics, Delete behavior, etc.)
