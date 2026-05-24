# FD-011: Runtime Custom Types

**Status:** Complete
**Completed:** 2026-05-24
**Priority:** High
**Effort:** Medium (1-4 hours)
**Impact:** Enables runtime libraries to define opaque handle types (file handles, images, sounds, etc.), required before expanding the runtime catalog with real functionality.

## Problem

The runtime catalog can only declare functions using primitive types (`Int`, `Float`, `String`, `Bool`, `Void`). A real CoolBasic runtime needs opaque handle types — e.g., `Image`, `Sound`, `File` — that runtime functions can create and accept but user code cannot inspect or manipulate. Original CoolBasic didn't have proper custom types, but we're adding them as first-class opaque handles.

## Solution

Add a **type declaration section** to the C runtime catalog. Each custom type gets a name and a unique type tag. Function signatures reference these tags the same way they reference primitive types today.

### Catalog layer (`cb-runtime-sys`)

- Extend `CbCatalog` with a type declaration array: `{ name, type_tag }`
- Add new type tags for custom types (starting after the existing primitive tags)
- `load_catalog()` reads the type declarations and returns them alongside function descriptors

### Semantic analysis (`cb-sema`)

- Register runtime custom types as distinct named types in the symbol table
- These types support **only**: assignment, comparison to `Null`, passing to/from runtime functions
- No arithmetic, no field access, no conversions to/from other types
- Type-check function calls against the declared parameter/return types

### IR (`cb-ir`)

- Extend `IrType` (or the type system) to represent runtime opaque types
- Values of these types flow through the IR like any other value

### Interpreter (`cb-backend-interp`)

- Treat runtime custom type values as **opaque pointers** (or integer handles)
- The interpreter stores and passes them but never inspects their contents
- Runtime functions that create/consume these handles do so via the existing `call_runtime` dispatch

### Key constraints

- User code **cannot** add, subtract, or manipulate opaque values in any way
- Sema must reject arithmetic/comparison (other than `= Null` / `<> Null`) on these types
- The interpreter doesn't need to know what's inside — it's just passing handles around

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-runtime-sys/c/cb_runtime.h` | MODIFY | Add type declaration struct and array to catalog |
| `crates/cb-runtime-sys/c/catalog.c` | MODIFY | Define example custom types |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | Parse type declarations from catalog, expose to driver |
| `crates/cb-ir/src/lib.rs` | MODIFY | Extend `IrType` for opaque runtime types |
| `crates/cb-sema/src/*.rs` | MODIFY | Register custom types, enforce opaque semantics |
| `crates/cb-backend-interp/src/value.rs` | MODIFY | Handle opaque values |
| `crates/cb-backend-interp/src/interp.rs` | MODIFY | Pass opaque values through runtime calls |
| `crates/cb-driver/src/main.rs` | MODIFY | Thread type declarations from catalog to sema |

## Verification

- `cargo test --workspace` — all existing tests still pass
- New unit tests in `cb-sema` verifying that arithmetic/field-access on opaque types is rejected
- New integration test in `cb-backend-interp` with a dummy opaque type: create via runtime call, pass to another runtime call, verify null comparison works
- Verify that `cargo check -p cb-backend-llvm` still compiles (IR changes are backend-agnostic)

## Related

- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) — Runtime library catalog architecture
- [FD-010](archive/FD-010_INTERPRETER_BACKEND.md) — Interpreter backend (handles the opaque values)
