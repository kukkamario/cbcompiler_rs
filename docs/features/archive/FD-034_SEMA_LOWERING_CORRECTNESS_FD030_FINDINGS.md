# FD-034: Sema/Lowering Correctness — FD-030 Findings

**Status:** Complete
**Completed:** 2026-06-16
**Priority:** Medium-High
**Effort:** Medium (1-4 hours)
**Impact:** Fixes three correctness defects in `cb-sema` surfaced by the FD-030 snapshot review: a type-table inconsistency for arrays of structs, a silently dropped `Delete` form, and a For-Each-over-multi-dim-array path that traps at runtime.

## Problem

All three were found while hand-reviewing the FD-030 lowering snapshots (recorded in that FD's "Bugs surfaced" section; evidence verified 2026-06-09):

1. **Array-of-structs element type inconsistency.** `check.rs::resolve_type_expr` (~line 108) refines `Type::TypeRef` → `Type::StructVal` (or `RuntimeType`) using the declaration kind, but only when the *whole* resolved type is a `TypeRef`. Composite types are never walked, so `Dim arr As P[]` resolves to `Array { elem: TypeRef(p) }` while `New P[3]` (whose element TypeExpr goes through the refinement as a top-level name, check.rs:1234) produces `StructVal(p)`. Visible in the `array_of_structs_element_field` snapshot: local printed as `Array<TypeRef(p), 1>`, allocation as `new_array StructVal(p)`. Consequences: For-Each element typing derives from the *declared* array type (check.rs:1733 clones `elem`), so iterating an array of structs types the loop variable as a heap `TypeRef`; the same hole exists for struct names inside `FnPtr` parameter/return positions. The interpreter mostly survives because it trusts the `new_array` element type, but checker decisions (field access rules, conversions, copy-vs-reference semantics) run on the wrong kind.

2. **`Delete <field/index lvalue>` silently dropped.** check.rs:1931 classifies `Delete n.link` / `Delete arr[0]` as `DeleteClass::Lvalue`, but the lowering arm (lower.rs ~1244–1257) only emits `DeleteLvalue`/`DeleteLvalueGlobal` for plain `Ident` operands; a Field/Index operand classified Lvalue falls through and emits **nothing** — the statement vanishes with no diagnostic and no IR.

3. **For Each over a multi-dim array traps at runtime.** `lower_for_each_array` (lower.rs:1892) emits `Len(arr)` with no dim (→ axis-0 length) and a single-index `GetElement`. The interpreter's `flat_index` (cb-backend-interp/src/heap.rs:179) returns `None` whenever `indices.len() != dims.len()`, so the first iteration over a 2-D array raises an `IndexOutOfBounds` trap. cb_syntax.md §6.3 explicitly allows `For val = Each scores` where "scores is Float[] or Float[,] etc." with row-major order — so today the documented feature is broken for rank ≥ 2.

## Solution

All in `cb-sema` (item 3 may optionally touch `cb-ir`/interp depending on the chosen design):

1. **Recursive refinement.** Extract the decl-kind refinement in `check.rs::resolve_type_expr` into a helper that walks the resolved `Type` structurally — `Array` element, `FnPtr` params/return — refining every embedded `TypeRef` whose name resolves to a `StructDef`/`RuntimeTypeDef`. The FD-030 snapshot `array_of_structs_element_field` then pins the fix (local becomes `Array<StructVal(p), 1>`); re-review the For-Each-over-struct-array typing afterwards and add a lowering snapshot for `For v = Each arr` where `arr` is `P[]`.

2. **Decide and implement Delete-on-place semantics.** Two defensible options:
   - (a) Lower Field/Index delete operands as **rvalue deletes**: evaluate the place, `DeleteRvalue` the resulting reference. The instance dies; other references (including the field/element itself) become stale and trap as `DeletedAccess` on later use — consistent with the existing heap generation machinery. Rewind semantics stay exclusive to plain variables.
   - (b) Reject Field/Index operands in sema with a new diagnostic if original-CoolBasic semantics require the slot rewind that only variable slots support.
   Consult `docs/cb_syntax.md` §3.3 / the user for which matches legacy behavior — **ask, don't guess** (CLAUDE.md language-reference rule). Either way the silent drop must go.

3. **Fix For Each over rank ≥ 2.** Preferred: iterate the flat backing store — requires exposing total element count and flat indexing to the IR (e.g. `Len` with a sentinel/total mode or a dedicated `TotalLen`, plus `GetElement` accepting a flat index for this desugar — coordinate with the interpreter's `flat_index` contract). Cheaper interim alternative: emit a sema error for For-Each over rank ≥ 2 so the trap becomes a compile error. Pick based on how soon multi-dim iteration is actually needed; document the choice in `cb_syntax.md` §6.3 if restricting.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-sema/src/check.rs` | MODIFY | Recursive TypeRef refinement (item 1); Delete classification or new diagnostic (item 2); possibly rank check for For-Each (item 3) |
| `crates/cb-sema/src/lower.rs` | MODIFY | Delete-on-place lowering (item 2); For-Each multi-dim desugar (item 3) |
| `crates/cb-sema/tests/lower_snapshots.rs` | MODIFY | New/updated snapshots: struct-array local type, For-Each over struct array, Delete-on-place, multi-dim For-Each (or its rejection) |
| `crates/cb-backend-interp/tests/integration.rs` | MODIFY | Runtime tests for items 2–3 once the design is settled (needs SDK or FD-033) |
| `docs/cb_syntax.md` | MODIFY | Record the decided Delete-on-place and multi-dim For-Each semantics |

## Verification

- `cargo test -p cb-sema` — updated `array_of_structs_element_field` snapshot shows `Array<StructVal(p), 1>`; new snapshots reviewed by hand.
- Item 2: `Delete n.link` produces either IR or a diagnostic — grep the printed IR / diags in a test; never silence.
- Item 3: a `For v = Each b` over `New Float[2, 3]` either iterates all 6 elements (interp test, SDK machine or post-FD-033) or fails sema with a clear error.
- `cargo llvm-cov -p cb-sema` not regressed; clippy `-D warnings` green.

## Implementation results (2026-06-15)

All three findings fixed. Decisions taken: item 2 → **rvalue delete** (user
choice, matches §3.3's `Delete n.something` worked example); item 3 → **full
row-major flat iteration** (the spec mandates it at §6.3, so the "emit a sema
error" interim option was rejected).

1. **Recursive `TypeRef` refinement.** Extracted the decl-kind refinement in
   `check.rs::resolve_type_expr` into a new `refine_type` helper that walks the
   resolved `Type` structurally — `Array` element and `FnPtr` params/return —
   refining every embedded `TypeRef` to `StructVal`/`RuntimeType`. `Dim arr As
   P[]` now resolves to `Array<StructVal(p), 1>`, consistent with `New P[3]`;
   the `array_of_structs_element_field` snapshot flipped accordingly, and For
   Each over a struct array now types the loop variable `StructVal(p)` (new
   `for_each_struct_array` snapshot). Helper takes `&self` (the symbol-table
   `lookup` is `&self`), so the recursion borrow-checks cleanly.

2. **`Delete <field/index>` → rvalue delete.** `check.rs::check_delete` now
   classifies only `Expr::Ident` as `DeleteClass::Lvalue`; Field/Index operands
   are `Rvalue`, so the lowerer evaluates the place and emits `delete_rvalue`
   over the loaded reference (node freed, no rewind, aliases dangle — exactly
   like `Delete First(T)`). The silent drop is gone. New `lower_snapshots`
   case `delete_field_and_index_are_rvalue` pins the IR; interp tests
   `delete_field_frees_node`/`delete_array_element_frees_node` confirm the node
   is actually unlinked at runtime (totals 1 and 2; would be 3 if dropped).
   Fixed the self-contradicting §3.3 line in `cb_syntax.md` (it had called
   field/element operands "lvalues").

3. **For Each over rank ≥ 2 — flat row-major walk.** Added two IR instructions
   in `cb-ir`: `ArrayTotalLen { array }` (product of all dimension lengths) and
   `GetElementFlat { array, index }` (single flat index into the row-major
   backing store, any rank). `lower_for_each_array` now emits these instead of
   `Len{dim:None}` + single-index `GetElement`, unifying every rank onto one
   desugar (for rank 1 they reduce to the old behavior — `for_each_array`
   snapshot updated). Wired through `print.rs`, `verify.rs`, and the interpreter
   (`interp.rs` reads `arr.total_len()` / `arr.data[flat]`; both trap
   `IndexOutOfBounds`/`NullDeref` appropriately). The LLVM backend is a stub
   with no `InstKind` match, so it needed no change. New snapshot
   `for_each_multidim_array`; interp test `for_each_multidim_array_row_major`
   over `New Int[2,3]` prints `1..6` in row-major order (previously trapped on
   the first iteration).

**Verification (Windows + Allegro SDK).** `cargo test --workspace` 624 passed /
0 failed (cb-sema 124+44, cb-backend-interp 50); `cargo clippy --workspace
--all-targets -D warnings` clean; `cargo llvm-cov -p cb-sema` lower.rs 80.56%
(unchanged), crate total 84.74% (not regressed). Every new/changed snapshot
hand-reviewed before acceptance.

## Related

- FD-030 (found all three; its snapshots are the regression net), FD-019 (StorePlace/value-struct machinery), FD-032 (interpreter hardening — runtime-side tests overlap), FD-033 (SDK-free testing unblocks the interp verification here)
- `docs/cb_syntax.md` §3.3 (Delete), §6.3 (For Each)
