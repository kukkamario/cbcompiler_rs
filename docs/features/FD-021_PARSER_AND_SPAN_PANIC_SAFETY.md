# FD-021: Parser & Span Panic-Safety

**Status:** Pending Verification
**Priority:** High
**Effort:** Low-Medium (1-3 hours)
**Impact:** Restores the frontend's "never abort on untrusted input" contract. Today a single small input file can crash the compiler via unbounded recursion; two latent panic vectors in span-slicing share the same risk.

## Problem

The lexer and parser are deliberately built to *never abort* on malformed input — errors become `Error` tokens plus structured diagnostics. The post-FD-018 review found one place this contract is broken outright and two latent panic vectors that violate the codebase's own defensive convention.

1. **Unbounded parser recursion overflows the stack and aborts the process.** `parse_expr_bp` (`parser.rs:405`), `parse_primary` (parens/`New`, `:502-511`), `parse_type_expr`/`parse_type_atom` (`:685`/`:693`), and `parse_postfix` (`.field` chains, `:540`/`:611`) each recurse once per nesting level with no depth cap. **Confirmed empirically** against the built `cb` binary: `x = ((((…1…))))` at depth 2000 parses (exit 0), at depth ~6000 aborts with `fatal runtime error: stack overflow` (exit 134). A single small file crashes the compiler. The `parser_props.rs` `no_panic_on_arbitrary_utf8` proptest uses `any::<String>()`, which can't realistically generate thousands of balanced delimiters, so the case is untested. `ast_print::print_node` (`ast_print.rs:24`, reachable via `--dump-ast`, `main.rs:182-187`) has the same unbounded recursion.

2. **`SpanExt::slice` uses panic-prone raw string indexing.** `span.rs:25` does `&source[start..end]`, which panics on out-of-range or non-char-boundary offsets. Every *other* span-slicing site is defensive: `parser.rs:311` (`span_slice`) and `string_value.rs:33` (`slice`) bounds-check and return `""`. `SpanExt::slice` is consumed by `cb-sema` (`lower.rs:238/246`, `types.rs:110`) on parser-produced spans — currently safe because name-spans land on char boundaries, but a latent panic vector inconsistent with the crate's own convention.

3. **`offset_to_line_char_col` can panic on a non-char-boundary offset.** In `cb-diagnostics`, `offset_to_line_byte_col` clamps to `text_len` but does **not** snap to a char boundary (`source.rs:235`), so `byte_col` may point mid-codepoint; `self.text[slice_start..slice_end]` (`source.rs:62`) then panics (`byte index N is not a char boundary`). The byte-col path's doc promises "never panics" but the char-col wrapper built on it has no such guard.

Folded in (same theme):

- **The Pratt loop has three hard panics keyed to hand-maintained table invariants:** `unop_from(...).expect(...)` (`parser.rs:409`), the analogous `binop_from` expect (`:431`), and `unreachable!("parse_postfix called on non-postfix token")` (`:611`). Safe by construction today, but the parser deliberately demoted similar invariant-violation sites to the `E0299` internal-error diagnostic path (`:1025`, `:1483`); these three are the remaining hard crashes on a future table drift.

## Solution

- **Recursion guard (`cb-frontend`):** add a depth counter on `Parser`, incremented in `parse_expr_bp`/`parse_type_expr`/`parse_postfix` and decremented on return. On exceeding a generous limit (~256), emit a new `E02xx` "expression/type nesting too deep" diagnostic and return `Expr::Error`/`TypeExpr::Error` for recovery. Mirror the guard in `ast_print::print_node` (depth-limit with an elision marker) since it is reachable on untrusted ASTs via `--dump-ast`.
- **`SpanExt::slice`:** make it bounds-and-boundary-checked (return `""` on bad input, or `Option`), matching `span_slice`/`string_value::slice`; or document the precondition and `debug_assert` it. Prefer the defensive form for consistency.
- **`offset_to_line_char_col`:** floor `slice_end` to the nearest char boundary (`str::floor_char_boundary` or a manual check) before slicing, so a bad `u32` clamps instead of panicking.
- **Pratt panics:** either fold the `bp` lookup and op mapping into a single function returning `(bp, op)` so the tables cannot drift, or demote the three to the existing `E_INTERNAL_PARSER` (`E0299`) diagnostic path.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-frontend/src/parser.rs` | MODIFY | Recursion-depth counter + "nesting too deep" diagnostic; demote/eliminate the three Pratt panics |
| `crates/cb-frontend/src/ast_print.rs` | MODIFY | Depth-limited `print_node` with elision marker |
| `crates/cb-frontend/src/span.rs` | MODIFY | Bounds/boundary-safe `SpanExt::slice` |
| `crates/cb-diagnostics/src/source.rs` | MODIFY | Char-boundary floor before the char-col slice |
| `crates/cb-frontend/tests/*` | CREATE/MODIFY | Regression: few-thousand-deep input yields a diagnostic (not abort); `--dump-ast` on it is safe |
| `crates/cb-diagnostics/tests/line_index.rs` | MODIFY | Test passing a mid-codepoint offset to `offset_to_line_char_col` |

## Verification

- New `cb-frontend` test: a 5000-deep `((((…))))` source produces the "nesting too deep" diagnostic and exits cleanly (no `exit 134`); same input under `--dump-ast` does not abort.
- New `cb-diagnostics` test: a mid-codepoint offset returns a clamped position rather than panicking.
- `cargo test --workspace` + `clippy -- -D warnings` green; existing 266 frontend tests unaffected.
- Manual: `printf 'x = %.0s(' {1..6000} > deep.cb; cargo run -p cb-driver -- deep.cb` exits with a diagnostic, not a stack-overflow abort.

## Implementation Notes

Implemented on branch `fd-021-parser-span-panic-safety`.

- **Recursion guard.** Added a shared `recursion_depth: u32` counter on `Parser` with `MAX_RECURSION_DEPTH = 256`, guarded in the two true recursion gateways: `parse_expr_bp` and `parse_type_atom` (each split into a thin guard wrapper + `_inner` so inc/dec is balanced on *every* path, including `?` early returns). Field/`\` chains and left-assoc operator chains are iterative (handled in the Pratt `loop`), so they need no guard. The counter is reset to 0 at each statement boundary in `parse_stmt`, so a `?`-aborted sub-parse can't inflate a later statement's depth. New diagnostic **`E0218` (`E_NESTING_TOO_DEEP`)**; the guard returns `Expr::Error` / `TypeExpr::Error` for recovery.
- **Pratt panics — folded (chosen over demote-to-E0299).** `infix_bp`+`binop_from` → `infix_op` returning `(u8, u8, BinOp)`; `prefix_bp`+`unop_from` → `prefix_op` returning `(u8, UnOp)`; `postfix_bp` → `postfix_op` returning `(u8, PostfixKind)`. `parse_postfix` now takes a `PostfixKind` and matches it exhaustively, eliminating the `unreachable!`. The two `.expect()`s are gone — the tables can no longer drift.
- **`SpanExt::slice`** now bounds- **and** char-boundary-checks, returning `""` on bad input (stronger than the existing `span_slice`/`string_value::slice`, which only bounds-check).
- **`offset_to_line_char_col`** floors `slice_end` down to the nearest char boundary before slicing.
- **`ast_print::print_node`** gains `MAX_PRINT_DEPTH = 512` (above the parser cap so all real ASTs print fully) with an elision marker.
- **Tests:** new `crates/cb-frontend/tests/deep_nesting.rs` (12 cases: deep parens/prefix/calls/types yield E0218 not abort; `--dump-ast` safe; depth resets between statements; moderate nesting unaffected; `SpanExt::slice` defensive cases). New `offset_to_line_char_col_mid_codepoint_does_not_panic` in `cb-diagnostics`. Full workspace `cargo test` + `clippy -D warnings` green; manual `deep.cb` repro now exits 1 with a diagnostic (was 134 / stack-overflow abort).

## Related

- Surfaced by the post-FD-018 codebase review (frontend + diagnostics areas).
- [FD-003](archive/FD-003_LEXER_CORRECTNESS.md) / [FD-004](archive/FD-004_PARSER_CORRECTNESS.md) — prior panic-reachability and `E0299` internal-error-promotion work this continues.
- [FD-001](archive/FD-001_LEXER.md) — the "never abort on untrusted input" contract.
