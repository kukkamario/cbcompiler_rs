# FD-028: Syntax Fidelity for Legacy Code

**Status:** Open
**Priority:** High
**Effort:** Medium (1-4 hours)
**Impact:** Brings the lexer/parser back in line with real CoolBasic so legacy `.cb` programs parse correctly. Corrects four syntax divergences that currently make us reject or misinterpret valid CoolBasic source.

## Problem

Our frontend diverges from the original CoolBasic language in several places. These were introduced when the syntax was (incorrectly) documented/implemented from memory, and they break compatibility with existing CoolBasic code:

1. **`\` is not integer division — it is the `Type` field accessor.** Real CoolBasic writes `player\x` to read field `x` of the `Type` instance `player`. We currently lex `\` as `Op::BackSlash` and parse it as integer division (`BinOp::IntDiv`). There is no `\` integer-division operator in CoolBasic at all.
2. **`.` is not the field accessor in real CoolBasic — but we want to support it anyway.** The original uses only `\`. We currently use `.` (`Punct::Dot` → `Expr::Field`). We want to support **both** `\` and `.` as field accessors so both legacy (`player\x`) and dotted (`player.x`) styles work.
3. **The exponent operator is `^`, not `**`.** We currently lex `**` (`Op::StarStar`) → `BinOp::Pow`. CoolBasic uses `^`. (`^` is otherwise unused in our lexer; bitwise XOR is the `Xor` keyword, so there is no clash.)
4. **`'` (apostrophe) starts an inline comment.** Classic BASIC/CoolBasic uses `'` to begin a line comment. We currently only recognise `Rem`, `//`, and block `/* */`. `'` is currently unhandled by the lexer.

## Solution

All changes are in `cb-frontend` (lexer/parser/AST), with downstream cleanup in `cb-sema`, `cb-ir`, and `cb-backend-interp` for the removed integer-division path. Update `docs/cb_syntax.md` to match.

### Item 1 + 2 — `\` and `.` as field accessors; remove integer division

- **Lexer:** keep emitting a token for a standalone `\` (the line-continuation case `\`+newline is unchanged). Repurpose it from an arithmetic operator to a field-access punctuator.
- **Parser:** treat `\` as a **postfix** field accessor with the same binding power as `.` (see `postfix_bp`, currently handles `(`, `[`, `.` at `parser.rs:2800`). Both `\` and `.` produce `Expr::Field`. Remove `\` from the infix binding-power table (`parser.rs:2771`) and remove the `Op::BackSlash => BinOp::IntDiv` mapping (`parser.rs:2820`).
- **Remove the integer-division path entirely** (it has no source syntax left): `BinOp::IntDiv` (`ast.rs:328`), `IrBinOp::IntDiv` (`inst.rs:104`, printer at `print.rs:318`), sema typing (`types.rs:233` + tests `372-373`), lowering (`lower.rs:935`), and interpreter eval arms (`interp.rs:956,1040,1094`).
- **Const folding / div-by-zero:** the `E0322` const div-by-zero check (`check.rs:1379`) was documented against `\`; integer div-by-zero now arises only from `/` between integer operands and `Mod`. Keep the check for those; drop `\` from its wording.
- **Invariant — `/` semantics are unchanged by this removal.** `/` already does **integer division when both operands are integers** and **floating-point division when either operand is a `Float`** (the float operand promotes both, per `cb_syntax.md` §1.7). This is the existing, correct behavior: sema types `Div` via `numeric_promote` (`types.rs:228`, `Int / Int → Int`); interp uses `wrapping_div` on the integer path (`interp.rs:956`) and float division on the float path (`interp.rs:1093`). Removing `IntDiv` only collapses the combined `Div | IntDiv` match arms down to `Div` — it must **not** alter `/`'s int-vs-float behavior. Add/keep a test asserting `7 / 2 == 3` (Int) and `7.0 / 2 == 3.5` (Float).

### Item 3 — exponent `^` replaces `**`

- **Lexer:** add `^` → a new `Op::Caret` token; remove `**` (`Op::StarStar`, lexer `lexer.rs:1000`, token `token.rs:270`).
- **Parser:** map `^` to `BinOp::Pow` with the same right-associative binding power `**` had (`parser.rs:2774`, `2821`), preserving the `-2^2 = -(2^2)` precedence (`parser.rs:2790`).
- `BinOp::Pow` / `IrBinOp::Pow` and all downstream Pow handling are unchanged — only the surface token changes.

### Item 4 — `'` inline comment

- **Lexer:** add `'` to the dispatch (near `lexer.rs:166`) to scan a line comment to end-of-line, emitting `TokenKind::Comment(CommentKind::Line)` (same as `//` / `Rem`).

## Resolved decisions

- **D1 (Item 3):** **Replace** `**` outright with `^`. `**` is removed from the lexer; only `^` produces `BinOp::Pow`.
- **D2 (Item 4):** **Keep all** existing comment forms — `//`, block `/* */`, and `Rem` stay — and **add** `'` as an additional inline-comment introducer.
- **D3 (Item 1):** `\` and `.` are **fully interchangeable** — same postfix binding power, both produce `Expr::Field`, freely mixable in chains like `a\b.c\d`.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-frontend/src/token.rs` | MODIFY | Remove `Op::StarStar` & `Op::BackSlash`-as-operator; add `Op::Caret`; clarify `\` punctuator role |
| `crates/cb-frontend/src/lexer.rs` | MODIFY | `'` → line comment; `^` → `Op::Caret`; drop `**`; `\` token role |
| `crates/cb-frontend/src/parser.rs` | MODIFY | `\` & `.` as postfix field access; `^` → `Pow`; remove `**` and `\`-as-IntDiv |
| `crates/cb-frontend/src/ast.rs` | MODIFY | Remove `BinOp::IntDiv` |
| `crates/cb-ir/src/inst.rs` | MODIFY | Remove `IrBinOp::IntDiv` |
| `crates/cb-ir/src/print.rs` | MODIFY | Remove `int_div` printer arm |
| `crates/cb-sema/src/types.rs` | MODIFY | Remove `IntDiv` typing + tests |
| `crates/cb-sema/src/lower.rs` | MODIFY | Remove `IntDiv` → IR mapping |
| `crates/cb-sema/src/check.rs` | MODIFY | Reword `E0322` const div-by-zero (drop `\`) |
| `crates/cb-backend-interp/src/interp.rs` | MODIFY | Remove `IntDiv` eval arms |
| `crates/cb-frontend/tests/lexer_units.rs` | MODIFY | Update `\` tests; add `'` comment + `^` tests |
| `crates/cb-frontend/tests/parser_snapshots.rs` | MODIFY | Remove `IntDiv` arm; add `\` field-access + `^` snapshots |
| `docs/cb_syntax.md` | MODIFY | §1.2 comments (`'`), §1.7 operators (`^` not `**`, drop `\` arithmetic), §5 precedence, field-access via `\` and `.`, trap list |

## Verification

- `cargo test -p cb-frontend` — lexer + parser snapshots (new `\` field access, `^`, `'` comment; no `**` / `\`-division).
- `cargo test -p cb-sema` — typing/lowering snapshots no longer reference `IntDiv`.
- `cargo build --workspace` — confirms no dangling `IntDiv` match arms.
- Smoke test a legacy snippet: `player\x = player.x + 2 ^ 3   ' comment` parses with field access on both sides, exponent, and trailing comment.

## Related

- `docs/cb_syntax.md` — §1.2 (comments), §1.7 (operators), §5 (precedence), §6 (`Type` field access), trap list (§ on division by zero)
- Supersedes the `\`-integer-division removal originally raised under FD-020; FD-020 (Sema Numeric & For-Loop Semantics) must drop its `1 \ 0` example and `\` references once this lands.
