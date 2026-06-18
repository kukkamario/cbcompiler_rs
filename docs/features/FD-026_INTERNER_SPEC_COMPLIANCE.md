# FD-026: Identifier Interner Spec Compliance

**Status:** Pending Verification
**Priority:** Medium
**Effort:** Low-Medium (1-2 hours)
**Impact:** Aligns the one place that defines identifier identity for the whole compiler with `cb_syntax.md`, and stops error messages from showing lowercased identifiers instead of what the user wrote.

## Problem

`cb-diagnostics` is a small, well-written foundation crate, but the post-FD-018 review found the interner diverges from the spec in ways that affect every downstream identifier comparison and every error message that names a symbol.

1. **The interner uses `to_lowercase`, not the Unicode *simple case folding* the spec mandates.** `cb_syntax.md:48` states identifiers are compared "using Unicode simple case folding"; `intern.rs:43` uses `str::to_lowercase` (full lowercasing). These genuinely differ for some characters (e.g. final sigma U+03C2, certain titlecase/special mappings), so some identifier pairs the spec calls equal are treated as distinct (or vice versa). Rare in real CoolBasic source, but this is *the* definition of identifier identity for the compiler. *(Review note: the often-cited ß and Kelvin-sign examples are not actually divergences under **simple** folding; the principle — `to_lowercase ≠ simple case folding` — still holds.)*
2. **`resolve()` returns the lowercased canonical form, discarding original spelling.** `intern()` stores only the folded key (`strings.push(key.clone())`, `intern.rs:48`), so `resolve()` can only ever return lowercase. Downstream sema/IR error text resolves symbols straight into user-facing messages (`cb-sema/src/check.rs:682, 1701`; `cb-ir/src/print.rs:39`), so a user who wrote `PlayerHealth` sees `playerhealth` in diagnostics. Most compilers preserve the original spelling for display while comparing case-insensitively.

Lower-severity items folded in:

- **`Symbol::DUMMY` shares the `u32` namespace with real symbols, unguarded.** `Symbol::DUMMY = Symbol(u32::MAX)` (`intern.rs:15`) and `intern()` mints `Symbol(self.strings.len() as u32)` from 0 with no overflow assertion (`:47`), so the 4-billion-th string would collide with `DUMMY` and `len() as u32` truncates rather than panics. `SourceMap` deliberately asserts against its own `u32::MAX` sentinel (`source.rs:131-136`); the interner doesn't mirror that discipline. (Realistically unreachable, but a trivially-closed asymmetry.)
- **`SourceMap::add` integrity check on same-name/different-text is debug-only** (`source.rs:102`): in release a second `add("foo.cb", different_text)` silently returns the original `FileId` and drops the new text, so diagnostics would render against stale source with no signal.

## Solution

In `cb-diagnostics`:

- Adopt a simple-case-folding implementation for the intern key (e.g. the `caseless`/`unicode-case-mapping` crate, or an explicit ASCII-only restriction documented in `cb_syntax.md`). If an external crate is undesirable, the user's call on whether to amend the spec to say "ASCII case-insensitive / lowercasing" instead — flag this at review.
- Store the original spelling alongside the fold key (e.g. `strings` holds the first-seen original; a separate map keyed by the fold) so `resolve()` returns the original casing, or add a `resolve_display()` variant used by diagnostic-rendering call sites. At minimum document the lossy behavior.
- Use `u32::try_from(self.strings.len())` and assert the result `!= u32::MAX`, matching `SourceMap::push_source`.
- For `SourceMap::add`, either return a `Result`/log in release when texts differ, or document and test the dedupe contract in both build modes.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-diagnostics/src/intern.rs` | MODIFY | Simple case folding for the key; preserve original spelling; `Symbol::DUMMY` overflow guard |
| `crates/cb-diagnostics/src/source.rs` | MODIFY | Harden/clarify `SourceMap::add` same-name/different-text behavior |
| `crates/cb-diagnostics/Cargo.toml` | MODIFY (if a folding crate is chosen) | Add the case-folding dependency |
| `crates/cb-diagnostics/tests/` | MODIFY | Tests: a non-ASCII pair distinguishing folding from lowercasing; `resolve()` returns original spelling; `DUMMY` non-collision; `add` divergent-text behavior |
| `docs/cb_syntax.md` | MODIFY (if the rule is amended) | Pin the exact identifier-comparison rule chosen |

## Verification

- `cargo test -p cb-diagnostics` green, with new tests:
  - An identifier pair where simple-case-folding and `to_lowercase` differ interns to the chosen rule.
  - `resolve()` of an interned `PlayerHealth` returns `PlayerHealth` (or `resolve_display()` does).
  - Interning never mints `Symbol(u32::MAX)`; resolving `DUMMY` behaves as documented.
- Diagnostic snapshot(s) in `cb-sema`/`cb-driver` now show original-cased identifiers — update goldens accordingly.
- `cargo test --workspace` + `clippy -- -D warnings` green.

## Implementation Notes (2026-06-18)

Implemented on branch `fd-026-interner-spec-compliance`. User chose **full Unicode
simple case folding via a crate** (option B) over an ASCII restriction, because
Nordic identifiers (`ä`/`Ä`, `ö`/`Ö`, `å`/`Å`) must compare case-insensitively.

**Crate:** [`unicode-case-mapping`](https://docs.rs/unicode-case-mapping) (`= "1"`,
Unicode 16.0). Its `case_folded(char) -> Option<NonZeroU32>` is exactly *simple*
folding (one scalar → one scalar; `None` = the char is its own fold) — not the
full folding `caseless` does (`ß` → `ss`), which would diverge from the spec. Added
as a workspace dep and a `cb-diagnostics` dep. Confirmed `ß` stays `ß` under simple
folding, matching the FD's review note.

**`intern.rs`:**
- New `pub fn fold(name: &str) -> String` (the canonical simple-fold key) and a
  private `fold_char`. `intern` keys the map by `fold(name)` but pushes the
  **original** spelling into `strings`.
- `resolve()` now returns the **first-seen original spelling** (chosen over a
  separate `resolve_display()` — fewer call sites change, and IR dumps / every
  diagnostic get correct casing for free). The `@main` lookup (`interp.rs`) and
  `IrType::RuntimeType` string are safe: `@main` has no cased chars, and the
  RuntimeType name is never matched case-sensitively at runtime (FFI treats it as
  an opaque pointer; the catalog decodes by its own C-side names).
- **Overflow guard:** `u32::try_from(len)` + `assert!(id != u32::MAX)` so the
  4-billion-th name can't silently collide with `Symbol::DUMMY` — mirrors
  `SourceMap::push_source`.

**Non-obvious fallout — intrinsic dispatch (the load-bearing fix):** intrinsics
(`Len`/`Int`/`Str`/`First`/`Last`/`Next`/`Previous`/`Float`/`Integer`) were
matched by comparing `resolve(callee)` against lowercase constants in
`check.rs::check_intrinsic_call` and `lower.rs::lower_call`. That only worked
because `resolve` *used* to lowercase. Both sites now match on
`cb_diagnostics::fold(name)` (keeping the original `name` for `{name}` in error
messages). Without this, `Len(x)` etc. silently stopped being recognized as
intrinsics (caught by the existing `pass2_intrinsic_*` tests).

**`source.rs`:** `SourceMap::add` same-name/different-text promoted from
`debug_assert_eq!` to a hard `assert_eq!`, so the integrity violation signals in
**release** too (was a silent stale-source render). No caller legitimately passes
divergent text (driver adds one file per run); zero ripple.

**Spec:** `cb_syntax.md` §1.3 already mandated "Unicode simple case folding" — we
conformed to it rather than amending, so no doc change was needed.

**Tests:** interner unit tests for Nordic case-insensitivity, the simple-fold ≠
`to_lowercase` divergence (Greek final sigma `ς`/`σ`/`Σ` collapse to one symbol),
and `resolve` returning original spelling; `resolve_round_trip` updated to expect
`MyVar`. New `#[should_panic]` test for the hardened `add`. 16 lowering/print
snapshots (`cb-ir`, `cb-sema`) re-accepted — all diffs were name casing only
(`TypeRef(mob)` → `TypeRef(Mob)`), no IR structure change.

**Verified (2026-06-18, Windows):** `cargo test --workspace` all green (0 failed);
`cargo clippy --workspace --all-targets -D warnings` clean; `cargo fmt --all
--check` clean. Driver smoke tests: undeclared `MissingThing` renders with its
original casing (was `missingthing`); `PlayerHealth`/`playerhealth` resolve to one
variable; `Dim Hämäläinen` referenced as `HÄMÄLÄINEN`/`hämäläinen` runs and prints
`42` (exit 0).

## Related

- Surfaced by the post-FD-018 codebase review (diagnostics area).
- [FD-007](archive/FD-007_Semantic_Analysis.md) — introduced the `Symbol`/`Interner` with case-insensitive dedup and the symbol-into-error-message resolution this corrects.
- `docs/cb_syntax.md` §identifiers (Unicode simple case folding).
