# FD-032: Interpreter Hardening Tests

**Status:** Complete
**Completed:** 2026-06-17
**Priority:** Medium-High
**Effort:** Medium (1-4 hours)
**Impact:** The reference backend's untested execution paths — indirect calls, multi-dim arrays, heap lifecycle, narrow integer widths — get direct coverage before the LLVM backend needs them as an oracle. **Scope grew during implementation:** the function-pointer item required a small first-class-functions *feature* (address-of), because the interpreter's `CallIndirect` success arm was dead code — nothing constructed a non-null function pointer. The narrow-width item surfaced a pre-existing numeric bug, spun off to **[FD-035](../FD-035_NARROW_UNSIGNED_NUMERIC_CORRECTNESS.md)**.

## Problem

`cb-backend-interp` has 47 solid integration tests, but a grep-verified sweep (2026-06-09) found these paths with **zero** test coverage:

1. **`CallIndirect` / function pointers** — no test anywhere exercises indirect calls, and the `NullFnPtr` trap has never fired in a test (the other 5 `TrapKind`s are tested).
2. **Multi-dimensional arrays** — no 2D+ creation/indexing test; `Len(arr, dim)` with an explicit dimension (`InstKind::Len { dim: Option<Reg> }`) is never exercised.
3. **Heap lifecycle edge cases** (`heap.rs`, 209 lines, no inline tests):
   - slab slot reuse after `Delete` (free-list reallocation) and that a stale `TypeInstanceId` with an old generation is rejected by `get()`'s generation check;
   - deleting multiple sibling instances mid-list (the Delete-with-rewind iteration contract beyond the single case in integration.rs:272);
   - a standalone instance lifecycle (create → mutate → delete) outside any `For Each` context.
4. **Narrow integer widths** — Byte/Short/UInt/ULong arithmetic, wrapping, comparison, and shift behavior are untested standalone (FD-019 fixed shift width bugs for Int/Long; the narrow widths got no equivalent tests). Same-width signed↔unsigned conversion behavior is untested end-to-end.
5. **Observer** — deferred call-result delivery is tested one level deep (integration.rs:478) but not across nested calls.

These are exactly the areas where the interpreter must be trustworthy as the reference implementation (CLAUDE.md: "Treat it as the reference implementation when the two backends disagree").

## Solution

- **Integration tests** (`tests/integration.rs`): function-pointer declaration + call + null-pointer trap; 2D array create/index/`Len(arr, n)`; sibling-delete iteration; standalone instance lifecycle; narrow-width arithmetic/wrap/shift programs (one per width, asserting exact wrapped values); nested-call observer test.
- **Inline unit tests** (`src/heap.rs`): slot reuse, generation bump on free, stale-id rejection. These need no catalog/SDK and run anywhere.
- If source-level syntax can't express something (e.g., if function-pointer *literals* aren't parseable yet), build the IR directly in the test as `print_snapshots.rs` does in cb-ir — the interpreter consumes `cb_ir::Program`, not source.

Note: integration.rs currently requires the C++ runtime via `load_catalog()` (FD-033). The heap inline tests are SDK-free regardless; write the rest so they need only `Print`-level catalog support, making them easy to move onto the FD-033 mock later.

## Implementation Notes (2026-06-17)

**Part A — first-class functions (the address-of feature).** The `CallIndirect` success arm (`interp.rs:806`) was dead code: nothing constructed `Value::FnPtr(Some(_))` (no address-of IR instruction; a bare function name lowered to `ConstNull`). cb_syntax.md §7.4 already specifies the behavior, so it was implemented rather than worked around:

- New `InstKind::FuncAddr { func: FuncId }` (`cb-ir/src/inst.rs`), wired through the printer (`func_addr <name>`), the verifier (`FuncId` bounds-check + no-operand reg arm), and the interpreter (`FuncAddr → Value::FnPtr(Some(func))`).
- `check_ident` types a bare function name in value position as `Type::FnPtr` (from the function's signature); assignment compatibility falls out of structural `Type` equality + the existing `coerce`/`E_CANNOT_CONVERT` path. Overloaded / built-in commands have no single address → new diagnostic **E0329** (`E_ADDRESS_OF_UNSUPPORTED`).
- **`check_call` was reordered** so a callee ident naming a function/command is resolved *by name* before the callee is ever typed as a value — otherwise E0329 would mis-fire on every command call (`print(...)`). `check_stmt`'s `ExprStmt` arm gained the matching interception so a bare 0-arg command statement stays a call (mirrors `lower_stmt`).
- `lower_ident_expr` emits `FuncAddr` for a bare function name (re-resolved via `func_id_map`; unambiguous because §7.2 forbids overloading). Scope: non-overloaded user-defined functions/subs only; `MyFunc()` is still the way to call a 0-arg function in a value context.

**Part B — the tests.** Heap inline tests in `src/heap.rs` (slot reuse + generation bump, stale-handle rejection, standalone lifecycle, LIFO reuse). Integration tests for the fn-pointer round-trip, higher-order call, reassignment, the `NullFnPtr` trap (the 6th trap, now fired), `Len(arr, dim)` on a 2-D array, multiple mid-iteration sibling deletes, and a nested-call observer. (The 2-D For-Each row-major and single-sibling-delete cases already existed from FD-034, so they were complemented, not duplicated.) Lowering + printer snapshots pin `func_addr`.

**FD-035 spun off.** The narrow-width tests surfaced a pre-existing numeric bug: `Dim x As <narrow/unsigned> = <int literal>` is not coerced to the declared type (`check_dim` never `coerce`s the init), so the variable holds a plain `Int`; shifts then dispatch as 32-bit signed, and `eval_binop` lacks LHS-type shift dispatch; plus `Value::Short(i16)` vs the documented unsigned 16-bit. Three narrow-width tests (`uint_shift_stays_unsigned_32bit`, `ulong_shift_stays_unsigned_64bit`, `short_holds_documented_unsigned_range`) are committed `#[ignore]`d with reasons pointing at **FD-035**; `byte_wraps_modulo_256_on_assignment` passes (it exercises the assignment-narrowing `Convert`, which does coerce).

## Files Created/Modified

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-ir/src/inst.rs` | MODIFY | new `InstKind::FuncAddr { func: FuncId }` |
| `crates/cb-ir/src/print.rs` | MODIFY | `FuncAddr` printer arm |
| `crates/cb-ir/src/verify.rs` | MODIFY | `FuncAddr` `FuncId` bounds-check + no-operand reg arm |
| `crates/cb-sema/src/diagnostics.rs` | MODIFY | `E_ADDRESS_OF_UNSUPPORTED` = E0329 |
| `crates/cb-sema/src/check.rs` | MODIFY | `check_ident` → `Type::FnPtr`; E0329; `check_call` reorder; `ExprStmt` bare-call interception |
| `crates/cb-sema/src/lower.rs` | MODIFY | `lower_ident_expr` emits `FuncAddr` |
| `crates/cb-backend-interp/src/interp.rs` | MODIFY | `FuncAddr` eval arm |
| `crates/cb-backend-interp/src/heap.rs` | MODIFY | inline `#[cfg(test)]` heap tests |
| `crates/cb-backend-interp/tests/integration.rs` | MODIFY | fn-ptr / `Len(arr,dim)` / multi-delete / nested-observer tests; 3 narrow-width tests `#[ignore]`d (FD-035) |
| `crates/cb-sema/tests/lower_snapshots.rs` (+ `.snap`) | MODIFY | `func_addr` lowering snapshot |
| `crates/cb-ir/tests/print_snapshots.rs` (+ `.snap`) | MODIFY | `FuncAddr` print snapshot |

## Verification (2026-06-17, Windows + Allegro SDK)

- `cargo test --workspace`: **0 failed** across all crates; **3 ignored** (the FD-035 narrow-width tests). New: 5 heap inline tests, fn-pointer round-trip/higher-order/reassign, `NullFnPtr` trap, `Len(arr,dim)`, multi-delete iteration, nested-call observer, byte assignment-wrap.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- Driver smoke test: `fp = add; Print Str(fp(2,3))` and `apply(inc, 41)` print `5` / `42`, exit 0. `x = Timer` (address-of a command) renders a clean `error[E0329]` with a real source label, exit 1.
- `NullFnPtr` now joins the other five `TrapKind`s with an explicit trap test.
- Narrow-width §3.4 cross-check is what surfaced FD-035; those assertions are `#[ignore]`d pending that fix rather than asserting the buggy values.

## Related

- FD-019 (interpreter correctness — established the trap/struct test patterns), FD-010 (interpreter backend), FD-020 (numeric semantics)
- FD-033 (catalog mock) — unblocks running these on machines without Allegro
- **FD-035 (narrow/unsigned numeric correctness)** — spun off from this FD's narrow-width findings
- cb_syntax.md §7.4 (first-class functions) — the address-of behavior implemented here
- Coverage analysis session, 2026-06-09
