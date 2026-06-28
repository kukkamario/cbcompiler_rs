# FD-052: `Repeat … Until` Loop — Loop Until Condition Is Truthy

**Status:** Complete
**Completed:** 2026-06-28
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** Completes the loop family with the classic CoolBasic post-test loop. `Repeat … Until cond` runs the body, then loops back while `cond` is falsy and exits the moment it becomes truthy — the dual of the existing `Repeat … While`.

## Problem

The loop family currently expresses its *continue* condition only with the `While` keyword:

- `While cond … Wend` — pre-test, runs body while `cond` is truthy (`Stmt::While`)
- `Repeat … While cond` — post-test, runs body while `cond` is truthy (`Stmt::RepeatWhile`)
- `Repeat … Forever` — unconditional (`Stmt::RepeatForever`)

The most common real CoolBasic loop is missing: **`Repeat … Until cond`**, which loops *until* the condition becomes truthy (i.e. continues while it is falsy). This is the idiomatic game-loop form — e.g. `Repeat … Until KeyHit(cbKeyEnter)` appears throughout the cbEnchanted test corpus. Today it fails to parse because `Until` is not even a keyword.

## Solution

Add `Until` as a third closer for the `Repeat` block. It mirrors `Repeat … While` exactly; the only semantic difference is the back-edge polarity (loop when `cond` is **false**, exit when **true**). This is a **frontend + sema/lowering** change only — **no new IR instruction and no backend work**: lowering reuses the existing `Terminator::BranchIf` with the `then`/`else` targets swapped relative to `lower_repeat_while`, and both backends already handle `BranchIf`.

- **`cb-frontend`** — register `until` in the keyword map and add `Kw::Until`. In `parse_repeat`, add `Kw::Until` to the block closers alongside `Forever`/`While`, and add the arm that builds a new `Stmt::RepeatUntil { body, cond }` AST node (same shape as `RepeatWhile`). Extend `ast_print` and the parser's closer-recovery/keyword-name tables (the same spots `RepeatWhile` touches). Add `Until` to the reserved-words list in `docs/cb_syntax.md` §3.
- **`cb-sema`** — add the `Stmt::RepeatUntil` arm everywhere `RepeatWhile` is matched: the body-scan passthroughs and label-collection in `lower.rs`/`check.rs`, the condition-check + `ControlKind::Loop` push in `check.rs`, and the lowering dispatch in `lower.rs`.
- **Lowering** — add `lower_repeat_until`, a copy of `lower_repeat_while` with the condition block's `BranchIf` arms swapped (`then_block: exit_block`, `else_block: body_block`).

`Continue` jumps to the condition check and `Break` exits, identical to `Repeat … While` (handled for free by `lower_loop_body`).

### Scope decision

CoolBasic has no *pre-test* `Until` form — the pre-test loop is always `While … Wend`. Scope is intentionally **only the post-test `Repeat … Until`**; no pre-test `Until` variant (confirmed).

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-frontend/src/keywords.rs` | MODIFY | Map `"until"` → `Kw::Until` |
| `crates/cb-frontend/src/token.rs` | MODIFY | Add `Kw::Until` variant + its `"until"` display string |
| `crates/cb-frontend/src/ast.rs` | MODIFY | Add `Stmt::RepeatUntil { body, cond }` |
| `crates/cb-frontend/src/parser.rs` | MODIFY | `parse_repeat`: add `Until` closer + arm; closer-recovery / keyword-name tables |
| `crates/cb-frontend/src/ast_print.rs` | MODIFY | Print `RepeatUntil` (mirror `RepeatWhile`) |
| `crates/cb-sema/src/check.rs` | MODIFY | `RepeatUntil` arms: passthrough, label collection, condition check + loop scope |
| `crates/cb-sema/src/lower.rs` | MODIFY | Passthrough arms + `lower_repeat_until` (swapped `BranchIf`) |
| `docs/cb_syntax.md` | MODIFY | Add `Until` to reserved words; document `Repeat … Until` under §6.3 Loops |
| `crates/cb-frontend/tests/fixtures/parser_loops.cb` | MODIFY | Add a `Repeat … Until` case |
| `crates/cb-frontend/tests/parser_snapshots.rs` (+ `.snap`) | MODIFY | Snapshot the new node |

## Verification

- `cargo test -p cb-frontend` — parser snapshot/unit tests for the new closer, including mismatched-closer recovery (E0204) parity with `Repeat … While`.
- `cargo test -p cb-sema` — lowering snapshot showing `Repeat … Until` produces the same CFG as `Repeat … While` with inverted condition arms; `Break`/`Continue` targets unchanged.
- End-to-end interp run: `Repeat … Until cond` executes the body at least once and stops the iteration after `cond` first becomes truthy.
- If the LLVM differential suite covers loops, add a `Repeat … Until` fixture to confirm interp ≡ LLVM.

## Related

- `docs/cb_syntax.md` §6.3 (Loops) — existing `Repeat … While` / `Repeat … Forever` / `While … Wend`
- Mirrors `Stmt::RepeatWhile` lowering (`lower_repeat_while`) — the implementation template
- Real-world usage: `cbEnchanted/tests/*.cb` (e.g. `rotateimage.cb`: `Repeat … Until KeyHit(cbKeyEnter)`)
