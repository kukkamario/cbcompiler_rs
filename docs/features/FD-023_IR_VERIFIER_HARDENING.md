# FD-023: IR Verifier Hardening

**Status:** Open
**Priority:** Medium
**Effort:** Low-Medium (1-3 hours)
**Impact:** Makes `cb-ir`'s structural verifier actually guard the CFG invariants both backends depend on, and backfills the missing in-crate tests for the verifier and the IR text printer.

## Problem

The post-FD-018 review found the IR itself clean and backend-agnostic, but the verifier — whose entire job is catching malformed IR before a backend trips over it — is shallow, and the 11-test count overstates direct coverage (the printer has zero in-crate tests).

1. **No entry-block / empty-blocks validation.** `verify()` iterates `func.blocks` (`verify.rs:17`) but never asserts a function has at least one block or a designated entry. A `Function` with `blocks: vec![]` passes — yet `cb-backend-interp` initializes `current_block` to `BlockId(0)` (`interp.rs:235`), so such a function fails at runtime instead of at verification.
2. **Duplicate `BlockId` silently accepted.** `block_ids` is built via `.map(|b| b.id).collect()` into a `HashSet` (`verify.rs:19`), which silently dedups; two blocks sharing an id pass, and terminator-target checks (`:193-199`) only test membership, so the ambiguity is invisible. `BlockId` uniqueness is a core CFG invariant nothing else guards.
3. **No register dominance; correctness depends on block vector order.** `defined_regs` is a single set accumulated in `func.blocks` order (`verify.rs:20`). A use is accepted if *any* earlier-listed block defined the reg, regardless of whether it dominates (or reaches) the use; conversely valid IR with a definition in a later-listed CFG predecessor would be wrongly rejected. The use-before-def check is thus both unsound (no dominance) and order-fragile.
4. **Result reg inserted before operands are checked.** For each inst the result reg is inserted into `defined_regs` (`verify.rs:30-32`) *before* `verify_inst_regs` runs (`:34`), so a malformed instruction referencing its own result would pass the use-before-def check.
5. **`FuncKind::UserDefined.body_index` is never range-checked** (`lib.rs:42`); an out-of-range or duplicated `body_index` only misbehaves at backend time.

Coverage gaps in the same crate:

- **`print.rs` (379 LOC, the `--dump-ir` surface) has zero direct tests** (`print.rs:88-379`); `format_type`/`format_binop`/`format_unop` and most `print_inst_kind` arms (`GetField`, `SetElement`, `Redim`, `Convert`, all `TrapKind` labels, `FnPtr`/`Array`/`RuntimeType`) are covered only incidentally via `cb-sema` snapshots.
- **`ConstLong`/`ConstFloat`/`ConstString`/`ConstNull`, all string/comparison binops, `IntDiv`/`Mod`/`Pow`/`Shl`/`Shr`/`Sar`, and `Terminator::Halt`/`Trap`** never appear in any `cb-ir` test (`inst.rs:53`).
- **Runtime catalog types lack `Debug`** (`FuncDesc`, `FuncParamDesc`, `RuntimeTypeDesc`, `RuntimeCatalog`, `lib.rs:57-83`) while the rest of the crate derives `Clone+Debug` — harder to inspect in an observability-focused project (the `unsafe extern "C" fn()` field blocks a naive `Clone` but fn pointers *do* implement `Debug`).

## Solution

In `cb-ir`:

- Add to `verify()`: assert `!func.blocks.is_empty()` and that the entry convention (`blocks[0]` / `BlockId(0)`) is present; detect duplicate `BlockId`s (compare set len to `blocks.len()`, or check `insert` returned `true`); validate `body_index < program.functions.len()` for every `UserDefined` entry (and ideally that the mapping is a bijection); check operand regs *before* inserting the result reg.
- For dominance: either **document and assert** that blocks must be in a dominance-respecting order, or compute reachability/dominance and validate uses against dominating defs. At minimum add a back-edge/loop test pinning the intended contract. (Document-and-assert is the cheaper first step given the interpreter's current well-ordered output.)
- Add `#[derive(Debug)]` (and `Clone` where feasible) to the catalog types, or document why they intentionally omit them.
- Add focused `insta` snapshot tests (insta is already a dev-dependency) that build small `Program`s by hand and assert `print_program` output across each `InstKind`, `Terminator`, and `IrType` variant; extend verifier tests with `should_panic`/`Err` cases for empty-blocks, duplicate `BlockId`, and out-of-range `body_index`, plus accept-cases for `Halt`/`Trap` and a string/conversion instruction.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-ir/src/verify.rs` | MODIFY | Entry/empty-block, duplicate-`BlockId`, `body_index`-range, operand-before-result checks; dominance contract (assert or compute) |
| `crates/cb-ir/src/lib.rs` | MODIFY | `Debug`/`Clone` derives on catalog types |
| `crates/cb-ir/tests/` (new) or `#[cfg(test)]` | CREATE | `insta` snapshots for `print_program` across all variants; new verifier failure/accept tests |

## Verification

- `cargo test -p cb-ir` green, with new tests:
  - `should_panic`/`Err` for a zero-block function, duplicate `BlockId`, and out-of-range `body_index`.
  - Accept-cases touching `Halt`, `Trap`, a `Const*` of each kind, and a string/comparison binop.
  - `print_program` snapshots covering every `InstKind`/`Terminator`/`IrType`.
- `cargo test --workspace` + `clippy -- -D warnings` green; downstream `cb-sema` snapshot tests unaffected.

## Related

- Surfaced by the post-FD-018 codebase review (IR area).
- [FD-008](archive/FD-008_IR.md) — the IR types, lowering, printer, and the original (debug-build) verifier this hardens.
- [FD-010](archive/FD-010_INTERPRETER_BACKEND.md) — `current_block = BlockId(0)` startup assumption the entry-block check protects.
