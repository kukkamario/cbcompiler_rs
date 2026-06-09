# FD-032: Interpreter Hardening Tests

**Status:** Open
**Priority:** Medium-High
**Effort:** Medium (1-4 hours)
**Impact:** The reference backend's untested execution paths — indirect calls, multi-dim arrays, heap lifecycle, narrow integer widths — get direct coverage before the LLVM backend needs them as an oracle.

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

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-backend-interp/tests/integration.rs` | MODIFY | ~10–14 new tests per the list above |
| `crates/cb-backend-interp/src/heap.rs` | MODIFY | Inline `#[cfg(test)]` module: slot reuse, generation/stale-handle checks |

## Verification

- `cargo test -p cb-backend-interp` green (requires SDK until FD-033 lands; heap inline tests run regardless).
- `NullFnPtr` joins the other trap kinds in having an explicit trap test.
- Narrow-width results cross-checked by hand against `docs/cb_syntax.md` §3.4 numeric rules.

## Related

- FD-019 (interpreter correctness — established the trap/struct test patterns), FD-010 (interpreter backend), FD-020 (numeric semantics)
- FD-033 (catalog mock) — unblocks running these on machines without Allegro
- Coverage analysis session, 2026-06-09
