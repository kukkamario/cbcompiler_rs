# FD-004: Parser Correctness & Small Spec Gaps

**Status:** Pending Verification
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** Closes correctness bugs and small spec gaps found in the post-FD-002 code review. Fixes one panic-reachability case in the statement parser, makes the `\` line-continuation token actually work, and lifts several documented-but-unparseable forms.

## Problem

A workspace-wide code review (2026-05-23) of the parser surfaced bugs and small documented-but-unparseable forms. The substantial `Delete` statement (`cb_syntax.md` §3.3) is scoped out to FD-005 because its runtime semantics are non-trivial. See `.claude/missing_tests.md` for matching test gaps.

### Issues

1. **`Continuation` tokens are never consumed by the parser.** The lexer emits `TokenKind::Continuation` (`lexer.rs:253`), but no parser code path consumes or skips it. The spec example `total = a + b + c + \\\n d + e + f` (§1.1) is silently broken today: with `emit_trivia: false` the token is filtered, but the spec expects it to fuse lines unconditionally regardless of options. Multi-line expressions do not parse.

2. **Single-line `If` can reach `unreachable!`.** `parse_if` (`parser.rs:1336-1338`) calls `parse_single_line_body_stmt` → `parse_stmt_inner`. For inputs like `If x Then : a = 1\n` (`Then` immediately followed by `:`) or `If x Then\n` reaching certain code paths, `parse_stmt_inner`'s `_` arm at `:911-918` is hit with a confusing "unexpected token" — and worse, the `unreachable!` for `Newline | Eof` at `:922-924` is reachable in adjacent inputs. The partially-built If body retains incomplete state.

3. **`Int`/`Integer` aliasing is asymmetric.** `keywords.rs:41-42` maps `"int" → Kw::Integer`; there is no `Kw::Int`. The lexer loses the user-written spelling. Future diagnostics that say "expected `Int`, found `Long`" will be a lie. Same for `UInt`/`UInteger`.

4. **`z As String = "asd"` (spec §4.1 implicit decl with annotation) is rejected.** Parser has no dispatch for the `Ident As Type = expr` statement form; falls through to `parse_expr_or_assign_stmt`'s expression-statement branch and errors on `As`.

5. **`Next foo%` (sigilled name after `Next`) is silently ignored.** `parser.rs:1631-1640` only accepts sigil-less idents. With a sigilled `For` variable (`For i% = 0 To 10`), there is no spellable `Next` and the sigilled name falls through as a separate statement.

6. **`New T[]` returns a malformed `NewKind::Array { dims: [] }`** after emitting a diagnostic (`parser.rs:575-581`). Downstream code that assumes `dims.is_empty() → error` may consume the node and build malformed IR.

7. **`Field` outside a Type body drops the rest of the line** (`parser.rs:889-894`), including a possible trailing `:` separator and the next statement on the same line.

8. **`Redim`'s element type uses `parse_type_atom`.** `Redim arr2 As Integer[][10]` (resize a `Integer[]` variable, with one dim) isn't expressible (`parser.rs:2205`). Document the restriction or widen the grammar.

9. **`Stmt::Error` from the forced-progress guard reuses `bad.span`** (`parser.rs:812`) even when the bumped token is far past the original error position — Error nodes get spans over freshly-bumped tokens, scattering with confusing positions.

10. **`Select` silently accepts multiple `Default` arms.** Spec is ambiguous; pin behaviour and reject the second `Default` with a diagnostic.

11. **`Case 1, 2, 3` comma-separated case values:** `parse_case_arm` supports them at `parser.rs:1704-1707` but no fixture exercises the path — likely working but unverified.

12. **Minor: `Expr::Field { target, name }` wraps the field name as a full `Ident` node** (`ast.rs:110-113`); every other declaration site uses a bare `Span`. Inconsistent and bloats the arena for deep `.a.b.c.d` chains. Sema will have to special-case Field-name lookups.

13. **Minor: `parse_select`'s "Defensive" fallback** (`parser.rs:1294-1301`) silently returns without a diagnostic. Promote to an internal-error diagnostic so a future maintainer gets a clear failure.

14. **Minor: `Cursor::bump` returns the same Eof token's zero-length span repeatedly** (`parser.rs:105-111`). Callers that accumulate `bump().span` in a loop receive identical zero-length spans — latent footgun.

15. **Minor: `STMT_LHS_MIN_BP = 17` is a magic constant** with a careful comment; if `CMP_LBP` changes, the comment stays valid but the constant silently breaks. Derive: `const STMT_LHS_MIN_BP: u8 = CMP_LBP + 1;`.

16. **Nit: W2/W3/W4/W5/W6 phase markers** (`parser.rs:783, 1159, 1733`) are dev-process noise from FD-002 that survived into committed code. Remove.

17. **Nit: `E_BAD_RAW_INDENT`, `E_INVALID_ESCAPE` constants** declared in `parser.rs:29-30` are only emitted from `string_value`. Relocate.

## Solution

Touch `crates/cb-frontend` only. No frontend-external surface changes.

### Per-issue approach

| # | Approach |
|---|----------|
| 1 | At every place that consumes `Newline` (`Cursor::bump`, `eat_newlines`, expression continuation points), also consume `Continuation`. Concretely: introduce a `Cursor::skip_continuations(&mut self)` helper that bumps any `Continuation` tokens at the front of the lookahead, and call it from `Cursor::peek`/`Cursor::bump` so the rest of the parser is unaware of the token kind. Verify with a snapshot fixture `continuation_multi_line.cb`. |
| 2 | In `parse_if`, when entering the single-line branch, peek the next non-trivia token *before* calling `parse_single_line_body_stmt`. If it is `Colon`, `Newline`, `Eof`, or `Kw::Else`, emit `E_EMPTY_SINGLE_LINE_IF_BODY` (new code, e.g. `E0215`) and record an empty body; do not recurse. Replace the `unreachable!` at `parser.rs:922-924` with an internal-error diagnostic — it should remain unreachable, but stop being a hard panic. |
| 3 | Either (a) introduce `Kw::Int`/`Kw::UInt` and have the keyword table return the spelling-preserved variant, with parser/sema treating them as aliases; or (b) keep the table as-is and add a `lexeme: Span` to the type-keyword AST node so diagnostics can render the user's spelling. (a) is simpler. |
| 4 | Add an LHS+`As`+`Type`+`=` dispatch to `parse_expr_or_assign_stmt` (or to a new `parse_implicit_decl_stmt`). Produces a `Stmt::Dim` (or a new `Stmt::ImplicitDecl`) with span over the full statement. Pin behaviour with a fixture. |
| 5 | In `parse_for`, accept a sigilled ident after `Next` and either (a) compare the sigil to the loop-var sigil and error on mismatch, or (b) accept any sigil and let sema verify the match. (a) is preferred since the parser already inspects the loop-var. |
| 6 | When `New T[]` is rejected, return `Expr::Error` (with the diagnostic span) instead of `NewKind::Array { dims: [] }`. Audit any other diagnostic-and-return site for the same pattern. |
| 7 | In the stray-`Field` recovery loop (`parser.rs:889-894`), stop at the first `Colon` or `Newline` instead of consuming the whole line. |
| 8 | Pick (b): widen the grammar so `Redim arr As <ArrayElementType>[]…` accepts the rank marker. Cheap — `parse_type_expr` already does this; swap `parse_type_atom` for `parse_type_expr` and check the existing tests still pass. |
| 9 | When the forced-progress guard creates a `Stmt::Error`, span it over `[original_error_span.start, bumped_token.span.end]` so the Error node visually covers the recovered range. |
| 10 | Track `seen_default: bool` in `parse_select`; emit `E_DUPLICATE_DEFAULT` (new code) on second `Default`. |
| 11 | Add a snapshot fixture `case_comma_list.cb` (no code change). |
| 12 | Change `Expr::Field { target, name }` to `Expr::Field { target, name_span: Span }`. Update consumers in driver/printer. |
| 13 | Replace the silent return in `parse_select`'s defensive branch with an internal-error diagnostic. |
| 14 | In `Cursor::bump`, if the cursor is at Eof, return an Eof token whose span is `[src.len(), src.len()]` (already the case via lexer) but mark via debug-assert that callers should not loop on it. Document the contract on `Cursor::bump`. |
| 15 | `const STMT_LHS_MIN_BP: u8 = CMP_LBP + 1;`. |
| 16 | Delete W2-W6 markers. |
| 17 | Move `E_BAD_RAW_INDENT` and `E_INVALID_ESCAPE` constants into `string_value.rs`; re-export from `parser.rs` if used there. |

### Out of scope

- `Delete` statement (FD-005).
- Lexer changes that would also fix `Continuation` (e.g. always filtering it before the parser sees the stream) — handled here at the parser level so the lexer remains the source of truth for source-byte fidelity.
- `For Each` over arbitrary expressions — not in the review findings.
- Numeric `For` `Step` with float literal — already works structurally; add a snapshot fixture, but not a code change.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-frontend/src/parser.rs` | MODIFY | Issues #1-#11, #13-#16. New error codes `E0215` (empty single-line If body), `E0216` (duplicate Default). |
| `crates/cb-frontend/src/ast.rs` | MODIFY | Issue #12: `Expr::Field` carries `name_span: Span` instead of an Ident node. |
| `crates/cb-frontend/src/keywords.rs` | MODIFY | Issue #3: `Kw::Int`/`Kw::UInt` aliases. |
| `crates/cb-frontend/src/token.rs` | MODIFY | Issue #3: enum variants. |
| `crates/cb-frontend/src/string_value.rs` | MODIFY | Issue #17: relocate error-code constants. |
| `crates/cb-frontend/tests/parser_snapshots.rs` + fixtures | MODIFY | Add fixtures: `continuation_multi_line.cb`, `single_line_if_empty.cb`, `implicit_decl_as.cb`, `next_with_sigil.cb`, `case_comma_list.cb`, `select_duplicate_default.cb`, `redim_array_element_type.cb`. |
| `crates/cb-frontend/tests/parser_props.rs` | MODIFY | Extend `safe_source` generator to occasionally emit `Continuation` between expression operators. |
| `crates/cb-driver/src/main.rs` | MODIFY | Issue #12: `Expr::Field` printer arm adapts to span-only field name. |

## Verification

- `cargo test -p cb-frontend` green.
- `cargo test -p cb-driver` still green after AST printer adjustment.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Concrete fixture-driven checks:
  - `a + b + \\\n c` parses as one `BinOp(Add, BinOp(Add, a, b), c)`.
  - `If x Then : a = 1` emits `E0215` and does not panic; the program still has a valid `Stmt::Assign` for `a = 1` recovered as a sibling statement.
  - `Function foo() As Int : EndFunction` parses; the AST renders the return-type spelling as `Int` (issue #3).
  - `z As String = "asd"` parses without diagnostic.
  - `For i% = 0 To 10 : Next i%` parses without diagnostic; `For i% = 0 To 10 : Next j%` emits a sigil-mismatch diagnostic.
  - `Dim arr = New Integer[]` produces an `Expr::Error` at the `New` site (assert via AST snapshot).
  - `Field x : Print y` produces one stray-Field diagnostic and a valid `Print y` (not consumed).
  - `Redim arr As Integer[][10]` parses without diagnostic.
  - `Select x : Default : Default : End Select` emits `E0216`.
  - `Case 1, 2, 3` produces a `CaseArm` with three values (snapshot).
- Proptest property: parsing any `safe_source` containing inline `Continuation` tokens yields the same AST as the same source with the `Continuation`s removed.

## Related

- `docs/cb_syntax.md` §1.1 (line continuation), §1.5 (type keywords), §4.1 (implicit decl), §6.2 (Select), §6.3 (For/Next)
- `.claude/missing_tests.md` — "cb-frontend — Parser" section
- `docs/features/archive/FD-002_PARSER.md` — original parser FD; this is the follow-up correctness pass
- FD-003 (lexer correctness) — independent; can land in either order
- FD-005 (`Delete` statement) — depends on the keyword table and AST changes here only if FD-005 lands first; otherwise independent
