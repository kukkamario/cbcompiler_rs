# FD-031: Diagnostic Assertion Sweep

**Status:** Complete
**Completed:** 2026-06-18
**Priority:** Medium
**Effort:** Medium (2-3 hours) — mostly one-line tests; E0311 is the one real implementation (value-position gating).
**Impact:** Every defined diagnostic code is either asserted by a test or deliberately removed; error spans/recovery get regression protection.

## Implementation Notes (2026-06-18, complete)

Implemented in three commits on `fd-031-diagnostic-assertion-sweep`:
1. **cb-sema asserts** — tests for E0301 (operator + `Len` sites), E0304, E0306, E0307, E0309, E0329 (no behavior change).
2. **cb-sema E0311** — `check_ident` now rejects any `TypeDef`/`StructDef`/`RuntimeTypeDef` name in value position. The legitimate bare-name positions (`First`/`Last`, For-Each source) resolve the name directly via a new `resolve_type_name_arg` helper, which records the arg type in `self.types` so lowering's `self.types.get(arg)` stays valid. `New` (structural `TypeExpr`) and `Delete`/`Next`/`Previous` (instance values) were unaffected. The matching-slot soundness hole (`Function f() As Foo : Return Foo`, previously exit 0) now fails with E0311 — confirmed by a driver spot-check.
3. **cb-frontend** — E0205 tests (sigil-on-type, non-type token); E0207 deleted; E0299 guards commented.

Verification: full workspace `cargo test` green; `cargo clippy --workspace --all-targets -D warnings` clean; `cargo fmt --all --check` clean (run as a final whitespace-only commit, `d311107`, which also absorbed pre-existing rustfmt-version drift in a few untouched crates — heap.rs, interp.rs, ir/types.rs — so the tree is now uniformly formatted).

## Problem

A grep-verified sweep (originally 2026-06-09, **re-verified 2026-06-18 against current HEAD**) found diagnostic codes that no test asserts. The re-verification corrected several stale claims: the parser side gained coverage from snapshot + integration tests that landed after the original sweep, and FD-035 added one sema code (E0330, already asserted).

**cb-sema** (7 of 31 codes unasserted):
- **E0301** `E_TYPE_MISMATCH` — emitted in ~13 places (operator mismatches, `Len`/`New`/`First`/`Last` validation, loop condition types, `Redim` dims) but never asserted by code.
- **E0304** `E_CALL_NON_FUNCTION` (check.rs:1028) — e.g. `Dim x As Int : x(1)`.
- **E0306** `E_INDEX_NON_ARRAY` (check.rs:1251) — e.g. `x[0]` on an Int.
- **E0307** `E_RANK_MISMATCH` (check.rs:1239) — 1-D array indexed with 2 indices.
- **E0309** `E_FIELD_ON_NON_TYPE` (check.rs:1279) — `x.field` on an Int.
- **E0329** `E_ADDRESS_OF_UNSUPPORTED` (check.rs:832) — address-of an unsupported target. *(Missed by the original sweep.)*
- **E0311** `E_TYPE_AS_VALUE` — defined but **never emitted**, and (verified 2026-06-18) this is a real gap, *not* dead code. A bare type name in value position (`Return Foo`, `a = Foo`, `Print Foo`) resolves silently to a `TypeRef` value (check.rs:624 declares the TypeDef with `ty: TypeRef`; check.rs:792 `check_ident` returns it with no guard). Two observed outcomes:
  - Into a **mismatched** slot it leaks out as `E0317` with an internal Debug repr: `cannot implicitly convert TypeRef { name: Symbol(0) } to Int` (note the unresolved `Symbol(0)` instead of `Foo`).
  - Into a slot expecting **that same type** (`Function f() As Foo : Return Foo`) it **compiles clean (exit 0)** — a genuine soundness hole: the type itself is accepted as an instance.
  **Resolution: implement E0311** (don't tombstone). Four positions legitimately take a **bare `Type` name** — `First`, `Last`, `New`, and `Each` (For-Each over a Type) — and there the name is *always a plain identifier, never a more complex expression*. `New` already represents its name structurally as a `TypeExpr` (resolved via `resolve_type_expr`, check.rs:1307-1308), so it never reaches the value path and needs no change. `First`/`Last` (check.rs:1162-1182) and `check_for_each`'s source (check.rs:1861-1863), however, currently call `check_expr` and lean on the bare name returning `TypeRef` — they must instead resolve their plain type-name identifier directly to its `TypeDef`. `Delete`/`Next`/`Previous` take an actual **instance value** (a variable typed `TypeRef`), so they are unaffected. With the name positions handled upstream, make `check_ident` emit E0311 + return `Type::Error` whenever an ident resolves to `DeclKind::TypeDef | StructDef | RuntimeTypeDef` — no value-position flag needed; every type name that still reaches `check_ident` is genuinely misused. This also pre-empts the leaky `E0317` for `a = Foo` / `Print Foo`.

**cb-frontend parser** (5 of 18 codes unasserted *via the parser*):
- **E0205** `E_INVALID_TYPE_EXPR` (parser.rs:803,812) — emitted but never asserted.
- **E0207** `E_RESERVED_WORD_AS_NAME` — defined but **never emitted anywhere** (dead code). **Resolution: deleted.** A reserved word can never reach a name slot — the lexer emits `Keyword` tokens (lexer.rs:972-977), so such inputs are rejected earlier as E0201/E0202; the code number is left unused with a tombstone comment. *(This — not E0203 — was the genuinely-dead parser code; the original sweep pointed the suspicion at the wrong code.)*
- **E0208** `E_INVALID_ESCAPE` / **E0209** `E_BAD_RAW_INDENT` — asserted at the `string_value.rs` decoder layer. **A parser-level fixture is not possible:** the string decoder is not wired into `parse()`, so these codes are unreachable via a full parse. Decoder-layer coverage (`escaped_unknown_escape_recovers`, `raw_content_less_indented_errors`) is the only and sufficient coverage; documented as such.
- **E0299** `E_INTERNAL_PARSER` — emitted only from defensive/unreachable-by-construction branches. **Resolution: comment marker at both sites** (`// (E0299) intentionally untested — unreachable defensive guard`); no test.

**Corrections to the original (2026-06-09) sweep, now stale:**
- cb-sema was "6 of 29"; now **7 of 31** — FD-035 added E0330 (already asserted) and the original list missed E0329.
- E0304/E0306/E0307/E0309 line numbers all drifted (now 1028/1251/1239/1279).
- cb-frontend claimed "~11 of 18 unasserted"; the real number is **5**. E0201, E0210, E0211, E0212, E0213, E0214 are all now asserted (snapshot + inline tests).
- **E0203 is NOT dead** — it is emitted on EOF-unterminated blocks (parser.rs:1450/2025/2288) and asserted (parser.rs:4093/4101/5346). The E0203-vs-E0204 split is by design (EOF-without-closer vs wrong-closer-token). No action needed on E0203.

Untested error paths are where spans drift, recovery loops, and messages rot — the exact defect class FD-004/FD-021/FD-027 kept finding by accident.

## Solution

One small test per still-unasserted code, in the established style:

- **cb-sema:** inline tests in `check.rs`'s existing test module (`check_err(source, "E03xx")` pattern). For E0301, pick 2–3 representative emission sites rather than all of them. Add E0304, E0306, E0307, E0309, E0329.
- **cb-frontend:** unit test for E0205 in the existing `recovery_tests`/`decl_tests` modules in `parser.rs`, asserting the diagnostic code and that parsing recovers (no panic, bounded errors). E0208/E0209 keep their decoder-layer coverage only — a parser-level fixture is impossible (the decoder is not wired into `parse()`).
- **E0311 (sema) — implement, don't tombstone:** make `First`/`Last` and `check_for_each`'s source resolve their plain type-name identifier directly (they are always plain identifiers), then emit E0311 + return `Type::Error` in `check_ident` for any ident resolving to `TypeDef`/`StructDef`/`RuntimeTypeDef`. `New` already takes its type name structurally (as a `TypeExpr`), and `Delete`/`Next`/`Previous` take instance values — neither reaches the value path, so they need no change. This also pre-empts the leaky `E0317` message for `a = Foo` / `Print Foo`. Add `check_err` tests for both the mismatched-slot and the matching-slot (`Function f() As Foo : Return Foo`) cases — the latter currently compiles clean and is the real motivation.
- **E0207 (parser) — dead code:** confirm no path can emit it (likely the lexer never yields an identifier token for a reserved word, so the parser hits E0201/E0202 first). If genuinely unreachable, delete the constant and leave a tombstone comment in `parser.rs`.
- **E0299:** leave a `// intentionally untested: unreachable defensive branch` comment rather than forcing a test.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-sema/src/check.rs` | MODIFY | Implement E0311 (type-name-as-value guard, gated for intrinsic slots); ~7-8 inline tests (E0301 ×2–3, E0304, E0306, E0307, E0309, E0329) + E0311 mismatched/matching-slot tests |
| `crates/cb-frontend/src/parser.rs` | MODIFY | E0205 tests; E0207 deleted (tombstone comment); E0299 guards commented |
| `crates/cb-frontend/src/string_value.rs` | NO CHANGE | E0208/E0209 already covered at the decoder layer; not reachable via `parse()` |

## Verification

- `cargo test -p cb-sema -p cb-frontend` green.
- Sweep check: every code listed in `diagnostics.rs` / parser error constants appears in at least one `#[test]` (grep), or carries a comment explaining why not.
- `cargo llvm-cov -p cb-sema --summary-only`: `check.rs` error paths should push line coverage from ~82% toward ~90%.

## Related

- FD-004 (parser correctness — introduced several of these codes), FD-021 (panic safety), FD-027 (diagnostic rendering robustness), FD-035 (type-system simplification — added E0330)
- Coverage analysis session, 2026-06-09; re-verified 2026-06-18 (post-FD-035)
