# FD-027: Runtime-Command Name Collisions Produce an Unrenderable Diagnostic

**Status:** Pending Verification
**Priority:** Medium
**Effort:** Low-Medium
**Impact:** A user program that declares a name colliding with a runtime command (e.g. `Dim box As Int`, where `Box` is the rectangle-draw command) produces *no usable error* — the compiler prints an internal renderer failure (`Span references unknown FileId(4294967295)`) and swallows the real diagnostic. Fixing this restores a clear message for a common, easy-to-hit mistake and hardens the renderer against any synthetic-span label.

## Problem

Discovered during FD-019 verification. This program:

```cb
Dim box As Int
box = 5
Print Str(box)
```

fails to compile with:

```
cb-diagnostics: Span references unknown FileId(4294967295)
cb: failed to render diagnostic: Span references unknown FileId(4294967295)
```

instead of a real diagnostic. The user has no idea what is wrong.

**Root cause (two layers):**

1. **Synthetic span on runtime decls.** All runtime-catalog declarations are
   registered with a placeholder span: `register_runtime_catalog` uses
   `Span::new(0, 0, FileId::SYNTHETIC)` (`check.rs:172`), where
   `FileId::SYNTHETIC == FileId(u32::MAX)` (`source.rs:16`). Runtime commands
   have no source text, so this is the only span available.

2. **Collision → duplicate-decl diagnostic that points at the synthetic span.**
   CoolBasic names are case-insensitive, so `box` collides with the runtime
   command `Box` (`docs/cb_runtime.md:203` — `Box x, y, w, h, fill`). The
   implicit/`Dim` declaration calls `try_declare`, the symbol table returns
   `Err(prev_span)` with `prev_span = FileId::SYNTHETIC`, and the diagnostic
   attaches a secondary label there:
   `Label::with_message(prev_span, "previously declared here")` (`check.rs:141`).

3. **The renderer hard-errors on the synthetic FileId.** `validate_label`
   rejects any label whose `FileId` is not in the `SourceMap`
   (`render.rs:99–111`), returning `Err("Span references unknown
   FileId(...)")`. The driver treats that as a fatal render failure and the
   real "duplicate declaration of `box`" diagnostic never reaches the user.

So a single synthetic-span label anywhere in a diagnostic makes the *whole*
diagnostic unrenderable — fragile beyond just this collision case.

**Design decision (resolved).** Shadowing of runtime **commands** depends on
how the user declares the name:
- An **explicit** declaration (`Dim box As Int`, `Global box`, `Function box`)
  **shadows** the command — the name now refers to the user's
  variable/function. No error.
- An **implicit** declaration (a bare assignment `box = 5` with no prior
  declaration) **may not** shadow a command — it is an error with a tailored
  message that points the user to declare it explicitly with `Dim`.

Runtime-defined **constants** and runtime **types** remain **reserved** (a
colliding user declaration is `E0303`, explicit or not) — unchanged from
FD-029. This rule is recorded in `docs/cb_syntax.md` §1.5.1.

## Solution

Two independent fixes; (A) is the safety net and should land regardless of the
(B) decision.

**A. Renderer robustness (`cb-diagnostics`).** A label on the
`FileId::SYNTHETIC` sentinel now *degrades* instead of aborting the whole
diagnostic: `validate_label` returns `Ok` for it, and `to_codespan` drops the
snippet while folding any label message into a note (`<message> (built-in; no
source location)`). The message text, code, primary label, and notes still
render. This guarantees a synthetic span can never again swallow a real error.
Hard validation is kept for genuine caller bugs — *inverted* spans, out-of-range
offsets within a real file, and unknown *non-synthetic* `FileId`s (a wrong
`SourceMap` is a driver bug, so the existing `unknown_file_id` tests stay
`Err`). **Implemented** (this also future-proofs the renderer; after fix B the
sema no longer emits synthetic-span labels itself).

**B. Shadow vs. reserve in `cb-sema`.** Per the design decision:
- `Checker::declare_var_shadowing` (used by `check_dim`) lets an explicit `Dim`
  overwrite a same-scope runtime *command* entry via the new
  `SymbolTable::force_declare` / `local_is_runtime_command` helpers. A `Dim`
  inside a function declares into the function scope and shadows the top-level
  command through normal lookup, so it never needs the overwrite. Reserved
  runtime constants/types fall through to `try_declare` and still report
  `E0303`.
- `check_assign` detects a bare-identifier assignment target that resolves to a
  runtime command with no user declaration, and emits the new `E0328`
  ("built-in command … declare it explicitly with `Dim`") instead of the old
  confusing `cannot convert Int to Void`. No synthetic secondary label is ever
  produced.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-diagnostics/src/render.rs` | DONE | `validate_label` returns `Ok` for `FileId::SYNTHETIC`; `to_codespan`/`push_label_or_note` drop the snippet and fold the message into a note. New `emit_synthetic_label_degrades` test. |
| `crates/cb-sema/src/scope.rs` | DONE | `force_declare` (overwrite a scope entry) and `local_is_runtime_command` (synthetic-span `RuntimeFn`/`OverloadSet`) helpers; `FileId` import. |
| `crates/cb-sema/src/check.rs` | DONE | `declare_var_shadowing` used by `check_dim` (explicit shadow); `check_assign` emits `E0328` for an implicit assignment over a command. New FD-027 tests. |
| `crates/cb-sema/src/diagnostics.rs` | DONE | New `E0328` (`E_RUNTIME_COMMAND_AS_VAR`). |
| `docs/cb_syntax.md` | DONE | §1.5.1 documents the command shadow/reserve rule. |

## Verification

- The repro `Dim box As Int` / `box = 5` / `Print Str(box)` now **compiles
  cleanly** (the explicit `Dim` shadows the `Box` command) instead of printing
  `Span references unknown FileId`.
- `cb-diagnostics` `emit_synthetic_label_degrades`: a `Diagnostic` whose
  secondary label carries `FileId::SYNTHETIC` renders successfully (real message
  preserved, synthetic message folded into a note, no `Err`). The
  `emit_unknown_file_id_returns_invalid_input` / `unknown_file_id_returns_err`
  tests still pin the unknown-*non-synthetic* `FileId` case as `Err` (decision:
  a wrong `SourceMap` is a driver bug and stays hard).
- `cb-sema` FD-027 tests: `explicit_dim_shadows_runtime_command`,
  `explicit_dim_shadows_overloaded_runtime_command`,
  `dim_inside_function_shadows_runtime_command` (no diagnostics) and
  `implicit_assignment_over_runtime_command_is_e0328` (`E0328`). The FD-029
  reserved-constant tests (`user_dim_colliding_with_runtime_const_is_e0303`)
  still pass.
- `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` green.

## Related

- Surfaced during FD-019 (Interpreter Correctness & Memory-Safety Fixes) verification.
- [FD-006](archive/FD-006_DIAGNOSTICS_DRIVER_HARDENING.md) — added `validate_label` FileId/`Span` validation in the renderer (the check that now over-rejects synthetic spans).
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) / [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) — runtime catalog registration (`register_runtime_catalog`, `DeclKind::RuntimeFn`/`RuntimeTypeDef`).
- `docs/cb_runtime.md` §`Box` (the colliding command); `cb-diagnostics` `FileId::SYNTHETIC` (`source.rs:16`).
