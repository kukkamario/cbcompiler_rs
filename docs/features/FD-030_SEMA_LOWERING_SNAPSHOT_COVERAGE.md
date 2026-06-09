# FD-030: Sema Lowering Snapshot Coverage

**Status:** Pending Verification
**Priority:** High
**Effort:** Medium (1-4 hours)
**Impact:** Pins the IR shape for every major language construct, so backend disagreements (interp vs. future LLVM) can be triaged at the lowering layer instead of bisected end-to-end.

## Problem

`crates/cb-sema/src/lower.rs` (2112 lines) measures **53.8% line coverage** (716 uncovered lines) under host-runnable tests — the lowest of any non-trivial file in the workspace (`cargo llvm-cov`, 2026-06-09). `tests/lower_snapshots.rs` has only 18 snapshots, and these constructs have **zero** lowering snapshots:

- `Repeat ... Forever` / `Repeat ... While` (the only two Repeat forms — `Repeat ... Until` does not exist in this dialect)
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

## Implementation results (2026-06-09)

20 snapshot tests added (41 tests total in `lower_snapshots.rs`); every snapshot
hand-reviewed against `docs/cb_syntax.md` §3.2/§3.3/§6.2/§6.3 before acceptance.
Key checks confirmed: `Continue` targets body/cond/step per loop kind, `Break 2`
jumps to the *outer* loop's exit, Select `Continue` falls through into the next
arm's body skipping its test, all lvalue writes are single `store_place`
instructions rooted at the owning local/global with correct projection chains,
and both For-Each desugars match the spec.

Coverage (cargo llvm-cov, lines): `lower.rs` **53.8% → 80.6%**, `check.rs`
82.4% → 86.2%, crate total 84.8%. `cargo test -p cb-sema` (165 tests) and
clippy `-D warnings` on host crates green. Interp cross-check of `Break 2` /
struct snapshots still pending on a machine with the Allegro SDK (blocked here;
see FD-033).

### Bugs surfaced (not fixed here — candidate follow-up FDs)

1. **Array-of-structs element type inconsistency.** `check.rs::resolve_type_expr`
   (~line 108) refines `TypeRef` → `StructVal` only at the top level, so
   `Dim arr As P[]` keeps element type `TypeRef(p)` while `New P[3]` produces
   `StructVal(p)` — visible in the `array_of_structs_element_field` snapshot
   (`Array<TypeRef(p), 1>` local vs. `new_array StructVal(p)`). check.rs:1733
   derives For-Each element types from the *declared* array type, so iteration
   over arrays of structs likely mistypes the element as a heap ref.
2. **`Delete <field/index lvalue>` silently dropped.** check.rs:1931 classifies
   `Delete n.link` / `Delete arr[0]` as `DeleteClass::Lvalue`, but lower.rs
   (~1244–1257) only emits an instruction for plain identifier operands — the
   statement vanishes without a diagnostic. Deliberately *not* snapshotted.
3. **For Each over multi-dim arrays** iterates `Len(arr)` dimension 0 only,
   vs. §6.3's row-major full-iteration wording — semantics question to settle.

## Related

- FD-008 (IR + lowering), FD-019 (`StorePlace`, value structs), FD-020 (For-loop coercions — its snapshots are the model to follow)
- FD-033 (SDK-free testing) — explains why driver fixtures don't substitute for these snapshots in CI
- Coverage analysis session, 2026-06-09
