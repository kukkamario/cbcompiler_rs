# FD-035: Narrow/Unsigned Numeric Correctness

**Status:** Open
**Priority:** Medium-High
**Effort:** Medium (1-4 hours)
**Impact:** A narrow/unsigned variable initialized from an integer literal currently holds a plain `Int` at runtime, so width and signedness are silently lost. Fixing it makes `Byte`/`Short`/`UInt`/`ULong` behave per `docs/cb_syntax.md` §3.1/§3.4 in the reference interpreter — the backend both other backends are validated against.

## Problem

Surfaced by the **FD-032** interpreter-hardening tests (the narrow-width cases). Three entangled defects:

### 1. `Dim x As <T> = <init>` is not coerced to the declared type

`check_dim` only *checks* the initializer, it never `coerce`s it to the declared variable type (unlike `check_assign`, which does):

```rust
// crates/cb-sema/src/check.rs  (check_dim, ~1733)
if let Some(init_id) = init {
    self.check_expr(init_id);   // no coercion to var_ty
}
```

…and lowering stores the init register as-is, with no `Convert`:

```rust
// crates/cb-sema/src/lower.rs  (~1271)
let val = self.lower_expr(init_id);
// ... StoreLocal { local, value: val }   // val keeps the init's IR type
```

So `Dim u As UInt = 1` stores `Value::Int(1)`, not `Value::UInt(1)`: the runtime value type diverges from the declared/sema type. This affects every `Dim`/`Global` with an initializer whose natural type differs from the declared type (narrow/unsigned widths, and `Float`-from-int-literal). The reason it went unnoticed: existing `Dim …= literal` tests all use matching types (`Dim x As Int = 42`, `Dim t As Float = 0.0`).

*Subtlety:* `Global` init has the same shape (`lower.rs` ~1286) — fix both. Watch the FD-020 rule that an **in-range integer literal** converts silently (no `E0318` narrowing warning); only genuinely narrowing non-literal inits warn.

### 2. `eval_binop` cannot dispatch a shift with a mismatched count type

`eval_binop` requires both operands to share a `Value` variant:

```rust
// crates/cb-backend-interp/src/interp.rs  (~916)
(Value::UInt(a), Value::UInt(b)) => self.uint_binop(op, *a as u64, *b as u64, span, false),
// ... no (UInt, Int) arm
_ => Err(/* "type mismatch in binop" */),
```

A shift `u Shl 31` has a narrow/unsigned **LHS** and an `Int` **count** (sema deliberately does *not* coerce shift operands to a common type — `check_binary` skips coercion for `Shl`/`Shr`/`Sar`). So even once defect #1 is fixed and the LHS is a real `UInt`, the `(UInt, Int)` pair has no arm and falls through to the type-mismatch error. Shifts should dispatch on the **LHS** type and read the RHS as an integer count regardless of its variant.

*(Today, with defect #1 present, the LHS is `Int`, so `u Shl 31` evaluates as 32-bit signed: `1 Shl 31` → `-2147483648`; for `ULong`, `1 Shl 63` reduces the count mod 32 → also `-2147483648`.)*

### 3. `Value::Short` is signed, but `Short` is documented unsigned

`crates/cb-backend-interp/src/value.rs` defines `Short(i16)`, and `eval_binop` routes `Short` through the **signed** `int_binop`. `docs/cb_syntax.md` §3.1 documents `Short` as **16-bit unsigned**. `Byte(u8)` already matches the doc; `Short` does not. Decide the canonical representation (most likely `u16` + route through `uint_binop`) and update conversions / `Str` rendering.

### 4. (Investigate) unsigned comparison promotion

`numeric_promote` prefers the **signed** type on a same-width tie (FD-020), so `UInt > 0` may promote the `UInt` operand to `Int` and compare as signed — wrong when the high bit is set. Confirm whether this produces incorrect results for unsigned comparisons and fix in the same pass if so.

## Solution

- **check_dim / check_global:** coerce the initializer to the declared type via the existing `self.coerce(...)` path (mirror `check_assign`), so lowering emits the right `Convert`. Preserve the FD-020 in-range-literal-is-silent rule.
- **eval_binop:** special-case `Shl`/`Shr`/`Sar` to dispatch on the LHS `Value` type and read the count from the RHS independently (any integer variant).
- **Short:** change `Value::Short` to `u16` and route it through `uint_binop`; update `convert_value`, defaulting, and display. Cross-check the §3.4 rules for every width.
- **Comparisons:** if defect #4 is confirmed, make same-width unsigned comparisons compare as unsigned.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-sema/src/check.rs` | MODIFY | `check_dim` (and `Global` init) coerce the initializer to the declared type |
| `crates/cb-backend-interp/src/interp.rs` | MODIFY | `eval_binop` shift dispatch on LHS type; `Short`→`uint_binop`; conversions |
| `crates/cb-backend-interp/src/value.rs` | MODIFY | `Value::Short(u16)` (documented unsigned); display |
| `crates/cb-backend-interp/tests/integration.rs` | MODIFY | Un-ignore + extend the 3 FD-032 narrow-width tests |

## Verification

- Un-`#[ignore]` and pass the three FD-032 tests: `uint_shift_stays_unsigned_32bit`, `ulong_shift_stays_unsigned_64bit`, `short_holds_documented_unsigned_range` (the last now genuinely exercises `Short` storage).
- Add per-width arithmetic / wrap / comparison / shift programs, expected values cross-checked by hand against `docs/cb_syntax.md` §3.4.
- A `Dim x As Float = 1` (int literal → Float) test to lock the broadened coercion.
- `cargo test --workspace` (0 failed) and `cargo clippy --workspace --all-targets -- -D warnings` clean. Re-review any lowering snapshots that shift because `Dim` inits now carry a `Convert`.

## Related

- **Surfaced by [FD-032](FD-032_INTERPRETER_HARDENING_TESTS.md)** — the narrow-width hardening tests (currently `#[ignore]`d with reasons pointing here).
- [FD-019](archive/FD-019_INTERPRETER_CORRECTNESS_FIXES.md) — shift width-correctness for `Int`/`Long` (the narrow widths got no equivalent fix).
- [FD-020](archive/FD-020_SEMA_NUMERIC_AND_FOR_LOOP_SEMANTICS.md) — numeric coercion rules, the in-range-literal-silent rule, and the same-width-signed-tie promotion this revisits.
