# FD-030: Sema Lowering Snapshot Coverage

**Status:** Open
**Priority:** High
**Effort:** Medium (1-4 hours)
**Impact:** Pins the IR shape for every major language construct, so backend disagreements (interp vs. future LLVM) can be triaged at the lowering layer instead of bisected end-to-end.

## Problem

`crates/cb-sema/src/lower.rs` (2112 lines) measures **53.8% line coverage** (716 uncovered lines) under host-runnable tests — the lowest of any non-trivial file in the workspace (`cargo llvm-cov`, 2026-06-09). `tests/lower_snapshots.rs` has only 18 snapshots, and these constructs have **zero** lowering snapshots:

- `Repeat ... Forever` / `Repeat ... Until` / `Repeat ... While`
- `Break` (including `Break n` with a nesting count) and `Continue`, in each loop kind
- `For Each` over arrays and over Type linked lists
- Arrays: indexing, multi-dimensional `New Int[a, b]`, element assignment, `Redim`
- Value structs: field access, nested fields (`s.a.b`), copy semantics, arrays of structs (the FD-019 `StorePlace` paths)
- Type references: field access via `\` and `.`, list intrinsics (`First`/`Last`/`Next`/`Previous`), `Delete`

The driver's 28 program fixtures exercise some of these end-to-end, but (a) they require the Allegro SDK to even build (see FD-033), and (b) they assert program *output*, not IR shape — a lowering regression that happens to produce the same output passes silently.

## Solution

Extend `crates/cb-sema/tests/lower_snapshots.rs` with one `insta` snapshot per construct above, following the existing `lower_ok(source)` → `assert_snapshot!` pattern. Group related cases (e.g., one source with all three Repeat forms is fine if the snapshot stays readable; Break/Continue deserve separate snapshots per loop kind because the branch targets are the interesting part).

For constructs needing the runtime catalog (none of the above should — they are pure language core), reuse the existing test-catalog setup already in the file.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-sema/tests/lower_snapshots.rs` | MODIFY | ~15–20 new snapshot tests for the constructs listed above |
| `crates/cb-sema/tests/snapshots/*.snap` | CREATE | Generated via `cargo insta review` |

## Verification

- `cargo test -p cb-sema` green; review each new snapshot by hand against `docs/cb_syntax.md` semantics before accepting.
- `cargo llvm-cov -p cb-sema --summary-only`: `lower.rs` line coverage should rise from ~54% to ≥80%. Remaining misses should be error/unreachable paths only.
- Spot-check at least the struct and `Break n` snapshots against interpreter behavior (`cargo test -p cb-backend-interp`) where the SDK is available.

## Related

- FD-008 (IR + lowering), FD-019 (`StorePlace`, value structs), FD-020 (For-loop coercions — its snapshots are the model to follow)
- FD-033 (SDK-free testing) — explains why driver fixtures don't substitute for these snapshots in CI
- Coverage analysis session, 2026-06-09
