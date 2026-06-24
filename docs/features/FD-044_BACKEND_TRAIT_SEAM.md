# FD-044: Backend Trait Seam

**Status:** Planned
**Created:** 2026-06-24
**Priority:** Medium (blocking for LLVM codegen)
**Effort:** Medium (a load-bearing structural decision — trait shape, crate placement)
**Impact:** Materializes the "pluggable backend" architecture the project committed to up front, so adding the LLVM backend is implementing a trait rather than threading new `cfg`'d `match` arms through the driver.

> Surfaced by the Bundle 6 code-review cleanup (finding **DR-R10**). This is a structural/architectural decision (CLAUDE.md flags backend-trait shape as one to decide with the user, not unilaterally), so it is filed Planned for a dedicated design pass rather than folded into a cleanup bundle.

## Problem

CLAUDE.md states the backend is pluggable: "The frontend … must not depend on any specific backend. A backend should be selectable at compile time (cargo features) and ideally at runtime (CLI flag)." Today there is **no materialized `Backend` trait**. The driver dispatches each backend through hand-written, `cfg`-gated `match` arms (`crates/cb-driver/src/main.rs`): `Some(Backend::Interp) => cb_backend_interp::interpret(...)`, `#[cfg(feature = "llvm")] Some(Backend::Llvm) => <unimplemented>`. `cb-backend-llvm` is a 3-line stub (`crates/cb-backend-llvm/src/lib.rs`).

This works for one real backend, but every backend-specific concern (how a program is run, exit-code mapping, dump hooks, diagnostics) lives inline in the driver. When LLVM codegen begins, it will accrete a second parallel set of driver arms instead of slotting behind a shared seam — and the "selectable at runtime" goal has no interface to hang on.

## Proposed direction (to be designed — needs user input on trait shape)

Open questions to resolve in the design pass:

- **Where does the trait live?** The IR is the natural backend-agnostic seam (`cb-ir`), but a `Backend` trait that "runs a program and returns an exit code" pulls in process/exit-code concerns that may not belong in `cb-ir`. Candidates: `cb-ir`, a new `cb-backend-api` crate, or `cb-driver` (rejected — that defeats pluggability).
- **What is the trait's surface?** Minimally something like `fn run(&self, program: &ir::Program, ...) -> Result<i32, BackendError>`; but AOT (LLVM) "produces an artifact" while interp "executes" — the trait must accommodate both (e.g. a `Backend` that returns an outcome enum, or separate `Run`/`Compile` capabilities).
- **Compile-time vs runtime selection.** Keep the cargo-feature gating (`interp` default, `llvm` opt-in) but resolve the selected backend to a `dyn Backend` / enum behind the `--backend` flag, so the driver's run site becomes backend-agnostic.
- **Migration.** Move the existing interp dispatch behind the trait first (no behavior change, all four feature combos green — see FD-025's matrix), then implement the LLVM backend against the same trait.

## References

- CLAUDE.md — architectural ground rules ("backend is pluggable", "two backends from day one", "shared IR is the natural seam").
- [[FD-025]] Driver CLI, Backend-Selection & Exit-Code Correctness — current `Backend` enum, `--backend` flag, exit-code contract, four-feature-combo test matrix.
- [[FD-006]] Diagnostics & driver hardening — original optional-backend cargo-feature wiring.
- Code review finding DR-R10.
