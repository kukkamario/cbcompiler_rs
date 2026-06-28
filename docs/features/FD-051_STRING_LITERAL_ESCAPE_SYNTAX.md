# FD-051: String Literal Syntax — Verbatim `"..."`, Escapes Move to `$"..."`

**Status:** Open
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** Plain `"..."` strings become verbatim (no escape surprises, e.g. Windows paths); a new `$"..."` form opts into the C-style escapes that `"..."` does today.

## Problem

Today a single-line `"..."` literal processes C-style escapes (`\n`, `\t`, `\xNN`, `\uNNNN`, `\"`, `\\`; see `cb_syntax.md` §1.6). That means a backslash inside an ordinary string is "magic": `"C:\new"` silently becomes `C:<LF>ew`, and embedding a literal backslash requires `\\`. Classic CoolBasic strings had **no** escapes — `"\n"` was the two characters backslash-n — so the current behaviour also diverges from the dialect we're reimplementing.

We want the common case to be unsurprising and verbatim, while still keeping a way to write control characters and Unicode escapes:

- **`"..."` becomes verbatim** — no escape processing. `\` is an ordinary character, `"` always closes the literal.
- **`$"..."` is the new escaped form** — it processes exactly the escape set `"..."` handles today.

The `$` prefix is a mode marker only; this is **not** string interpolation. Triple-quoted `"""..."""` raw strings are unchanged.

## Solution

The whole change lives in `cb-frontend`; decoding already happens at parse time and bakes a finished `String` into the AST, so `cb-ir` and both backends are untouched.

**Kind is chosen by delimiter, not by content.** Today the lexer picks `StrLitKind::Plain` vs `Escaped` by whether it saw a `\`. After this change the *delimiter form* decides:

- `"..."`   → verbatim (no escapes)
- `$"..."`  → escaped (current escape set)
- `"""..."""` → raw (unchanged)

The three `StrLitKind` variants stay; only their selection rule and doc-comments change (rename `Plain`→`Verbatim`, `Escaped`→`Dollar` optional, for clarity).

**Lexer (`lexer.rs`).**
- At a token-start `$`, peek one byte: if it is `"`, scan a `$"..."` escaped string; otherwise fall through to the existing hex-radix scan (`$2f4E4`). The lookahead is unambiguous — a hex literal needs a following hex digit, and the String *sigil* `$` only ever appears glued to the end of an identifier (handled in `scan_ident`), never at token start.
- Verbatim `"..."` scan: stop tracking backslashes for kind selection; `\` is just a body byte and `"` always terminates (there is no `\"` in a verbatim string). Newline-in-string stays an error.
- `$"..."` scan: reuse the current escape-aware body loop (a `\` still escapes the following char, so `\"` does not terminate). Tag the token `Escaped`.
- Recovery messages that suggest `\n` (newline-in-string, backslash-before-newline) must branch: a verbatim `"..."` should point the user to `$"\n"` or `"""..."""`; the `$"..."` form keeps the `\n` advice.

**Decoder (`string_value.rs`).**
- `decode_plain` already returns the body verbatim — it now serves every `"..."`.
- `decode_escaped` must strip the leading `$` in addition to the quotes, and shift `body_offset_in_lit` from 1 to 2 (`$` + `"`) so escape-diagnostic spans stay byte-aligned (this offset is load-bearing — see the F-L14 regression test).
- `decode_raw` unchanged.

**Docs.** Rewrite `cb_syntax.md` §1.6 "String literals": `"..."` is verbatim, `$"..."` carries the escape table, `"""..."""` stays raw. Note explicitly that `$"..."` is not interpolation and that a literal `"` inside a string needs either `$"\""` or a raw `"""..."""`.

Out of scope: a `$"""..."""` form, and any interpolation. `$"..."` stays single-line like the escaped form it replaces.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-frontend/src/lexer.rs` | MODIFY | `$"` lookahead → escaped-string scan; verbatim `"..."` scan no longer keys kind on `\`; branch the newline/continuation recovery messages |
| `crates/cb-frontend/src/token.rs` | MODIFY | Re-document `StrLitKind` selection (kind = delimiter form); optional rename `Plain`→`Verbatim`, `Escaped`→`Dollar` |
| `crates/cb-frontend/src/string_value.rs` | MODIFY | `decode_escaped` strips the `$` prefix and shifts the body offset to 2; refresh/extend unit tests |
| `crates/cb-frontend/src/parser.rs` | MODIFY | Update the StrLit tests whose expected kind now follows the delimiter, not the content; parse path itself unchanged |
| `docs/cb_syntax.md` | MODIFY | Rewrite §1.6 string-literal section for the verbatim / `$"..."` / raw split |

## Verification

- `cargo test -p cb-frontend` — lexer, `string_value`, and parser suites green.
- New/updated cases:
  - `"a\nb"` → the four chars `a \ n b` (verbatim); `$"a\nb"` → `a<LF>b`.
  - `"C:\new"` → verbatim `C:\new` (the motivating path case).
  - `$"\xFF"` → U+00FF; `$"é"` → `é`; `$"\""` → `"`.
  - `$"\q"` → E0208 with the diagnostic span on the `\q`, including a non-zero literal start (port the existing F-L14 / absolute-span regression to the `$`-prefixed form).
  - Disambiguation: `$ff` still lexes as a hex literal, `name$` still lexes as an identifier + sigil, and only `$"` starts an escaped string.
- Differential suite (`diff_llvm`) unaffected — a `**/*.cb` grep for backslash escapes is currently clean, so no fixtures change meaning.

## Related

- `docs/cb_syntax.md` §1.6 — the authoritative string-literal spec this FD rewrites.
- [FD-002](archive/FD-002_PARSER.md) — established the original escape set and the verbatim-recovery choices in `string_value.rs`.
- String model is Unicode code points (not bytes) — `\xNN` stays U+00NN under `$"..."`; keep that invariant.
