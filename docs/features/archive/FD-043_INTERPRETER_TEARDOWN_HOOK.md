# FD-043: Interpreter Runtime Teardown Hook (`about_to_exit`)

**Status:** Complete
**Completed:** 2026-06-26
**Created:** 2026-06-24
**Designed:** 2026-06-26 (via `/fd-deep`, 4-agent analysis)
**Implemented:** 2026-06-26
**Priority:** Low
**Effort:** Low–Medium (one focused change in `cb-backend-interp` + a runtime-side teardown)
**Impact:** Lets the C++ runtime release process-global resources (the Allegro display, audio channels, open-file buffers) on a clean program exit instead of relying on process teardown; removes a standing `#[allow(dead_code)]` and proves the `cb_runtime_init` hook channel end-to-end.

> Surfaced by the Bundle 6 code-review cleanup (finding **II-V23**).

## Problem

`cb-backend-interp` stashes the `CbRuntimeHooks` returned by the FD-015/FD-024 `cb_runtime_init` handshake in a `runtime_hooks` field, but that field is dead — it carries `#[allow(dead_code)]` and is never read (`crates/cb-backend-interp/src/interp.rs:118-119`). FD-015 explicitly **reserved** an `about_to_exit` teardown hook and FD-024 stashed the hooks "for the FD-015 `about_to_exit` teardown the design reserves" — but nothing ever invokes it. On the C side the slot is hard-coded `nullptr` (`runtime/cb_host.cpp:17-23`).

Today the program ends (normal completion, `End`/`Halt`, `request_exit`, or a trap) and the interpreter simply returns; any process-global runtime state is reclaimed only by OS process teardown. That is fine for a one-shot CLI but is the wrong contract for resources that want an orderly flush (open-file buffers, audio, the display), and it leaves the reserved channel unproven.

## Solution

### Scope decision

This FD delivers **(1)** the interpreter-side wiring that fires the hook exactly once per run on every termination path, and **(2)** a minimal, real C++ teardown body for the full (Allegro) build. The SDK-free build's teardown is a no-op (nothing process-global to release that the OS won't reclaim).

**Explicitly out of scope** (see *Out of scope* below): a runtime *reset / re-init* contract that would let multiple CoolBasic programs run in one process (REPL / LSP eval / in-process harness). That motivation, noted in the original problem framing, is **not deliverable by this hook alone** — it additionally requires clearing the C++ subsystem statics and rebinding `g_host`, which no symbol does today and which is architecturally blocked from living in core. Firing `about_to_exit` once is *necessary but not sufficient* for that use case; it is split out to a future FD.

### Interpreter side (`cb-backend-interp`)

The single fire point is a thin wrapper around the existing `run()` body. `run()` has **five** return points, three of which (`interp.rs:205` `find_main()?`, `:209` `@main`-not-user-defined, `:215` `push_frame()?`) sit **before** `exec_loop()`. Wrapping only the `match self.exec_loop()` result would silently skip those three. So:

1. Rename the current `run()` body to a private `run_inner()`.
2. Make `run()`:
   ```rust
   pub fn run(&mut self) -> Result<i32, InterpError> {
       let result = self.run_inner();
       self.fire_about_to_exit();
       result
   }
   ```
   Every `?`, every early `return`, and every `exec_loop` exit lives inside `run_inner`, so control unconditionally reaches `fire_about_to_exit()` exactly once on all five paths (clean `Ok(0)`, `Halt`→`Ok(code)`, `Exit`→`Ok(code)`, trap/error `Err`, and the startup-failure errors).
3. `fire_about_to_exit` = `if let Some(f) = self.runtime_hooks.about_to_exit { f() }`. Remove the `#[allow(dead_code)]` once the field is read (`interp.rs:118`).

**Not `Drop`.** A `Drop for Interpreter` is blocked: `with_observer` (`interp.rs:184-199`) moves fields out of `self` to rebuild the struct under a new observer type, which Rust forbids for a type implementing `Drop` (E0509). A wrapper is also more honest about exactly-once and keeps the interpreter "simple and observable" per the project ground rules.

**Fire-once guard.** `run()` takes `&mut self` and the only caller (`lib.rs:26-27` `interpret()`) calls it once, but the signature permits re-runs. Add a `Cell<bool>` "already fired" guard on the `Interpreter` so a repeat `run()` (or a future safety-net) cannot double-fire. The C side is independently idempotent (below), so this is belt-and-suspenders.

### Hook contract

- **Timing:** Fires after the last user instruction, before `run()` returns — i.e. while the interpreter's full state and the runtime's string/catalog ABI are all still alive.
- **Ordering constraint (corrected premise):** The string/catalog API is immortal `&'static` `.rodata` (`interp.rs:113,119`: "never moves, never drops") — there is *no* catalog/string-API teardown to order against. The real constraint is the inverse: the interpreter still owns live `CbStringHandle`s (in `globals`/`heap`/`type_lists`/`frame_pool`) that drop *after* `run()` returns. **`about_to_exit` must not free the string allocator or heap ABI**, or those later drops become use-after-free. Its remit is *functionality* resources only (display, audio, files).
- **Payload:** Keep the existing ABI `void about_to_exit(void)` (`runtime/cb_runtime_core.h:155-160`, `runtime-sys/src/lib.rs:114-118`, `sizeof == 16` pinned at `lib.rs:127`). No exit-code argument for now; adding one is a semantic ABI change requiring a `CB_HOST_ABI_VERSION` bump (currently 1, `lib.rs:148`) and is deferred until a consumer needs it.
- **Idempotency:** Required on the C side (a program can already tear down its display inline before exit — see below), and enforced on the Rust side by the `Cell<bool>` guard.

### Runtime side (C++)

**Teardown body (coarse, matching the cbEnchanted reference `cleanup() + al_uninstall_system()`):** flush sound channels, then a single `al_uninstall_system()` that releases display, addons, and audio in one call. Correct for a one-shot CLI; it invalidates all Allegro state, which is fine because nothing re-runs in-process today (a future in-process REPL would revisit granularity under the separate reset/re-init FD).

**Layering (FD-016 core/functionality split).** `g_hooks` lives in `cb_host.cpp`, which is part of the **Allegro-free** `cb_runtime_core` and is compiled in *both* builds. It therefore cannot directly reference `al_uninstall_system`. Resolve with a small **teardown-registration** seam kept in core:

- Core (`cb_host.cpp`) holds a fixed-size `static void (*g_teardowns[N])(void)` + count, exports `cb_runtime_register_teardown(fn)` (Allegro-free), and sets `g_hooks.about_to_exit` to a core function that iterates and invokes the registered callbacks (so the slot is always non-null and `size`-guard-clean).
- The full-build graphics/runtime init (`cb_gfx.cpp` `ensure_init`, `cb_gfx.cpp:160`) registers the Allegro teardown callback. Because registration is lazy, a program that never touches graphics registers nothing and `about_to_exit` is a clean no-op — no spurious `al_uninstall_system` on a non-graphics program.
- SDK-free build: the graphics TU isn't compiled (`build.rs` omits `cb_gfx.cpp`/`cb_sound.cpp` under `-DCB_NO_ALLEGRO`), nothing registers, the callback list is empty → no-op.

*(Alternative considered: a single aggregator TU compiled in both builds with the body under `#ifndef CB_NO_ALLEGRO`. Simpler diff, but reintroduces a build-flag `#ifdef` in the teardown body and is less extensible. The registration seam keeps core pure and generalizes to future subsystems; prefer it unless implementation friction argues otherwise.)*

**C-side idempotency.** The window-close path already destroys the display inline and routes through `request_exit(0)` (`runtime/cb_gfx.cpp:359-374`). The registered teardown must therefore guard against double-teardown (static "done" flag; treat an already-destroyed display / already-uninstalled system as a no-op) so the inline path + `about_to_exit` together run it at most once.

### Out of scope (deferred to future FDs)

- **Multi-program-in-one-process** (the reset/re-init contract that clears subsystem statics and rebinds `g_host`). The blocker, not this hook.
- **Open-file flush-on-exit.** Open files are *untracked* (`cb_file.cpp` has no registry; each `cb_rt_open_*` does a bare `new CbFile`), so the coarse teardown cannot flush them without first adding a file registry. The CRT flushes `FILE*` buffers at process exit today, so this is a latent gap only for the in-process-host scenario — file it with the reset/re-init work.
- **Passing the exit code to the hook** (ABI bump; no consumer yet).

## Test plan

1. **Interpreter contract (primary, thread-safe).** Add an `on_exit` callback to the `Observer` trait, invoked from `fire_about_to_exit`. Add per-path tests reusing the existing `Rc<RefCell<usize>>` recorder pattern (cf. `integration.rs` `CountingObserver`): normal completion, `End`/`Halt`, `request_exit(n)`, a div-by-zero trap, and `MakeError`. Assert the counter `== 1` in each. Each test owns its observer instance, so it is safe under parallel `cargo test`.
2. **C hook end-to-end (secondary).** Following the `cb_rt_string_test_refcount` precedent (`runtime-sys/src/lib.rs`, `cb_runtime_core.h`), a test-only teardown callback that bumps an atomic counter + an instrumentation accessor, asserting exactly one invocation and that a second invocation is a safe no-op (idempotency).

## Implementation outcome (2026-06-26)

Implemented as designed. Summary:

- **Interpreter** (`cb-backend-interp`): `run()` is now a thin wrapper over `run_inner()` that fires `fire_about_to_exit()` once on every termination path; added `Observer::on_exit(exit_code)`, an `about_to_exit_fired` latch, and removed the `#[allow(dead_code)]` on `runtime_hooks`. Exit code surfaced to the hook is `Ok(code)` as-is, `Err → 1`.
- **C++ runtime**: core teardown-registration seam in `cb_host.cpp` (`cb_runtime_register_teardown` + `run_teardowns`, de-duped, Allegro-free); `g_hooks.about_to_exit` now points at `run_teardowns`. Full build registers a coarse `allegro_teardown` (`cb::sound::flush_all()` + `al_uninstall_system()`, `static bool done` guarded) from `cb_gfx.cpp`'s `ensure_init`; new `cb::sound::flush_all()` destroys all live channels. SDK-free build registers nothing → no-op. ABI unchanged (`void(void)`, `sizeof==16`).
- **FFI** (`cb-runtime-sys`): declared the two bare symbols; updated `runtime_init_roundtrip` (slot is now `is_some`, dispatch bumps the counter once).
- **Tests**: 5 per-path `on_exit` fires-once tests (normal, `End`, `request_exit`, div-by-zero trap, `MakeError`) + the C-channel counter assertion.

**Verification:** `cargo test --workspace` (all green, incl. the new tests), `cargo clippy --workspace --all-targets -- -D warnings` (clean), and a full Allegro build smoke test (`Screen`/`End` program exits 0 — `al_uninstall_system` runs cleanly, no double-free). Known minor edge (documented in code): a pure-audio/no-graphics program never runs `ensure_init`, so its coarse teardown isn't registered — audio is reclaimed by the OS at process exit instead.

**Not done here (future FDs):** multi-program-in-one-process reset/re-init; open-file flush; exit-code over the ABI. Remaining manual check: interactive window-close on a full-build example (e.g. `examples/bounce.cb`) — code-reviewed but not auto-tested (blocking GUI).

## References

- [[FD-015]] Runtime Trap Channel — reserved `about_to_exit`, declared the hook table.
- [[FD-024]] Runtime FFI ABI Hardening — stashed the hooks; documented the field as reserved; the `sizeof == 16` ABI pin.
- [[FD-016]] Runtime Core/Functionality Split — the layering constraint that shapes the C-side registration seam.
- cbEnchanted reference runtime — `src/main.cpp:6-30` (`cleanup()` + `al_uninstall_system()`); authoritative end-of-program contract.
- Code review finding II-V23.
