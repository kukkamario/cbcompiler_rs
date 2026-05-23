# FD-003: Lexer Correctness & Robustness Pass

**Status:** Open
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** Closes correctness gaps and panic-reachability issues identified in the post-FD-001/FD-002 code review. Tightens the lexer's contract ("never aborts") and removes typing decisions that belong in sema, before the IR/sema layer starts depending on lexer output.

## Problem

A workspace-wide code review (2026-05-23) of the lexer surfaced several issues that are bugs today and will be harder to fix once the IR/sema layers depend on lexer output. See `.claude/missing_tests.md` for the matching test gaps.

### Issues

1. **`bump_char` panic is reachable from public input.** `lexer.rs:98` does `expect("bump_char at EOF")`, but `scan_string`'s `Some(_) => { self.bump_char(); }` arm (`lexer.rs:397`) can route there if any earlier byte-level guard ever desynchronizes from a multi-byte boundary. The lexer's own contract at `lexer.rs:3` is "the lexer never aborts" — an `expect` reachable from input violates that.

2. **`scan_one` "should be impossible" recovery is unsound.** `lexer.rs:192-196` documents the case as impossible but then does `self.pos += 1` as fallback. Advancing by 1 mid-UTF-8 breaks the `&str` slicing invariant in `peek_char` on the next iteration.

3. **Lexer makes a typing decision sema should make.** Integer literals are parsed via `stripped.parse::<i64>()` (`lexer.rs:716`; also `:780` hex, `:839` binary). Valid `ULong` literals like `9_223_372_036_854_775_808` (= 2^63) emit `NumberOverflow` *at the lexer*, before the type inferrer ever sees them. The lexer is committing the literal to a signed-64 representation.

4. **Bare `\r` behaviour is not pinned by tests.** All three contexts actually agree with `cb_syntax.md` §1.1 today: `scan_newline` (`lexer.rs:212-227`) and `scan_continuation_or_backslash` (`lexer.rs:236-249`) both treat bare `\r` as a line terminator, and `scan_string` (`lexer.rs:385`) correctly errors on it as `NewlineInString`. The risk is that *no fixture pins any of this end-to-end* — a future regression in any one context (e.g. dropping the `Some(b'\r')` arm) would lex silently differently and only show up far downstream. Need one fixture per context.

5. **Hex/binary "underscore only after prefix" UX is incomplete.** `$_` (prefix + underscore + no digits) emits exactly one diagnostic — "digit separator cannot appear before any hexadecimal digit" pushed at `lexer.rs:732-737`, then the `raw.is_empty()` arm at `:760` short-circuits to an error token *without* the "expected hex digits after `$`" message that the no-`_`-no-digit path gets at `:743`. The user sees a separator complaint when the deeper problem is "no hex digits at all". Same shape in `scan_number_binary` at `:794-799` / `:822-824`.

6. **Unterminated block-comment / raw-string diagnostics have poor span quality.** `scan_block_comment` labels only the 2-byte opener (`lexer.rs:324`) regardless of how many MB were consumed. `scan_raw_string` runs to EOF without a recovery point, so anything after a stray `"""` is silently swallowed (`lexer.rs:441-450`).

7. **`Token` carries `f64` and so cannot be `Eq`** (`token.rs:18`). Comment acknowledges it. Parser `==` sites comparing `FloatLit` tokens silently treat NaN-float-lit tokens as never equal — a latent footgun.

8. **`LONGEST_KEYWORD_LEN = 11` is a magic constant** tied to `endfunction`'s length (defined at `keywords.rs:75`, used at `:83-90` to short-circuit and size the lowercase scratch buffer). A longer keyword added later silently breaks lookup; no test asserts the invariant.

9. **Minor: `UnexpectedChar` vs `InvalidChar` distinction is muddled.** They share the `E_UNEXPECTED_CHAR` error code (`E0106`) but already carry different messages at call sites: `InvalidChar` is paired with "unexpected character \`X\`" (`lexer.rs:173`, `:188`, `:996-1001`); `UnexpectedChar` is paired with "expected hex/binary digits after `$`/`%`" (`:745`, `:809`). The comment at `token.rs:296-297` ("in case we want a different message later") is stale — they already diverge in message, just not in error code. Pick one: (a) split into distinct error codes so renderers can dispatch on them, or (b) collapse the variants and key message choice off the call site.

10. **Minor: `peek_byte_at` does `self.pos as usize + offset` with no overflow guard** (`lexer.rs:79-81`). The lexer uses `u32` byte offsets pervasively; a `debug_assert!(src.len() <= u32::MAX as usize)` at the top of `tokenize` would catch misuse cleanly.

## Solution

Touch `crates/cb-frontend` only. No public-API breaking changes except:
- `Token` may stop being `Copy` (or `IntLit`/`FloatLit` may carry an interned index instead of an inline value — TBD during implementation; either keeps the parser changes minimal).
- `NumberOverflow` for integer literals moves from the lexer to sema. Document in `cb_syntax.md` if the user-visible error code changes.

### Per-issue approach

| # | Approach |
|---|----------|
| 1 | Replace the `expect` in `bump_char` with a saturating advance + internal `Diagnostic` (or `debug_assert!` + cursor-clamp in release). Audit every caller to confirm pre-bump invariants. |
| 2 | Replace `self.pos += 1` recovery in `scan_one` with an unconditional `bump_char` so a "should be impossible" event still advances by a full codepoint. |
| 3 | Parse integer/hex/binary literals into `u64` (or store the raw lexeme bytes as a `Span` + base tag). Range-check against the inferred type in sema. Lexer emits `NumberMalformed` only for shapes that no type could accept (e.g. > 2^64 unsigned literal). The `NumberOverflow` code reserved at `lexer.rs` should be re-tasked or retired. |
| 4 | Behaviour is already spec-conformant; pin it. Add one `\r`-only fixture per context: top-level statements separated by `\r`, `\` continuation followed by a `\r`-only line ending, and a single-line string containing a bare `\r` (must produce `NewlineInString`). Do not change the lexer code for this issue. |
| 5 | In both `scan_number_hex` and `scan_number_binary`, when `raw.is_empty()` after consuming a leading `_`, emit the "expected hex/binary digits after `$`/`%`" diagnostic *in addition to* `InvalidDigitSeparator`. |
| 6 | (a) For unterminated block comments, extend the label span to `[opener.start, self.pos)` so the user sees the swallowed region. (b) For unterminated raw strings, improve label quality: primary label on the opener (where the literal began), secondary label at the EOF cursor (where scanning gave up). A sync rule that recovers mid-file is deferred — see "Out of scope" below. |
| 7 | Wrap `FloatLit` in a newtype that hashes via raw bits (`f64::to_bits`) and implements `Eq` via bit-equality. NaN-equality becomes "same NaN payload" rather than "never equal" — sufficient for parser-side `==` and far less surprising. Alternative: intern float literals and reference them by index. |
| 8 | Either compute `LONGEST_KEYWORD_LEN` via a `const fn` over the keyword table, or add a `#[test]` that asserts it equals the longest entry. The const-fn option is preferred. |
| 9 | Decide whether the variants are load-bearing. Recommended: collapse into a single `UnexpectedChar` variant — the distinction adds no information that the call site's message doesn't already convey, and there is no consumer that switches on the variant. Drop the stale "different message later" comment in `token.rs`. |
| 10 | Add `debug_assert!(src.len() <= u32::MAX as usize, "source too large for u32 offsets")` at the top of `tokenize`. |

### Out of scope

- The `Token` representation rework is bounded: keep `Token` `Copy` if possible by using a `FloatBits(u64)` newtype rather than interning.
- Bare `\r` test corpus stays small (one fixture per context); not adding a Mac-line-endings linting feature.
- Integer-literal range checking in sema is *not* implemented here — only the migration of the typing decision out of the lexer. Sema will land alongside the IR/sema FD.
- Raw-string mid-file recovery (the "sync rule" originally sketched in issue 6(b)): deferred. The heuristic ("stop at the next newline that ends with only whitespace before another `\"\"\"`-or-EOF") is non-trivial to define crisply and has no concrete bug forcing it today. FD-003 lands label-quality improvements only; a follow-up FD can revisit recovery if real-world unterminated-raw-string cases prove disruptive.
- Dedicated snapshot fixtures `bare_cr.cb` / `numeric_boundaries.cb`: coverage of those code paths landed as unit tests in `tests/lexer_units.rs` instead. Pinning them in snapshots adds no signal beyond what the unit assertions already give.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-frontend/src/lexer.rs` | MODIFY | Fix bump_char panic reachability, scan_one recovery, hex/binary UX, block-comment / raw-string span quality, `u64` literal parsing, `debug_assert!` on input size. |
| `crates/cb-frontend/src/token.rs` | MODIFY | `FloatBits(u64)` newtype for `FloatLit`. Collapse `InvalidChar` into `UnexpectedChar` and drop the stale comment. Update `IntLit` to `u64`. |
| `crates/cb-frontend/src/keywords.rs` | MODIFY | Replace `LONGEST_KEYWORD_LEN` constant with a derived const-fn or add a test. |
| `crates/cb-frontend/tests/lexer_units.rs` | MODIFY | Add unit tests per issue (see Verification). The bare-`\r` and numeric-boundary cases land here rather than as snapshot fixtures — see "Out of scope". |
| `crates/cb-frontend/tests/lexer_props.rs` | MODIFY | Add property: every non-Eof token span satisfies `start < end <= src.len()`. |
| `crates/cb-frontend/src/parser.rs` | MODIFY | Adjust any `==` comparison on `Token`/`TokenKind` that depended on the dropped `Eq` impl (likely none — verify). |
| `docs/cb_syntax.md` | MODIFY | If error-code semantics for integer overflow change, document the new lexer-vs-sema responsibility split. |

## Verification

- `cargo test -p cb-frontend` green.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- New unit/snapshot tests added for each issue, taken from `.claude/missing_tests.md` "Lexer" section. Specifically:
  - `bump_char` no longer reachable as a panic — proptest on arbitrary UTF-8 still terminates without panic (already covered) *plus* a regression test that feeds a synthesized cursor-misalignment-like input through a malformed-source fuzz seed.
  - `9223372036854775808` lexes successfully as `IntLit(u64)` and survives parser/AST traversal; sema is responsible for range-checking once it exists.
  - `\r`-only newline fixture: a 3-line file using only `\r` separators tokenizes into the same shape as `\n`/`\r\n`.
  - `$_`, `%_` (prefix + underscore + no digits) emit *two* diagnostics in order: "expected hex/binary digits after `$`/`%`" and `InvalidDigitSeparator`.
  - Unterminated `/* …` with 1 KiB of contents produces a diagnostic whose label covers the full 1 KiB, not just 2 bytes.
  - Unterminated `"""…` emits a diagnostic with a primary label on the 3-byte opener and a secondary label at the EOF cursor (mid-file recovery is deferred — see "Out of scope").
  - `FloatBits` round-trip test: two `FloatLit(NaN)` tokens with the same payload compare equal; with different payloads, unequal.
  - `LONGEST_KEYWORD_LEN` invariant test (or const-fn derivation makes the test redundant).
- Bench-eyeball: `cargo bench` does not exist yet; a microbenchmark is out of scope. Just confirm tokenizer wall time on `tests/fixtures/parser_*.cb` is unchanged within noise.

## Related

- `docs/cb_syntax.md` §1.1 (line terminators), §1.6 (numeric literals)
- `.claude/missing_tests.md` — "cb-frontend — Lexer", "cb-frontend — String value" sections
- `docs/features/archive/FD-001_LEXER.md` — original lexer FD; this is the follow-up correctness pass
- FD-004 (parser correctness) — overlaps on the `Continuation` token's parser-side handling, but lexer-side is unchanged
