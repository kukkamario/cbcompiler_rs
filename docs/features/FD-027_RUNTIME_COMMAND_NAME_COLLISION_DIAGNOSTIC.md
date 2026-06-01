# FD-027: Runtime-Command Name Collisions Produce an Unrenderable Diagnostic

**Status:** Open
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

**Open design question (needs an answer before implementing the UX half):**
should declaring a variable that collides with a runtime command name be an
error at all, and if so how phrased? Options:
- Reject with a tailored message (`` `box` conflicts with the built-in command `Box` ``) and no source secondary label.
- Allow user declarations to shadow runtime commands in inner scopes (treat the catalog as an outermost scope), erroring only on a true same-scope redeclaration.
Confirm intended CoolBasic behavior (original CB likely just reserves command
names globally) and record it in `docs/cb_syntax.md`.

## Solution

Two independent fixes; (A) is the safety net and should land regardless of the
(B) decision.

**A. Renderer robustness (`cb-diagnostics`).** A label that references a
synthetic/unknown `FileId` should *degrade*, not abort the whole diagnostic:
render the message text (and code, primary label, notes) without a source
snippet for that label — e.g. emit the label message with a `<built-in>` /
`<no source>` placeholder instead of returning `Err`. This guarantees a
synthetic span can never again swallow a real error. Keep the existing hard
validation for *inverted* spans / out-of-range offsets within a real file
(those are genuine bugs).

**B. Better collision diagnostic (`cb-sema`).** Make `try_declare` (or its
callers) detect that `prev_span` belongs to a runtime-catalog decl — e.g. mark
runtime decls (a `DeclKind` flag or an `is_builtin` bit / recognise
`FileId::SYNTHETIC`) — and emit a purpose-built message without a dangling
source label, per the design decision above. This removes the confusing
"previously declared here" pointing at nothing.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-diagnostics/src/render.rs` | MODIFY | `validate_label`/`emit`: degrade gracefully on synthetic/unknown `FileId` (render message without snippet) instead of returning `Err` |
| `crates/cb-sema/src/check.rs` | MODIFY | Detect runtime-decl collisions in `try_declare`; emit a tailored "conflicts with built-in command" diagnostic without a synthetic secondary label (per design decision) |
| `crates/cb-sema/src/diagnostics.rs` | MODIFY (maybe) | New error code if a distinct "name conflicts with built-in" diagnostic is chosen |
| `docs/cb_syntax.md` | MODIFY | Document whether runtime-command names are reserved / shadowable |

## Verification

- `cargo run -p cb-driver -- box.cb` on the repro above prints a clear,
  rendered diagnostic (no `Span references unknown FileId`).
- New `cb-diagnostics` renderer test: a `Diagnostic` whose (secondary) label
  carries `FileId::SYNTHETIC` renders successfully (message preserved, no
  `Err`), complementing the existing `emit_unknown_file_id_returns_invalid_input`
  test (which pins the *unknown-real-file* case — decide whether that stays an
  error or also degrades).
- New `cb-sema` test: `Dim box As Int` produces the intended diagnostic
  (tailored code/message), not a duplicate-decl with a dangling label.
- `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` green.

## Related

- Surfaced during FD-019 (Interpreter Correctness & Memory-Safety Fixes) verification.
- [FD-006](archive/FD-006_DIAGNOSTICS_DRIVER_HARDENING.md) — added `validate_label` FileId/`Span` validation in the renderer (the check that now over-rejects synthetic spans).
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) / [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) — runtime catalog registration (`register_runtime_catalog`, `DeclKind::RuntimeFn`/`RuntimeTypeDef`).
- `docs/cb_runtime.md` §`Box` (the colliding command); `cb-diagnostics` `FileId::SYNTHETIC` (`source.rs:16`).
