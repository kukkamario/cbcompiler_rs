# FD-005: `Delete` Statement

**Status:** Pending Verification
**Priority:** Medium
**Effort:** Medium (1-4 hours for frontend; runtime semantics deferred)
**Impact:** Adds first-class language surface for `Delete` (`cb_syntax.md` §3.3). Today the keyword does not exist in the lexer, the parser silently misparses `Delete x` as a paren-less subroutine call, and the documented lvalue-rewind / sentinel / double-delete-trap semantics are unreachable. This FD lands the lexer + parser + AST work so the construct is at least syntactically recognized; runtime semantics are scoped out to the eventual interpreter FD.

## Problem

`cb_syntax.md` §3.3 spends ~80 lines specifying `Delete`. Today nothing implements it:

- `Kw::Delete` is not in `crates/cb-frontend/src/token.rs`.
- The keyword table at `crates/cb-frontend/src/keywords.rs` has no entry.
- `parse_stmt_inner` (`parser.rs:918`) has no `Delete` arm.
- The lexer emits `Ident("Delete") Ident("x")` for `Delete x`, and the parser interprets it as a paren-less call to a subroutine named `Delete` with argument `x` — a silent and wrong parse.

The spec defines real, observable, runtime-relevant semantics that need a syntactic anchor before any of them can be implemented:

1. **Lvalue vs. rvalue distinction.** `Delete v` (variable, field, or array element) rewinds the variable to the previous live node and marks it "deleted". `Delete e` (rvalue) frees the node but does not rewind anything.
2. **Deleted state.** Set on the variable slot by `Delete v`; cleared by any subsequent assignment.
3. **Field access through a deleted variable traps** (§9.2).
4. **Double-delete traps.**
5. **`Delete` on `Null` traps.**
6. **`Next`/`Previous` on a deleted variable are transparent** — they walk from the underlying pointer (now the previous node or the sentinel).
7. **Aliasing.** Only the named variable is rewound and marked; other variables holding the same reference dangle.

All seven items are runtime/semantic concerns and are scoped to sema + the future interpreter FD. The frontend work in this FD only needs a *syntactic anchor*: a `Stmt::Delete` variant in the AST so the construct stops being silently misparsed as a paren-less call. Lvalue classification (item 1) is **not** done in the parser — it follows the existing pattern (`Stmt::Assign` stores any expression as `target`; sema validates the shape).

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
        operand: NodeId,
    },
}
```

The variant carries only the operand `NodeId`. The statement span is stored in the arena's parallel `spans` table (the established convention; no `Stmt` variant embeds its own span). Lvalue/rvalue classification is **not** done here — sema owns it (this matches `Stmt::Assign`, which is permissive on `target` shape per the comment at `parser.rs:1139`).

### Parser

In `parse_stmt_inner`:

```rust
TokenKind::Keyword(Kw::Delete) => self.parse_delete(),
```

`parse_delete`:

1. Consume `Delete` keyword (record its span as `start`).
2. Parse an expression with `parse_expr_bp(0)` (same as `Return`, `Goto`, `Include` — there is no `EXPR_BP_LOW` constant).
3. Build `Stmt::Delete { operand }`. Allocate with span `start.merge(arena.span_of(operand))`.
4. Call `consume_stmt_sep_or_terminator()`.

No new error codes required; the existing expression-parser diagnostics cover all malformed-operand cases (e.g. `Delete` followed by `Newline` falls into `parse_expr_bp`'s "expected expression" path).

### Driver

`cb-driver/src/main.rs` AST printer gets one new arm in `children_of` (push the operand) and one in `stmt_variant_name` (return `"Delete"`). The catch-all `_ => {}` in `children_of` today silently skips new variants — this exact FD demonstrates why FD-006 wants to replace them with explicit arms.

### Out of scope (deferred to sema / interpreter FD)

- **Lvalue vs. rvalue classification** of the operand. Sema decides whether the operand is a rewindable lvalue (`Ident` / `Field` / `Index` chain — and the policy call on `Paren { inner: <lvalue> }`) or an rvalue (anything else). This is the same boundary as `Stmt::Assign`'s target validation.
- Runtime sentinel for the linked-list rewind.
- "Deleted state" tracking on variable slots.
- The trap conditions (§9.2 items related to `Delete` — Null, double-delete, deleted-field-access).
- IR representation of the rewind operation. Sema/IR FD will pick this up; it likely wants two distinct IR ops: `IrDelete::Rewind(var_slot, value)` and `IrDelete::Free(value)`.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-frontend/src/token.rs` | MODIFY | Add `Kw::Delete` variant; `kw.as_str()` returns `"Delete"`. |
| `crates/cb-frontend/src/keywords.rs` | MODIFY | Add `"delete" => Kw::Delete`. `LONGEST_KEYWORD_LEN` is unaffected (`"delete"` is 6 chars, well under the existing 11); the `longest_keyword_len_matches_table` test will continue to hold without changes. |
| `crates/cb-frontend/src/ast.rs` | MODIFY | Add `Stmt::Delete { operand: NodeId }`. No span field — spans live in the arena's parallel `spans` table. |
| `crates/cb-frontend/src/parser.rs` | MODIFY | Add `parse_delete`; dispatch from `parse_stmt_inner`. No new helpers — lvalue classification is sema's job. |
| `crates/cb-frontend/tests/lexer_units.rs` | MODIFY | Assert `delete`, `Delete`, `DELETE` lex as `Kw::Delete`. |
| `crates/cb-frontend/tests/parser_snapshots.rs` + fixtures | MODIFY | New fixture `delete_statement.cb` exercising lvalue, field, indexed, and rvalue operands. |
| `crates/cb-driver/src/main.rs` | MODIFY | AST printer arm for `Stmt::Delete`. |
| `docs/cb_syntax.md` | LEAVE | Already specifies the syntax; no change. |

## Verification

- `cargo test -p cb-frontend` green; new snapshot fixture `delete_statement.cb` covers:
  - `Delete x` → `Stmt::Delete { operand: Expr::Ident }`.
  - `Delete y.field` → `Stmt::Delete { operand: Expr::Field }`.
  - `Delete arr[0]` → `Stmt::Delete { operand: Expr::Index }`.
  - `Delete First(MyType)` → `Stmt::Delete { operand: Expr::Call }` (sema will later flag this as rvalue per §3.3).
  - `If n.dead Then Delete n` → `Delete` inside single-line `If` body (the canonical loop-cleanup pattern from §3.3).
  - `Delete` with no operand → diagnostic, statement recovers.
  - `Delete : Print 1` → diagnostic on missing operand; the following `Print 1` recovers as a paren-less call (in this language `Print` is not a keyword — it's a user-defined sub).
- New lexer unit asserts `delete`, `Delete`, `DELETE` all lex as `Kw::Delete`.
- `cargo test -p cb-driver` green after printer update.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Spec smoke: every spec example in `cb_syntax.md` §3.3 worked-examples block parses without diagnostic. (Runtime semantics not asserted — interpreter FD owns those.)
- Regression check: `Delete x` no longer parses as a paren-less subroutine call. The snapshot for `Delete x` makes this visible (top-level node is `Stmt::Delete`, not `Stmt::ExprStmt { Expr::Call }`).

## Related

- `docs/cb_syntax.md` §3.3 — `Delete` semantics (the spec this implements)
- `docs/cb_syntax.md` §9.2 — runtime traps (out of scope for this FD)
- FD-004 (parser correctness, completed 2026-05-23) — established the `parse_stmt_inner` dispatch shape and the `Stmt::Error` / forced-progress recovery this FD's `parse_delete` plugs into.
- Future interpreter FD — owns the runtime semantics (deleted-state tracking, traps, rewind sentinel)
