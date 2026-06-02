# FD-020: Sema Numeric & For-Loop Semantics

**Status:** Open
**Priority:** High
**Effort:** Medium (1-4 hours)
**Impact:** Fixes type-coercion correctness in `cb-sema` for `For` loops (currently miscompiles Float/mixed-type loops) and tightens three documented numeric rules the checker does not enforce. All are in untested paths.

## Problem

The post-FD-018 review found `cb-sema` is well-structured with good error-code coverage, but the trickier numeric/control-flow semantics are both under-tested and partly wrong. The lowering snapshot suite only covers happy-path `For` loops with default (positive, integer) steps.

1. **`check_for` never coerces `from`/`to`/`step` to the loop-variable type.** `check.rs:1504-1529` only verifies the three are numeric (`is_numeric`); it never calls `coerce()`, so no `ConversionTable` entries are recorded and no narrowing warning (E0318) fires. `For i% = 1 To 10.5` leaves `to` typed `Float`, violating the §3.4 implicit-conversion rules. Compare `check_assign` (`:1419`) and `check_return` (`:1681`), which both coerce.

2. **`lower_for` emits the direction-test constant in the wrong type.** `lower.rs:1682` computes `step_positive` as `step_val > ConstInt(0)` — the `0` is always emitted as `ConstInt` regardless of loop-variable type, and the step temp (`:1661`) holds the *unconverted* step register. For a `Float` step (`For x# = 10.0 To 0.0 Step -0.5`) this compares Float against Int; the ascending/descending compares (`LtEq`/`GtEq` at `:1701`/`:1719`) likewise assume `var`/`to` share a type. Combined with #1, mixed/Float `For` loops produce IR with mismatched operand types. Entirely untested — no `Step`, descending, or Float-loop test exists.

3. **Integer-literal overflow against the declared type is not enforced.** `cb_syntax.md:125` (§3.4) states an integer literal that overflows its inferred type is a compile error. `check_expr` types every `IntLit` as `Type::Int` (`check.rs:608`) and assignment only checks convertibility, so `Dim b As Byte : b = 300` is not the error the spec requires. (Note: a narrowing *warning* E0318 *does* fire via `coerce(Int→Byte)`; the missing piece is the hard overflow *error*.)

4. **Signed/unsigned numeric promotion is order-dependent.** `numeric_promote` ranks `Int`/`UInt` equally (3) and `Long`/`ULong` equally (4), returning the lhs on a rank tie (`rank(a) >= rank(b)`, `types.rs:204`/`:209`). So `Int + UInt` → `Int` but `UInt + Int` → `UInt`: result signedness depends purely on operand order. `cb_syntax.md` §3.4 defines same-width signed↔unsigned only as a bit reinterpret, not the arithmetic result type — this needs a deliberate rule, not accidental lhs-wins.

Lower-severity items folded in:

- **`Pow` (`^`) has no const folding and an unclear result type.** `eval_const_binary` (`check.rs:1768`) handles `Add`/`Sub`/`Mul`/`Div`/`Mod` but not `Pow`, so `Const x = 2 ^ 10` keeps its placeholder `0`; `binary_result_type` treats `Pow` like `Mul` (so `Int ^ Int` → `Int`) even though exponentiation generally wants Float semantics. The doc pins neither.
- **`E0322` const division-by-zero has no test, and const *float* div-by-zero is silently allowed** (`check.rs:1369`), returning the IEEE result. (Integer div-by-zero now comes from `/` between integers or `Mod` — there is no `\` integer-division operator; FD-028 made `\` the `Type` field accessor.)

## Solution

In `cb-sema`:

- **`check_for`:** after typing `from`/`to`/`step`, `self.coerce(...)` each to `var_ty` so conversions are recorded and narrowing warnings fire — matching `check_assign`.
- **`lower_for`:** emit the zero/one direction-test constants in the loop variable's IR type (or insert explicit `Convert`), and ensure both compare operands share a type. With #1 recording the conversions, the lowered registers will already be the right type.
- **Literal overflow:** when a literal is assigned/coerced to a narrower integer type, validate the value fits the target and emit a diagnostic (new E03xx) when it does not.
- **Signed/unsigned promotion:** decide and document a deterministic rule for mixed same-width arithmetic (e.g. prefer unsigned, or require explicit conversion), update `numeric_promote`, and amend `cb_syntax.md` §3.4.
- **`Pow`:** add `Pow` to const folding (producing `Float`) and pin/document the static result type of `^` so checker and interpreter agree.
- **Const div-by-zero:** route folding through one checked helper; decide whether const float div-by-zero warns; add the missing `E0322` test.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-sema/src/check.rs` | MODIFY | Coerce `for` bounds to var type; literal-range check; `Pow` const fold + result type; `E0322`/float-div handling |
| `crates/cb-sema/src/lower.rs` | MODIFY | Direction-test constants/compares in the loop-variable type |
| `crates/cb-sema/src/types.rs` | MODIFY | Deterministic signed/unsigned same-width promotion rule |
| `crates/cb-sema/src/convert.rs` | MODIFY (if needed) | Literal-fits-target check support |
| `crates/cb-sema/tests/lower_snapshots.rs` | MODIFY | Snapshots for descending `For`, `Step`, and Float/mixed `For` loops |
| `docs/cb_syntax.md` | MODIFY | Document the chosen mixed signed/unsigned arithmetic rule and `^` result type |

## Verification

- `cargo test -p cb-sema` green, with new tests:
  - Snapshot of `For x# = 10.0 To 0.0 Step -0.5` and a descending integer loop — IR operands type-consistent.
  - `For i% = 1 To 10.5` records a Float→Int narrowing and warns.
  - Over-range literal to a narrow type (`Dim b As Byte : b = 300`) is the documented error.
  - `Int + UInt` and `UInt + Int` (both orders) produce the documented result type.
  - `Const x = 2 ^ 10` folds; `Const x = 1 / 0` (integer div-by-zero) emits `E0322`.
- `cargo test --workspace` + `clippy -- -D warnings` green.
- Cross-check the lowered Float-`For` IR by running it on the interpreter (interp is the reference per CLAUDE.md).

## Related

- Surfaced by the post-FD-018 codebase review (sema area).
- [FD-007](archive/FD-007_Semantic_Analysis.md) — two-pass checker, conversion tracking, narrowing warnings, diagnostic codes.
- [FD-008](archive/FD-008_IR.md) — `For` direction-aware lowering this corrects.
- `docs/cb_syntax.md` §3.4 (implicit conversions, literal overflow, signed/unsigned).
