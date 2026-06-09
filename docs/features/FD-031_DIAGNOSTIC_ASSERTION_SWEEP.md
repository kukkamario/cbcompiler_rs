# FD-031: Diagnostic Assertion Sweep

**Status:** Open
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** Every defined diagnostic code is either asserted by a test or deliberately removed; error spans/recovery get regression protection.

## Problem

A grep-verified sweep (2026-06-09) found diagnostic codes that no test ever asserts:

**cb-sema** (6 of 29 codes unasserted):
- **E0301** `E_TYPE_MISMATCH` — emitted in many places (operator mismatches, `Len`/`New`/`First`/`Last` validation, loop condition types, `Redim` dims) but never asserted by code.
- **E0304** `E_CALL_NON_FUNCTION` (check.rs ~line 931) — e.g. `Dim x As Int : x(1)`.
- **E0306** `E_INDEX_NON_ARRAY` (~line 1160) — e.g. `x[0]` on an Int.
- **E0307** `E_RANK_MISMATCH` (~line 1148) — 1-D array indexed with 2 indices.
- **E0309** `E_FIELD_ON_NON_TYPE` (~line 1189) — `x.field` on an Int.
- **E0311** `E_TYPE_AS_VALUE` — defined in diagnostics.rs but **never emitted anywhere**. Decide: implement the check or delete the code.

**cb-frontend parser** (~11 of 18 codes unasserted): E0201 only implicitly via recovery; **E0203** (unterminated block — possibly never emitted; the mismatched-closer path uses E0204), **E0205** (invalid type expression), **E0207** (reserved word as name), **E0210** (multi-name not allowed, e.g. `Const x, y = 1`), **E0211** (`Field` outside Type body), **E0212** (single-line If with ElseIf), **E0213** (`Break` count not a positive int literal), **E0214** (label with sigil). E0208/E0209 are asserted only at the `string_value.rs` layer, not through the parser.

Untested error paths are where spans drift, recovery loops, and messages rot — the exact defect class FD-004/FD-021/FD-027 kept finding by accident.

## Solution

One small test per code, in the established style:

- **cb-sema:** inline tests in `check.rs`'s existing test module (`check_err(source, "E03xx")` pattern). For E0301, pick 2–3 representative emission sites rather than all of them.
- **cb-frontend:** unit tests in the existing `recovery_tests`/`decl_tests` modules in `parser.rs`, asserting the diagnostic code and that parsing recovers (no panic, bounded errors).
- For **E0311** and **E0203**: first confirm whether any code path can emit them. If not, either implement the missing check (if `cb_syntax.md` requires it) or delete the code and renumber nothing — leave a tombstone comment in diagnostics.rs.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-sema/src/check.rs` | MODIFY | ~7 inline tests (E0301 ×2–3, E0304, E0306, E0307, E0309) |
| `crates/cb-sema/src/diagnostics.rs` | MODIFY | Resolve E0311 (implement or remove) |
| `crates/cb-frontend/src/parser.rs` | MODIFY | ~9 unit tests for the unasserted parser codes; resolve E0203 |

## Verification

- `cargo test -p cb-sema -p cb-frontend` green.
- Sweep check: every code listed in `diagnostics.rs` / parser error constants appears in at least one `#[test]` (grep), or carries a comment explaining why not.
- `cargo llvm-cov -p cb-sema --summary-only`: `check.rs` error paths should push line coverage from ~82% toward ~90%.

## Related

- FD-004 (parser correctness — introduced several of these codes), FD-021 (panic safety), FD-027 (diagnostic rendering robustness)
- Coverage analysis session, 2026-06-09
