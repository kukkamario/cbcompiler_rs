# FD-035: Type System Simplification — Classic Types + Long

**Status:** Pending Verification
**Priority:** Medium-High
**Effort:** Large (cross-crate: frontend, sema, IR, interp, FFI, spec, tests)
**Impact:** Aligns the scalar type set with **classic CoolBasic** (as evidenced by the `../cbEnchanted` reference), plus a single deliberate extension (`Long`). Removes the over-engineered unsigned-32/64 and `Bool` types that the current spec invented. This both simplifies the language and dissolves most of the original FD-035 defects — they were bugs in types we are now removing.

> **Supersedes** the original FD-035 ("Narrow/Unsigned Numeric Correctness"). That FD fixed `UInt`/`ULong`/`Short`-signedness behaviour inside the rich type model; this FD removes the rich model instead. The one surviving piece is `Short` → `u16`. See *Relationship to the original FD-035* below. (Filename retained as-is to avoid disrupting an open editor; rename to `FD-035_TYPE_SYSTEM_SIMPLIFICATION.md` at close time.)

## Motivation

The `../cbEnchanted` reference (authoritative classic-CoolBasic reimplementation) shows the **complete** classic scalar set is `Byte` (`uint8_t`), `Short` (`uint16_t`), `Int` (`int32_t`, signed), `Float` (32-bit single), and `String`. There is **no `Long`/int64, no `UInt`, no `ULong`, and no `Bool`**. Two structural facts drive this redesign:

1. **Booleans are just `Int`.** Every comparison and logical operator returns `int32_t`; `Any::toBool()` is literally `!= 0` (`../cbEnchanted/src/any.h`).
2. **`Byte`/`Short` are storage-only.** The runtime variant (`Any`) only ever holds `Int`/`Float`/`String`/`TypePtr`. A `Byte`/`Short` variable widens to `Int` when read onto the eval stack; **all arithmetic happens in `Int`**, and the result is narrowed back only on store (`../cbEnchanted/src/cbvariableholder.h`, `any.h`).

`cbcompiler_rs` currently diverges: `docs/cb_syntax.md` §3.1 adds `UInt`, `ULong`, `Long`, `Bool`, and 64-bit `Float`. We keep `Long` (genuinely useful for games — timers, large counts) and 64-bit `Float` (orthogonal, out of scope here), but drop the rest and adopt the classic widen-to-`Int` arithmetic model. This honours the CLAUDE.md principle: *if a feature is hard to implement cleanly in the interpreter, the model is wrong* — the per-type/unsigned arithmetic is exactly that friction.

## Decisions (locked)

- **Type set:** `Byte` (u8), `Short` (u16), `Int` (i32), `Long` (i64), `Float` (f64), `String`.
- **Removed types:** `UInt`, `UInteger`, `ULong`, `Bool`.
- **Reserved words** (kept as keywords, resolve to *no* type — using them in a type position is a hard error, never silently an identifier): `Bool`, `Boolean` (new), and — by the same principle — `UInt`, `UInteger`, `ULong`. Rationale: a clear "reserved/unsupported type" diagnostic beats silently treating `Dim x As UInt` / a stray `Bool` as an ordinary identifier, and it keeps the names free to reintroduce later.
- **Reserved symbol:** `!` stays a recognised token with **no** current meaning (no longer the `Bool` sigil). Using `!` as a sigil or operator is a hard error. Reserved for future use.
- **`True` / `False`:** keep the keywords, but they now denote the **`Int`** constants `1` and `0`. The `Bool()` conversion function is removed.
- **Arithmetic model:** `Byte`/`Short` widen to `Int` for *all* arithmetic, bitwise, shift, and comparison; narrowing back to `Byte`/`Short` happens only on store to such a location.
- **`Float` stays 64-bit** (classic is 32-bit single — a known, deliberate divergence, out of scope here).

## Resulting type system

| Type     | Repr | Signedness | Sigil | Role                              |
| -------- | ---- | ---------- | ----- | --------------------------------- |
| `Byte`   | u8   | unsigned   | —     | storage-only (widens to `Int`)    |
| `Short`  | u16  | unsigned   | —     | storage-only (widens to `Int`)    |
| `Int`/`Integer` | i32 | signed | `%`  | default integer; arithmetic type  |
| `Long`   | i64  | signed     | —     | arithmetic type (extension)       |
| `Float`  | f64  | —          | `#`   | arithmetic type                   |
| `String` | UTF-8| —          | `$`   | reference type                    |

Sigils: `%` Int, `#` Float, `$` String. `!` reserved (no meaning).

## Semantics

- **Integer arithmetic / bitwise:** each operand widens to at least `Int` (`Byte`/`Short` → `Int`). Result is `Long` if either widened operand is `Long`, else `Int`. Mixed with `Float` → `Float`.
- **Shifts (`Shl`/`Shr`/`Sar`):** result type is the widened **LHS** type (`Int` or `Long`); the count is read as an integer regardless of its declared type. (This is the same fix the original FD-035 proposed, but now the only LHS arithmetic types are `Int`/`Long` — no narrow/unsigned arms to special-case.)
- **`Pow` (`^`):** always `Float` (unchanged, per FD-020).
- **Comparisons (`=`,`<>`,`<`,`<=`,`>`,`>=`) and logical ops (`And`,`Or`,`Xor`,`Not`):** operands promote per the rules above; the **result is `Int`** (`1`/`0`). There is no `Bool` result type.
- **Conditions (`If`/`While`/`Until`, `For` direction tests):** test `<expr> <> 0`; any numeric is truthy. No `Bool` coercion step.
- **Literals:** an integer literal has type `Int`; a literal that exceeds `Int` range but fits `Long` has type `Long`; one that fits neither is a hard error. Assigning a literal to a narrower (`Byte`/`Short`) location is range-checked at compile time (in-range converts silently per the FD-020 rule; out-of-range is a hard error).
- **Narrowing on store:** a non-literal `Int`/`Long` value stored into a `Byte`/`Short`/`Int` (from `Long`) location narrows with the existing narrowing warning (FD-020 / `E0318`).
- **Promotion logic:** the same-width signed/unsigned "signed wins on tie" rule is **deleted** — no unsigned arithmetic types remain, so no tie exists. `numeric_promote` collapses to: `Byte`/`Short` → `Int` < `Long` < `Float`.

## Changes by area

| File | Action | Purpose |
|------|--------|---------|
| `docs/cb_syntax.md` | MODIFY (spec leads) | §1.4: remove `!` Bool sigil row, note `!` reserved. §3.1: type table → Byte/Short/Int/Long/Float/String; list `Bool`/`Boolean`/`UInt`/`UInteger`/`ULong` as reserved-unsupported. §3.4: drop unsigned-chain & Bool↔numeric conversion rules; comparisons/logical yield `Int`; `True`/`False` are `Int` 1/0; remove `Bool()` conversion. Fix scattered "Bool result" mentions (≈ lines 195–199, 214, 288, 379, 501–502, 514, 669, 759, 762, 1036). |
| `crates/cb-frontend/src/keywords.rs` | MODIFY | Add `Boolean` as a reserved keyword; keep `Bool`/`UInt`/`UInteger`/`ULong` as reserved keywords (no longer type-producing). |
| `crates/cb-frontend/src/token.rs` | MODIFY | Remove `!`→Bool sigil mapping; `!` becomes a reserved punctuation token with no meaning. |
| `crates/cb-sema/src/types.rs` | MODIFY | Drop `Type::{UInt,ULong,Bool}`; keyword→type map errors (reserved) for Bool/Boolean/UInt/UInteger/ULong; comparison/logical/`Not` result type → `Int`; `numeric_promote` simplified; range table drops UInt/ULong; `is_numeric`/`is_integer` updated. |
| `crates/cb-sema/src/convert.rs` | MODIFY | Remove Bool↔numeric and unsigned conversions; keep widen/narrow for Byte/Short/Int/Long + Int↔Float. |
| `crates/cb-sema/src/check.rs` | MODIFY | `True`/`False` → `Int` literals; conditions accept any numeric (`<> 0`); `check_dim`/`Global` coerce initializer to declared type (surviving original-FD-035 fix). |
| `crates/cb-sema/src/lower.rs` | MODIFY | Sema→IR type map drops UInt/ULong/Bool; lower `True`/`False` to `Int` const; emit `Convert` for narrowing `Dim`/`Global` inits. |
| `crates/cb-ir/src/types.rs` | MODIFY | Drop `IrType::{UInt,ULong,Bool}`. |
| `crates/cb-backend-interp/src/value.rs` | MODIFY | `Value`: drop `UInt`/`ULong`/`Bool`; `Short(u16)`; `is_truthy` = `!= 0`; `as_cb_string` drops `"True"/"False"` rendering. |
| `crates/cb-backend-interp/src/interp.rs` | MODIFY | `eval_binop`: comparisons/logical produce `Int`; shifts dispatch on widened LHS; remove unsigned/Bool arms; narrow→widen-to-Int model. |
| `crates/cb-backend-interp/src/ffi.rs` | MODIFY | Drop UInt/ULong/Bool marshaling arms. |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | Drop UInt/ULong/Bool from the IrType↔C-ABI map; **keep `CB_TYPE_*` numeric codes stable** (see FFI risk). |
| `runtime/cb_runtime_core.h`, `runtime/catalog.cpp` | AUDIT (likely no change) | Leave `CB_TYPE_UINT/ULONG/BOOL` codes reserved/unused; verify no catalog function signature uses `uint32_t`/`uint64_t`/`bool` (adjust the offender if one exists). |
| tests + insta snapshots | MODIFY | Regenerate lowering/IR snapshots that referenced Bool/UInt/ULong or now carry `Convert`; rework FD-032 narrow-width tests; remove unsigned-arithmetic tests. |

## FFI / ABI risk

The `CB_TYPE_*` codes (`runtime/cb_runtime_core.h:33`) are a **wire-ABI contract** with the C++ runtime, and `runtime/catalog.cpp:101` maps `uint32_t→UINT`, `uint64_t→ULONG`, `bool→BOOL`. **Do not renumber the codes** — that would force a lockstep ABI/version bump. Instead: stop the frontend from ever producing UInt/ULong/Bool, leave the codes reserved, and **audit `catalog_funcs` for any signature using `uint32_t`/`uint64_t`/`bool`**. If none (likely — the `type_tag<>` specializations appear to be unused completeness), the C++ side needs no change; if one exists (e.g. a colour packer), change its signature (`uint32_t`→`int32_t`) rather than keep the type alive.

## Relationship to the original FD-035

| Original defect | Fate under this redesign |
|-----------------|--------------------------|
| #1 `Dim x As UInt = 1` not coerced | UInt removed; **the underlying "coerce Dim/Global init to declared type" fix survives** (still needed for `Byte`/`Short`/`Float`-from-int-literal). |
| #2 shift dispatch on mismatched widths | **Dissolves** — shifts widen to `Int`/`Long`; the LHS-dispatch fix remains but with only two arms. |
| #3 `Value::Short` is `i16`, should be `u16` | **Survives** — `Short` → `u16`. |
| #4 unsigned same-width comparison promotion | **Vanishes** — no unsigned arithmetic types remain. |

## Verification

- `cargo build --workspace` and `cargo test --workspace` (0 failed); `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Reserved-word diagnostics: `Dim x As Bool` / `As Boolean` / `As UInt` / `As ULong` each produce a clear "reserved/unsupported type" error; a bare `!` is a clear error; none are silently accepted as identifiers.
- `True`/`False` evaluate to `Int` `1`/`0` (e.g. `Print True + True` → `2`).
- Arithmetic model: `Byte`/`Short` operands compute in `Int` and narrow on store; per-width wrap/shift/comparison programs cross-checked by hand against the updated §3.4.
- Un-`#[ignore]` and rework the FD-032 narrow-width tests; `short_holds_documented_unsigned_range` exercises `u16` storage. Drop the UInt/ULong-specific tests.
- `Dim x As Float = 1` (int literal → Float) and `Dim b As Byte = 300` (out-of-range literal → hard error) lock the coercion/range rules.
- Regenerate any lowering/IR snapshots that shift.
- Confirm the C++ runtime still builds (SDK-free path) after the FFI audit.

## Related

- **Supersedes** the original narrow/unsigned scope of this FD; **surfaced by [FD-032](archive/FD-032_INTERPRETER_HARDENING_TESTS.md)** (the narrow-width hardening tests, currently `#[ignore]`d).
- [FD-019](archive/FD-019_INTERPRETER_CORRECTNESS_FIXES.md) — shift width-correctness for `Int`/`Long`.
- [FD-020](archive/FD-020_SEMA_NUMERIC_AND_FOR_LOOP_SEMANTICS.md) — numeric coercion, in-range-literal-silent rule, and the same-width-signed-tie promotion this redesign **removes**.
- Reference: `../cbEnchanted/src/any.h`, `cbvariableholder.h` — the classic type model this aligns to.
