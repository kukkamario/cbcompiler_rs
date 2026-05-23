# FD-005: `Delete` Statement

**Status:** Open
**Priority:** Medium
**Effort:** Medium (1-4 hours for frontend; runtime semantics deferred)
**Impact:** Adds first-class language surface for `Delete` (`cb_syntax.md` §3.3). Today the keyword does not exist in the lexer, the parser silently misparses `Delete x` as a paren-less subroutine call, and the documented lvalue-rewind / sentinel / double-delete-trap semantics are unreachable. This FD lands the lexer + parser + AST work so the construct is at least syntactically recognized; runtime semantics are scoped out to the eventual interpreter FD.

## Problem

`cb_syntax.md` §3.3 spends ~80 lines specifying `Delete`. Today nothing implements it:

- `Kw::Delete` is not in `crates/cb-frontend/src/token.rs`.
- The keyword table at `crates/cb-frontend/src/keywords.rs` has no entry.
- `parse_stmt_inner` (`parser.rs:849-919`) has no `Delete` arm.
- The lexer emits `Ident("Delete") Ident("x")` for `Delete x`, and the parser interprets it as a paren-less call to a subroutine named `Delete` with argument `x` — a silent and wrong parse.

The spec defines real, observable, runtime-relevant semantics that need a syntactic anchor before any of them can be implemented:

1. **Lvalue vs. rvalue distinction.** `Delete v` (variable, field, or array element) rewinds the variable to the previous live node and marks it "deleted". `Delete e` (rvalue) frees the node but does not rewind anything.
2. **Deleted state.** Set on the variable slot by `Delete v`; cleared by any subsequent assignment.
3. **Field access through a deleted variable traps** (§9.2).
4. **Double-delete traps.**
5. **`Delete` on `Null` traps.**
6. **`Next`/`Previous` on a deleted variable are transparent** — they walk from the underlying pointer (now the previous node or the sentinel).
7. **Aliasing.** Only the named variable is rewound and marked; other variables holding the same reference dangle.

Items 1-2 are AST-level (the parser needs to know whether the operand is an lvalue or rvalue so the IR/interpreter can implement the rewind). Items 3-7 are runtime semantics, scoped to the future interpreter FD.

## Solution

Three pieces, all in `crates/cb-frontend`:

### Lexer

- Add `Kw::Delete` to the `Kw` enum in `token.rs`.
- Add `"delete" => Kw::Delete` to the keyword table in `keywords.rs`.
- No new error codes.

### AST

Add a new statement variant:

```rust
enum Stmt {
    // …existing…
    Delete {
        operand: ExprId,
        operand_is_lvalue: bool,  // resolved at parse time; sema may refine
        span: Span,
    },
}
```

`operand_is_lvalue` is computed by the parser using the same lvalue-shape check used in `parse_expr_or_assign_stmt` (a chain of `Ident` / `Expr::Field` / `Expr::Index` — anything else is an rvalue). Sema may later refine (e.g. an indexed Type-field is still an lvalue), but the parser's first-pass classification is enough for the AST.

### Parser

In `parse_stmt_inner`:

```rust
TokenKind::Keyword(Kw::Delete) => self.parse_delete(),
```

`parse_delete`:

1. Consume `Delete` keyword.
2. Parse an expression at `EXPR_BP_LOW` (same as `Return v`, `Print x`).
3. Compute `operand_is_lvalue` by walking the parsed expression.
4. Build `Stmt::Delete` with span over `[delete_kw.start, operand.span.end]`.
5. Consume statement separator (`Newline` / `Colon` / `Eof`).

No new error codes required; the existing expression-parser diagnostics cover all malformed-operand cases.

### Driver

`cb-driver/src/main.rs` AST printer gets one new arm in `children_of` and `print_stmt`. The catch-all `_ => {}` arms today silently skip new variants — this exact FD demonstrates why FD-006 wants to replace them with explicit arms.

### Out of scope (deferred to interpreter FD)

- Runtime sentinel for the linked-list rewind.
- "Deleted state" tracking on variable slots.
- The five trap conditions (§9.2 items related to `Delete`).
- IR representation of the rewind operation. Sema/IR FD will pick this up; it likely wants two distinct IR ops: `IrDelete::Rewind(var_slot, value)` and `IrDelete::Free(value)`.
- Diagnostics around lvalue/rvalue distinction. Today the parser records the classification; a sema pass can later upgrade an rvalue-form misuse into a warning if appropriate.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-frontend/src/token.rs` | MODIFY | Add `Kw::Delete` variant; `kw.as_str()` returns `"Delete"`. |
| `crates/cb-frontend/src/keywords.rs` | MODIFY | Add `"delete" => Kw::Delete`. Update `LONGEST_KEYWORD_LEN` if affected (`"delete"` is 6 chars, under the existing 11 — no change needed, but verify FD-003's const-fn replacement still derives correctly). |
| `crates/cb-frontend/src/ast.rs` | MODIFY | Add `Stmt::Delete { operand: ExprId, operand_is_lvalue: bool, span: Span }`. |
| `crates/cb-frontend/src/parser.rs` | MODIFY | Add `parse_delete`; dispatch from `parse_stmt_inner`. New helper `is_lvalue_shape(&self, expr: &Expr) -> bool`. |
| `crates/cb-frontend/tests/lexer_units.rs` | MODIFY | Assert `delete`, `Delete`, `DELETE` lex as `Kw::Delete`. |
| `crates/cb-frontend/tests/parser_snapshots.rs` + fixtures | MODIFY | New fixture `delete_statement.cb` exercising lvalue, field, indexed, and rvalue operands. |
| `crates/cb-driver/src/main.rs` | MODIFY | AST printer arm for `Stmt::Delete`. |
| `docs/cb_syntax.md` | LEAVE | Already specifies the syntax; no change. |

## Verification

- `cargo test -p cb-frontend` green; new fixture covers:
  - `Delete x` → `Stmt::Delete { operand_is_lvalue: true }`.
  - `Delete y.field` → lvalue.
  - `Delete arr[0]` → lvalue.
  - `Delete First(MyType)` → `operand_is_lvalue: false`.
  - `Delete` with no operand → diagnostic.
  - `Delete : Print 1` → diagnostic (missing operand), then valid `Print 1`.
- `cargo test -p cb-driver` green after printer update.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Spec smoke: every spec example in `cb_syntax.md` §3.3 lines 385-435 parses without diagnostic. (Runtime semantics not asserted — interpreter FD owns those.)
- Regression check: `Delete x` no longer parses as a paren-less subroutine call. Add a negative-form test that asserts the AST shape changed.

## Related

- `docs/cb_syntax.md` §3.3 — `Delete` semantics (the spec this implements)
- `docs/cb_syntax.md` §9.2 — runtime traps (out of scope for this FD)
- FD-004 (parser correctness) — independent; can land in either order. Both touch `parse_stmt_inner`; minor merge conflict expected.
- Future interpreter FD — owns the runtime semantics (deleted-state tracking, traps, rewind sentinel)
