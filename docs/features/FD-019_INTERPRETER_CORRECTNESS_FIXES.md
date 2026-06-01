# FD-019: Interpreter Correctness & Memory-Safety Fixes

**Status:** Open
**Priority:** High
**Effort:** Medium (1-4 hours)
**Impact:** Fixes four real correctness bugs in `cb-backend-interp` — the reference implementation — three of which produce silently-wrong results and one of which aborts the process on hostile input. Closes the test gaps (bitwise, structs, arrays) that let them ship.

## Problem

The post-FD-018 codebase review found that `cb-backend-interp` has broad happy-path coverage but almost no tests for bitwise ops, value-type structs, or multi-dimensional/runtime-sized arrays — and bugs live in exactly those untested paths. All four were independently verified against the source.

1. **Logical right shift is wrong on negative integers.** `eval_binop` sign-extends a 32-bit `Int` to `i64` (`*a as i64`, `interp.rs:845`), then `int_binop` does `(a as u64).wrapping_shr(..)` (`interp.rs:957`). For a negative `Int` the sign-extended high bits are all 1s, so the logical shift runs over 64 bits: `(-1) Shr 1` yields `-1` instead of `0x7FFFFFFF`. `cb_syntax.md:187` specifies `Shr` as a *logical* right shift. `Shl` (`:956`) and `Sar` (`:958`) also never mask the shift count to the operand width, so a count ≥ 32 on a 32-bit operand is not reduced.

2. **`SetField` on a value-type `Struct` never updates the backing local.** For `s.field = x` where `s` is a `Struct`, sema lowers `s` to a `LoadLocal` register copy (`lower.rs:1426` → `emit_load_var`); the `SetField` struct arm (`interp.rs:558-565`) mutates that throwaway copy and writes it back to `frame.registers[object.0]`, never to `frame.locals`. A subsequent read of the local sees the unmodified struct. `TypeInstance` works only because it indirects through the slab heap; `Struct` is by-value (`cb_syntax.md:441-460`) and has no such backing.

3. **Hostile/overflowing array dimensions abort the process.** `NewArray` (`interp.rs:674`), `Redim` (`:731`), and `RedimGlobal` (`:743`) convert dimension registers with `value_to_i64(..) as usize`. A negative dimension (`New Int[-1]`) wraps to a huge `usize`; `ArrayObj::new` then does `vec![default; total]` (`heap.rs:150`), triggering a multi-exabyte allocation that aborts the whole interpreter. `dims.iter().product()` (`heap.rs:148`) can also overflow and wrap silently in release. For the reference interpreter an OOM abort is the worst possible failure mode.

4. **Array of struct elements defaults to `Null` instead of a zero-initialized struct.** `ArrayObj::new` calls `default_value(&elem_type, &[], string_api)` with an *empty* `struct_defs` slice (`heap.rs:149`); in `value.rs` an `IrType::StructVal` with no matching def falls through to `Value::Null` (`value.rs:99-112`). So `New Vec2[3]` produces three `Null`s, and field access on any element traps as `NullDeref`. `self.program.struct_defs` is threaded into `default_value` everywhere else (`interp.rs:111, 219, 472`) but not at the three `ArrayObj::new` call sites (`:676, :733, :745`).

Lower-severity items found in the same review, folded in here:

- **`IrUnOp::Not`/`Neg` handle only `Bool`/`Int`, not `Long`/`Byte`/`Short`/`UInt`/`ULong`** (`interp.rs:1079-1089`); other numeric variants fall to the catch-all `RuntimeError`, relying on sema always pre-converting.
- **`value_to_i64`/`value_to_f64` use a different string→number policy than `parse_leading_int`** (`:1141`/`:1160` full-parse-or-0 vs `:1348` leading-int used by `value_to_int_cb`), so a string array index like `"3x"` becomes `0` here but `3` under CB `toInt` rules — two policies in one file, both silently masking bad values.
- **Observer never sees a user-`Call` result** (`interp.rs:259-275`): when a frame is pushed, `after_inst` is skipped entirely, so a debugger watching the call site sees nothing — an observability gap given the crate's debuggability mandate.

## Solution

In `cb-backend-interp`:

- **Shifts:** mask/zero-extend to the operand's actual width before shifting (`(a as u32)` for `Shr`, `(a as i32)` for `Sar` on non-wide ints, then re-extend) and reduce the shift count modulo the type width.
- **Struct field store:** address the lvalue location, not an rvalue register. Either add a dedicated `SetStructField`-on-local/global instruction (cross-crate, cleaner) or have `SetField` recognize a local-origin object register and write the modified struct back to that slot. Prefer the explicit-instruction route since it keeps the interpreter "simple and observable" per CLAUDE.md.
- **Array dimensions:** validate before allocating — reject negative sizes (clean `RuntimeError`/CB trap), use `checked_mul` for the product, and cap total length.
- **Array of structs:** thread `program.struct_defs` into `ArrayObj::new` (or construct element defaults at the `NewArray`/`Redim` sites where `struct_defs` is in scope).
- **Not/Neg:** route all integer `Not` through `is_truthy` uniformly (returning `Bool`) or add the missing integer arms; same for `Neg`.
- **String→number policy:** unify on the documented `parse_leading_int` policy, or comment why index/dim conversion intentionally differs; consider a `Result` for the index path so a non-numeric index is a clean error.
- **Observer:** fire `after_inst` for the call site once the callee returns (the `return_reg` write in the `Return` handler is the natural hook), or document the gap on the `Observer` trait.

Each fix lands with a regression test (see Verification).

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-backend-interp/src/interp.rs` | MODIFY | Width-correct shifts; dimension validation in `NewArray`/`Redim`/`RedimGlobal`; thread `struct_defs` to array element defaults; complete `Not`/`Neg` numeric arms; unify string→int policy; observer call-result hook |
| `crates/cb-backend-interp/src/heap.rs` | MODIFY | `ArrayObj::new` takes `struct_defs`; `checked_mul` product + length cap |
| `crates/cb-ir/src/inst.rs` | MODIFY (if instruction route chosen) | Add `SetStructField`-on-local/global `InstKind` |
| `crates/cb-sema/src/lower.rs` | MODIFY (if instruction route chosen) | Lower `s.field = x` for value structs to the new lvalue-addressing instruction |
| `crates/cb-backend-interp/tests/integration.rs` | MODIFY | Regression tests: bitwise (incl. negative `Shr`, large counts), struct field write-then-read, array of structs, negative/overflowing array dim traps cleanly |

## Verification

- `cargo test -p cb-backend-interp` green, with new tests asserting:
  - `(-1) Shr 1 == 0x7FFFFFFF`; `1 Shl 33` reduced to operand width; `Sar` sign-preserving.
  - `s.field = v` followed by reading `s.field` returns `v` for a value-type struct.
  - `New Vec2[3]` yields three zero-initialized structs; field access does not trap.
  - `New Int[-1]` / a runtime `Redim` with a negative or overflowing size produces a clean `RuntimeError`, not an abort.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` green.
- Cross-backend note: these become reference behaviors the future LLVM backend must match (interp is the reference per CLAUDE.md).

## Related

- Surfaced by the post-FD-018 codebase review (interpreter area).
- [FD-010](archive/FD-010_INTERPRETER_BACKEND.md) — interpreter value model, slab heap, `Observer` trait, `Struct`/`TypeInstance` distinction.
- [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) — opaque handle / type instance semantics.
- `docs/cb_syntax.md` §`Shr` (logical shift), §value-type structs (copied on assignment).
