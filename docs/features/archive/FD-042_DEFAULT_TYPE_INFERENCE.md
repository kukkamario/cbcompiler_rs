# FD-042: Default Type Inference for Implicit Declarations

**Status:** Complete
**Completed:** 2026-06-23
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** `obj = LoadObject("...")` implicitly declares `obj` as `Object` (inferred from the value) instead of defaulting to `Integer`. Removes the need to write `Dim obj As Object` / `obj As Object` before every reference-typed local.

## Problem

When a variable is **implicitly declared** at its first assignment with neither a sigil nor an `As` clause, it currently defaults to `Integer`:

```cb
x = 213          // x is Integer  (the desired default — value is an Int)
obj = LoadObject("hero.png")   // obj is *Integer* today → wrong / error
```

`obj = LoadObject(...)` is one of the most common idioms in CoolBasic game code, yet today it forces the user to write an explicit type:

```cb
Dim obj As Object
obj = LoadObject("hero.png")
```

The desired behavior: **infer the implicit variable's type from the value being assigned**. So `obj = LoadObject(...)` declares `obj As Object`, `snd = LoadSound(...)` declares `snd As Sound`, etc.

### Current mechanism (where the default lives)

- `types::resolve_var_type(None, None) => (Type::Int, false)` — `crates/cb-sema/src/types.rs:159`. This is the "no sigil, no `As`" default.
- Applied during implicit declaration in `Checker::check_assign` — `crates/cb-sema/src/check.rs:1782`. The target type is resolved **before** the value expression is checked (`self.check_expr(value)` runs afterwards at line ~1797), then the value is coerced to the (Integer) target.
- The lowering pass reads the variable type from `self.types` (`crates/cb-sema/src/lower.rs:663-668`, falling back to `IrType::Int`), so **lowering follows whatever the checker records** — no separate lowering change is needed if the checker assigns the inferred type and inserts it into `self.types`.
- Spec: `docs/cb_syntax.md` §4.1 line 589 — "If the first reference has neither a sigil nor an `As` clause, the variable is `Integer`."

### What is *not* affected (already pins the type)

Only the **bare** `Stmt::Assign` target (an `Expr::Ident` with `sigil = None`) hits the Integer default. These forms already carry an explicit type and must keep their current behavior:

- Sigil form — `y# = 23.04` → `resolve_var_type(Some(Float), None)` → `Float`.
- `As` form — `z As String = "asd"` is parsed into a `Stmt::Dim` (`parser.rs:1197-1228`, `parse_implicit_decl_stmt`), not an `Assign`, so it never reaches the inference path.

So the change is narrowly scoped to: *undeclared bare ident, no sigil, as the LHS of an `Assign`.*

## Solution

Value-based inference for implicitly-declared variables, applied at all three implicit-declaration sites. Inference uses the value's **exact type** (numeric included — decision 1), so `obj = LoadObject(...)` → `Object`, `snd = LoadSound(...)` → `Sound`, `s = "x"` → `String`, `x = 3.14` → `Float`, `x = 5` → `Integer` (the literal `5` is `Int`).

Both lowering local-allocation paths read the declared type from the checker (`lower_main` from `decl.ty`, `lower.rs:497`; `scan_body_for_locals` from `self.types`, `lower.rs:663`), and `lower_for` reads the loop-var IR type from the allocated local (`lower.rs:1740`). So **once the checker records the inferred type, lowering follows automatically** at every site — no hardcoded `Int` overrides it.

### Bare assignment — `check_assign` (`check.rs:1781-1792`)

When the target is an undeclared bare ident with `sigil == None`:

1. **Check the value first** to obtain its type (reorder — today the target type is fixed before the value is checked).
2. Declare the variable with the value's type: `try_declare(...)` with the inferred `Type`, then `self.types.insert(target, inferred_ty)`.
3. No coercion is then needed (target type == value type).

When `sigil` is present, keep today's behavior (sigil wins; value coerces to the sigil type). The `As` form is unaffected (parses to `Stmt::Dim`).

### `For` loop variable (decision 2 — `check_for`, `check.rs:1867`)

Infer the loop-variable type from the **numeric promotion** of `from`/`to`/`step` (floored at `Int`, matching arithmetic — so `Byte`/`Short` bounds give an `Int` loop var). Requires reordering `check_for` to check the bounds before declaring the var:

1. Check `from`/`to`/`step`.
2. If the var is an undeclared bare ident with `sigil == None`: `var_ty = numeric_promote(from, to, step)` (floored at `Int`); fall back to `Int` if a bound is non-numeric / error.
3. Declare + record, then coerce the bounds to `var_ty` (as today, `check.rs:1926-1932`).

Lowering already reads the loop-var IR type from the allocated local (`lower.rs:1740-1747`), so a `Float` loop var lowers with float direction-test constants and a `1.0` default step automatically. `For i = 1.0 To 10.0` → `i As Float`; `For i = 1 To 10` → `i As Integer`.

`For Each` already infers from the source element type (`check.rs:1954`) — unchanged; it is the precedent this feature generalizes.

### Arrays (decision 3)

`a = New Integer[10]` implicitly declares `a As Integer[]` (the value's `Array { elem, rank }` type). This mirrors `Dim a As Integer[] : a = New Integer[10]`, which already lowers correctly, so it is a checker-side change only — verify `sema_type_to_ir` + array-local handling in lowering carry through.

### Guard cases

- **`Null` (decision 4)** — `x = Null` on an undeclared var is an **error** (new `E0331`, `E_CANNOT_INFER_TYPE`): *"cannot infer a type for `x` from `Null`; declare it explicitly, e.g. `Dim x As <Type>`."* `Null` has no concrete type to infer from.
- **Self-referential RHS (decision 5)** — `x = x + 1` where `x` is undeclared is now a **use-before-declaration error**. Because the value is checked first, the RHS `x` resolves to nothing and the existing `E0300` (undeclared identifier) fires naturally. To avoid a cascade, still declare the target as `Type::Error` afterwards (the root error is already reported).
- **Void** — `x = SomeSub()` (a Sub returns `Void`): do not record `Void` as a variable type; this path already errors on assigning a void value.
- **`Type::Error`** — the value already errored: declare the target as `Error` and suppress further inference (no cascade).

### Divergences from classic CoolBasic (intentional, per the decisions above)

- Numeric first-assignment type follows the value (`x = 3.14` → `Float`), not the classic `Integer` default / FD-035 narrowing.
- `x = Null` on an undeclared var is an error (`E0331`) rather than zero/Null-init.
- `x = x + 1` on an undeclared var is a use-before-declaration error (`E0300`) rather than classic zero-init-on-first-use.

These supersede the §4.1 "the variable is `Integer`" rule, which is rewritten accordingly.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-sema/src/check.rs` | MODIFY | `check_assign`: check value first, infer from value type when target is an undeclared bare ident with no sigil; emit `E0331` for `Null`; let self-ref fall through to `E0300`. `check_for`: reorder to check bounds first, infer loop-var type from `numeric_promote(from,to,step)` floored at `Int`. |
| `crates/cb-sema/src/diagnostics.rs` | MODIFY | Add `E_CANNOT_INFER_TYPE` = `E0331` (next free code; highest today is `E0330`). |
| `crates/cb-sema/src/types.rs` | MODIFY (maybe) | Helper to decide whether a `Type` is a valid *declarable* variable type (reject `Void`/`Null`); reuse/confirm a `numeric_promote` helper for the `For`-bounds promotion. |
| `crates/cb-sema/src/lower.rs` | VERIFY | All three sites read the declared type from the checker (`lower.rs:497`, `:663`, `:1740`); confirm no change needed once the checker records the inferred type, incl. an inferred array local. |
| `docs/cb_syntax.md` | MODIFY | Rewrite §4.1 line 589 + the `x = 213 // x is Integer` example to describe value-based inference, the numeric rule, the `Null`→`E0331` rule, and self-ref→use-before-declaration. |
| `crates/cb-sema/src/check.rs` (tests) | MODIFY | Inference unit tests (see Verification). |
| `crates/cb-driver/tests/...` | MODIFY | A driver golden exercising `obj = LoadObject(...)` used as an `Object` with no `Dim`. |

## Verification

- `cargo test -p cb-sema` — new inference tests:
  - `obj = LoadObject(...)` → `obj` typed `Object`; subsequent object commands accept it with no `Dim`.
  - `s = "hello"` → `String` (today this would be an Integer-coercion error).
  - `x = 5` → `Integer` (regression — must not change).
  - `x = 3.14` → `Float` (decision 1 — numeric inference accepted).
  - `a = New Integer[10]` → `a` typed `Integer[]` (decision 3); indexing `a[i]` type-checks with no `Dim`.
  - `For i = 1.0 To 10.0` → `i As Float`; `For i = 1 To 10` → `i As Integer` (decision 2).
  - `x = Null` (undeclared) → `E0331` (decision 4).
  - `x = x + 1` (undeclared) → `E0300` use-before-declaration (decision 5).
  - `x = SomeSub()` (Void) — no `Void` variable recorded; existing error path holds.
- Driver smoke: a program that does `obj = LoadObject("...")` then uses `obj` as an `Object` without a `Dim`, asserting it compiles and runs.
- Regression: existing implicit-Integer tests still pass; **audit for self-referential first assignments** (`x = x + 1` as a first statement) — these now error and any such fixture/snapshot needs updating. Re-review snapshots (only type-of-implicit-var diffs expected). Confirm the FD-032 fn-pointer assignment smoke (`fp = add`) still passes (now allowed without `Dim`).
- `cargo test --workspace`, `clippy --workspace --all-targets -D warnings`, `cargo fmt --all --check`.

### Implementation result (2026-06-23, Windows + Allegro SDK)

Landed exactly as planned, entirely in `cb-sema` plus the new diagnostic and the spec/tests —
**no `lower.rs` or `types.rs` change was needed** (lowering reads the checker-recorded type at
all three sites; `numeric_promote`/`resolve_var_type` already existed and were reused). The
`Void` guard reuses **E0331** with a tailored message (a Sub RHS has no value to infer from).

- `cargo test --workspace` — **0 failed** across every crate. New `cb-sema` tests (9):
  `fd042_infer_float_from_value`, `_infer_string_no_coercion_error`, `_infer_array_from_new`,
  `_infer_int_from_literal_regression`, `_null_cannot_infer_e0331`,
  `_self_reference_use_before_decl_e0300`, `_for_infer_float_from_bounds`,
  `_for_infer_int_from_bounds_regression`, and `fd042_infer_opaque_runtime_type_fd041`
  (`s = LoadSound(...)` → `Sound`, no `Dim`). The 45 `cb-sema` lowering snapshots passed
  **unchanged** (every existing `For`/`While` fixture is `Dim i As Int` first → no drift).
- Driver: headless golden `type_inference_fd042` (String/Float/array/Int+Float `For` inference,
  `Print`ed end-to-end) + graphics-gated `object_inference_fd042` (`o = MakeObject()` with no
  `Dim`, ran live against Allegro) + `cli.rs::infer_from_null_exits_one_e0331` (`x = Null` → exit
  1, `E0331`). FD-032 fn-pointer behavior unaffected (full driver suite green).
- `clippy --workspace --all-targets -D warnings` clean; `cargo fmt --all --check` clean.

## Related

- `docs/cb_syntax.md` §4.1 (Declaration / implicit declaration), §3.4 (implicit conversions).
- **For Each iteration-variable inference** — `crates/cb-sema/src/check.rs:1954` (existing precedent for value-type-driven implicit declaration).
- FD-035 (Type System Simplification — defines the scalar type set and the Integer default / narrowing rules this interacts with).
- FD-004 #4 (the `As`-annotated implicit declaration form `z As String = "asd"`, parsed to `Stmt::Dim`).
