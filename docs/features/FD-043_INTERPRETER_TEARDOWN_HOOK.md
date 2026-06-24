# FD-043: Interpreter Runtime Teardown Hook (`about_to_exit`)

**Status:** Planned
**Created:** 2026-06-24
**Priority:** Low
**Effort:** Low–Medium (one focused change in `cb-backend-interp` + a runtime-side hook)
**Impact:** Lets the C++ runtime release process-global resources (open files, audio channels, the Allegro display) on a clean program exit instead of relying on process teardown; removes a standing `#[allow(dead_code)]`.

> Surfaced by the Bundle 6 code-review cleanup (finding **II-V23**). Filed for tracking; not yet designed.

## Problem

`cb-backend-interp` stashes the `CbRuntimeHooks` returned by the FD-015/FD-024 `cb_runtime_init` handshake in a `runtime_hooks` field, but that field is dead — it carries `#[allow(dead_code)]` and is never read (`crates/cb-backend-interp/src/interp.rs`, the `Interpreter` struct). FD-015 explicitly **reserved** an `about_to_exit` teardown hook ("Out of scope (deferred): … `about_to_exit` wiring") and FD-024 stashed the hooks "for the FD-015 `about_to_exit` teardown the design reserves" — but nothing ever invokes it.

Today the program ends (normal `Halt`, `End`, `request_exit`, or a trap) and the interpreter simply returns; any process-global runtime state is reclaimed only by OS process teardown. That is fine for a one-shot CLI but is the wrong contract for: (a) a future host that runs multiple programs in one process (test harness, REPL, LSP eval), and (b) resources that want an orderly flush (file buffers, audio, the display).

## Proposed direction (to be designed)

- Invoke the reserved `about_to_exit` hook from the interpreter's single exit point in `run()` — on every termination path (clean `Ok(code)`, `Exit`, and the trap/error path), so the runtime always gets exactly one teardown notification.
- Decide the hook's contract: does it run after the last user instruction but before the catalog/string API is torn down? Is it idempotent? What does it receive (exit code)?
- Remove the `#[allow(dead_code)]` once the field is read.
- Add a C++-runtime-side implementation (or confirm the existing `CbRuntimeHooks` slot is the right place) and a test that asserts the hook fires once per run on each termination path.

## References

- [[FD-015]] Runtime Trap Channel — reserved `about_to_exit`, declared the hook table.
- [[FD-024]] Runtime FFI ABI Hardening — stashed the hooks; documented the field as reserved.
- Code review finding II-V23.
