# FD-026: Identifier Interner Spec Compliance

**Status:** Open
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

## Related

- Surfaced by the post-FD-018 codebase review (diagnostics area).
- [FD-007](archive/FD-007_Semantic_Analysis.md) — introduced the `Symbol`/`Interner` with case-insensitive dedup and the symbol-into-error-message resolution this corrects.
- `docs/cb_syntax.md` §identifiers (Unicode simple case folding).
