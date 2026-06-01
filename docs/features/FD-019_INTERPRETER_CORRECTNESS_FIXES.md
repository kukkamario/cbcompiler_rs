# FD-019: Interpreter Correctness & Memory-Safety Fixes

**Status:** Pending Verification
**Priority:** High
**Effort:** Medium (grew during implementation — see note below)
**Impact:** Fixes four real correctness bugs in `cb-backend-interp` — the reference implementation — three of which produce silently-wrong results and one of which aborts the process on hostile input. Closes the test gaps (bitwise, structs, arrays) that let them ship.

> **Scope note (during implementation):** Bug #2 turned out to be deeper than the
> static code read suggested. Value-type structs were never wired end-to-end:
> sema's type-annotation resolver mapped a `Struct`-declared name to a heap
> `TypeRef` (defaulting to `Null`), so `Dim s As SomeStruct` never produced a
> `Value::Struct` and the interpreter's struct `SetField` arm was unreachable —
> `s.field = x` trapped `NullDeref` instead of silently dropping the write. With
> the user's agreement, FD-019 was expanded to enable value structs end-to-end
> (sema resolution + a path-based store) rather than only patch the interpreter.

## Problem

The post-FD-018 codebase review found that `cb-backend-interp` has broad happy-path coverage but almost no tests for bitwise ops, value-type structs, or multi-dimensional/runtime-sized arrays — and bugs live in exactly those untested paths. All four were independently verified against the source.

1. **Logical right shift is wrong on negative integers.** `eval_binop` sign-extends a 32-bit `Int` to `i64` (`*a as i64`, `interp.rs:845`), then `int_binop` does `(a as u64).wrapping_shr(..)` (`interp.rs:957`). For a negative `Int` the sign-extended high bits are all 1s, so the logical shift runs over 64 bits: `(-1) Shr 1` yields `-1` instead of `0x7FFFFFFF`. `cb_syntax.md:187` specifies `Shr` as a *logical* right shift. `Shl` (`:956`) and `Sar` (`:958`) also never mask the shift count to the operand width, so a count ≥ 32 on a 32-bit operand is not reduced.

2. **Value-type struct field writes do not persist — and value structs were unwired.** Two layered defects:
   - **sema:** `check.rs::resolve_type_expr` mapped every user type name to `Type::TypeRef`, so `As <Struct>` resolved to a heap reference type (defaulting to `Null`) even though the `Struct` was registered as `DeclKind::StructDef`/`Type::StructVal`. `Dim s As Vec2` therefore never produced a `Value::Struct`; `s.field = x` trapped `NullDeref`. There were zero sema/interp tests for value structs.
   - **interp:** even with a `StructVal` local, the old register-based `SetField` arm mutated a throwaway `LoadLocal`/`GetField` copy and wrote it back to a register, never to the owning slot. The same defect affected `arr[i].field = x` (array element clone) and nested structs.

3. **Hostile/overflowing array dimensions abort the process.** `NewArray` (`interp.rs:674`), `Redim` (`:731`), and `RedimGlobal` (`:743`) convert dimension registers with `value_to_i64(..) as usize`. A negative dimension (`New Int[-1]`) wraps to a huge `usize`; `ArrayObj::new` then does `vec![default; total]` (`heap.rs:150`), triggering a multi-exabyte allocation that aborts the whole interpreter. `dims.iter().product()` (`heap.rs:148`) can also overflow and wrap silently in release. For the reference interpreter an OOM abort is the worst possible failure mode.

4. **Array of struct elements defaults to `Null` instead of a zero-initialized struct.** `ArrayObj::new` calls `default_value(&elem_type, &[], string_api)` with an *empty* `struct_defs` slice (`heap.rs:149`); in `value.rs` an `IrType::StructVal` with no matching def falls through to `Value::Null` (`value.rs:99-112`). So `New Vec2[3]` produces three `Null`s, and field access on any element traps as `NullDeref`. `self.program.struct_defs` is threaded into `default_value` everywhere else (`interp.rs:111, 219, 472`) but not at the three `ArrayObj::new` call sites (`:676, :733, :745`).

Lower-severity items found in the same review, folded in here:

- **`IrUnOp::Not`/`Neg` handle only `Bool`/`Int`, not `Long`/`Byte`/`Short`/`UInt`/`ULong`** (`interp.rs:1079-1089`); other numeric variants fall to the catch-all `RuntimeError`, relying on sema always pre-converting.
- **`value_to_i64`/`value_to_f64` use a different string→number policy than `parse_leading_int`** (`:1141`/`:1160` full-parse-or-0 vs `:1348` leading-int used by `value_to_int_cb`), so a string array index like `"3x"` becomes `0` here but `3` under CB `toInt` rules — two policies in one file, both silently masking bad values.
- **Observer never sees a user-`Call` result** (`interp.rs:259-275`): when a frame is pushed, `after_inst` is skipped entirely, so a debugger watching the call site sees nothing — an observability gap given the crate's debuggability mandate.

## Solution

In `cb-backend-interp`:

- **Shifts (done):** width-correct in `int_binop`/`uint_binop` — `(a as u32)` for non-wide `Shr`, `(a as i32)` for non-wide `Sar`, and the shift count reduced modulo the operand width (32 or 64).
- **Value structs (done — path-based store):**
  - *sema:* `resolve_type_expr` now refines a `TypeRef` whose decl is a `StructDef` to `Type::StructVal` (and a `RuntimeTypeDef` to `RuntimeType`). The `First`/`Last`/`Next`/`Previous`/`New`/`Delete` checks correctly continue to reject value structs (they are not heap/list types); `check_field` already handled `StructVal`.
  - *IR:* a single `InstKind::StorePlace { root: PlaceRoot, path: Vec<Projection>, value }` (with `PlaceRoot::{Local,Global}` and `Projection::{Field,Index}`) replaces the register-based `SetField`/`SetElement` for **all** assignment lvalues. It addresses the owning slot directly, so value-struct writes persist; array and type-instance steps along the path are mutated through their shared handles.
  - *lowering:* `lower_assign` builds the place via `lower_place` (walking `Field`/`Index` down to a local/global root, lowering index regs after the RHS to preserve evaluation order).
  - *interp:* `store_walk` performs the in-place mutation, walking `Struct` (in place), `Array` (`Rc` borrow_mut + `flat_index`), and `TypeInstance` (heap, take/recurse/put-back) steps, deferring errors to `store_err` to keep `self` borrows disjoint. This was chosen over a `SetStructField`-on-local-only instruction because it also fixes nested (`s.a.b`) and array-element (`arr[i].field`) struct writes uniformly, per CB's "structs may contain structs by value".
- **Array dimensions (done):** `resolve_dims` rejects negative sizes with a clean `RuntimeError`; `ArrayObj::new` uses `checked_mul` for the product and `Vec::try_reserve_exact` so an over-large length is a clean error, not an allocation abort.
- **Array of structs (done):** `ArrayObj::new` now takes `&[StructDefInfo]`, threaded from `program.struct_defs`, so `StructVal` elements default to zero-initialised structs.
- **Not/Neg (done):** `Not` routes all integer widths + `Bool` through `is_truthy`; `Neg` gained `Byte`/`UInt`/`ULong` arms.
- **String→number policy (done):** `value_to_i64` now uses the documented `parse_leading_int` policy (matching `value_to_int_cb`); `value_to_f64` keeps strict full-parse with a comment.
- **Observer (done):** the `Return` handler fires the deferred `after_inst` for the call site (at `caller.pc - 1`) once a callee returns.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-backend-interp/src/interp.rs` | DONE | Width-correct shifts; `resolve_dims` validation + `make_array`; `StorePlace` execution via `store_walk`/`store_err`; complete `Not`/`Neg`; unify string→int policy; deferred observer call-result hook |
| `crates/cb-backend-interp/src/heap.rs` | DONE | `ArrayObj::new` takes `struct_defs`, returns `Result` (`checked_mul` + `try_reserve_exact`); `ArrayAllocError` |
| `crates/cb-ir/src/inst.rs` | DONE | Add `InstKind::StorePlace`, `PlaceRoot`, `Projection`; remove `SetField`/`SetElement` |
| `crates/cb-ir/src/print.rs`, `verify.rs` | DONE | Print/verify `StorePlace` (def-use of index + value regs; local/global root range checks) |
| `crates/cb-sema/src/check.rs` | DONE | `resolve_type_expr` resolves `StructDef` names to `Type::StructVal` |
| `crates/cb-sema/src/lower.rs` | DONE | `lower_place` builds root + projection path; `lower_assign` emits `StorePlace` for field/index targets |
| `crates/cb-backend-interp/tests/integration.rs` | DONE | 11 regression tests: shifts (negative `Shr`, `Sar`, large count), value-struct write/read/nested/copy, array-of-structs field + default, negative `New`/`Redim` dim clean errors |

## Verification

- `cargo test -p cb-backend-interp` green (41 tests, 11 new), asserting:
  - `(-1) Shr 1 == 2147483647`; `1 Shl 33 == 2` (count reduced to operand width); `(-8) Sar 1 == -4`.
  - `s.field = v` then read returns `v`; nested `o.inner.v = …`; struct copy (`q = p; q.x = 99`) leaves `p.x` unchanged.
  - `arr[i].field = v` round-trips; `New P[2]` then `arr[0].x` reads `0` without trapping.
  - `New Int[n]` with `n = -1` and `Redim arr As Int[n]` with `n = -5` produce a clean `RuntimeError` ("negative array dimension"), not an abort.
- ✅ `cargo test --workspace` green (no snapshot churn); ✅ `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Cross-backend note: these become reference behaviors the future LLVM backend must match (interp is the reference per CLAUDE.md). `StorePlace` is now the single lvalue-store opcode the LLVM backend will lower.

## Related

- Surfaced by the post-FD-018 codebase review (interpreter area).
- [FD-010](archive/FD-010_INTERPRETER_BACKEND.md) — interpreter value model, slab heap, `Observer` trait, `Struct`/`TypeInstance` distinction.
- [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) — opaque handle / type instance semantics.
- `docs/cb_syntax.md` §`Shr` (logical shift), §value-type structs (copied on assignment).
