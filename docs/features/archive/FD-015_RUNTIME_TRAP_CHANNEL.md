# FD-015: Runtime Trap Channel

**Status:** Complete
**Completed:** 2026-05-31
**Priority:** High
**Effort:** Medium (1-4 hours) — design-heavy; touches the catalog ABI and both backends
**Impact:** A generic, backend-agnostic mechanism for the C runtime to signal the host (interpreter today, LLVM later) — cleanly terminate, raise an error, or fire other host hooks — instead of calling `exit()` from inside an FFI call or silently clamping bad arguments. Unblocks honest error handling for the File I/O batch and removes three accumulated warts.

## Problem

Runtime functions run "below" the backend: the interpreter dispatches them through libffi (FD-012), and the future LLVM backend will call them as ordinary native functions. A runtime function therefore has **no clean way to interrupt the program it is running inside**. Today this gap is worked around inconsistently:

- **Window close calls `exit(0)`** from inside `cb_rt_drawscreen` (`runtime/cb_gfx.cpp`) — bypasses the interpreter's clean shutdown, observers, and the `Halt`-based exit-code path established in FD-013 Batch 3.
- **String/graphics out-of-range args clamp instead of erroring** (`runtime/cb_string.cpp`) — `Left("hi", 5)` saturates rather than raising, because there is no trap channel. The legacy ran a fatal `error()` here.
- **`EscapeKey` had to be demoted to a pure query** (FD-013 Batch 5) — the legacy "safe exit" (Esc auto-closes) was dropped because the runtime cannot ask the host to stop.

Each was deferred with the same note: *"needs a runtime→interpreter trap channel that does not exist yet."* The **File I/O batch (FD-013 Batch 6) will make this acute** — `OpenToRead` on a missing file, a read past EOF, etc. need to surface a real runtime error, not a sentinel.

The interpreter already has the *receiving* half partly in place: `interpret`/`run`/`exec_loop` return `Result<i32, _>` (the process exit code), `Terminator::Halt { code }` exists, and the `Observer` trait has an `on_trap` hook (FD-010). What's missing is the **channel from C back to the host** and a backend-agnostic contract for it.

This should be a *generic framework*, not a one-off "exit" callback — many host hooks are foreseeable (request exit with code, raise a runtime error with a message, possibly: yield/poll for cancellation, structured logging, debugger breakpoints). The design goal is one extensible seam that both backends implement.

## Solution

### Scope

The channel carries **only cooperative, runtime-originated signals** — a runtime function asking the host to exit, raise an error, or fire a registered hook. It is *not* for compiler-detected hard faults (null deref, divide-by-zero, index-out-of-bounds): those are already emitted statically by sema as `Terminator::Trap(TrapKind)` (`cb-ir`), handled entirely host-side, and never touch the runtime. So the two trap mechanisms coexist at different layers and this FD does not change the `Trap` path.

### Model: callback that records intent and returns (never unwinds)

A host callback **cannot** unwind the C frame it was called from, and `setjmp`/`longjmp` is rejected outright — it is `unsafe` (workspace `unsafe_code = "deny"`), skips C++ destructors (Allegro bitmaps, `CbString`), and has no LLVM analog. Instead the runtime *reports intent and returns*; the host records it and drains it through the existing "return a code up the stack" path. The runtime side must always tolerate the callback returning and do something reasonable after.

- **Interpreter (Model A — pending-trap slot):** the host's callback records the request in a backend-owned slot and returns. The interpreter drains the slot immediately after the dispatch in **`call_runtime`** (`crates/cb-backend-interp/src/interp.rs`, the single chokepoint that already returns `Result<Value, InterpError>` and sits under `#[allow(unsafe_code)]`) — *not* in `ffi.rs`, which lacks the `InterpError`/observer vocabulary. The slot is a `thread_local! { Cell<Option<PendingTrap>> }` with `enum PendingTrap { Exit(i32), Error(String) }`; `raise_error` copies the `CbString` bytes into a Rust `String` at the boundary. (Single-threaded today, so `thread_local`+`Cell` is safe with no new lint relaxation.)
- **LLVM (forward-compat, no code yet):** the *same* `CbHostApi` may have a *different* implementation. It emits a pending-flag check after runtime calls, **gated by the `CbFuncDesc.flags` "can-trap" bit** so pure math/trig never pays the cost. A terminating host impl *may* `fflush` + `std::exit(code)` directly, but **must flush stdout and run any `about_to_exit` hook first** so observable behavior matches the interpreter (the reference impl per CLAUDE.md). Non-terminating hooks (`about_to_exit`, a future `poll_cancel`) *require* the return-and-continue model — `std::exit` alone cannot express them.

### Bidirectional init handshake (also the plugin seam)

The original "host-API as a `const` substruct on `CbCatalog`" idea does **not** work: `cb_runtime_string_api` is a `const` global the runtime fills at compile time, but the *host* API is filled by the backend at runtime. So the host API is delivered by a **mutable init handshake**, modeled on SQLite's loadable-extension `pApi` pattern (host passes an API struct by `const` pointer; the runtime stashes it in a file-static) plus a game-engine-style returned hook table (the runtime returns the hooks it wants connected). `CbCatalog`/`CbStringApi` stay `const` and untouched.

```c
#define CB_CATALOG_VERSION 5   /* host-api handshake added */

typedef struct {
    uint32_t size;             /* sizeof(CbHostApi) — ABI guard, caller-set */
    uint32_t abi_version;      /* == CB_CATALOG_VERSION */
    void (*request_exit)(int32_t code);       /* clean exit; host drains → Halt/Ok(code) */
    void (*raise_error)(const CbString* msg); /* fatal runtime error → exit 1 */
    /* grow by appending; readers gate on `size` */
} CbHostApi;

typedef struct {
    uint32_t size;             /* sizeof(CbRuntimeHooks) — callee-set */
    void (*about_to_exit)(void);   /* host calls before shutdown; nullable */
    /* grow by appending */
} CbRuntimeHooks;

/* The handshake. Host passes its API; the runtime stores it in a file-static
   (mirroring cb_gfx.cpp's existing globals) and returns the hooks it wants
   connected (null = not connected). Kept separate from cb_runtime_get_catalog,
   which must stay retrievable as pure data before init runs (string_api()
   already fetches the catalog independently at startup). */
const CbRuntimeHooks* cb_runtime_init(const CbHostApi* host);
```

`cb_rt_*` functions then call `g_host->request_exit(0)` (window close) or `g_host->raise_error(msg)` (IO / out-of-range failures) instead of `exit()`/silent clamping. The `size` + `abi_version` fields give two independent ABI guards (whole-version rejection *and* additive field growth), so a plugin built against a different header revision can coexist.

**Plugins (FD-009 already specs the loader):** each plugin DLL exports `cb_runtime_get_catalog` + `cb_runtime_init`; the driver calls `init(host)` per DLL, stores that DLL's returned hooks, and merges catalogs (collision handling per FD-009). Each DLL keeps its own `g_host` file-static. No per-call host pointer is needed — the runtime stores it once at init, consistent with FD-009's "runtime owns its state, no context parameter" rule.

### Exit vs error return paths

`request_exit(code)` must reach `Ok(code)`, while `raise_error(msg)` must reach `Err` → exit 1, but there is no `InterpErrorKind::Exit` today. Plan: drain to `Err(InterpErrorKind::Exit(c))` / `Err(InterpErrorKind::RuntimeError(m))` (the latter already exists and `Display`s as `runtime error: {msg}` → driver exit 1), and have `run`/`exec_loop` intercept `Exit(c)` and convert it to `Ok(c)` — the same path `Terminator::Halt` already takes. No new IR terminator is needed; the message rides the slot, keeping the IR backend-agnostic.

### Remaining decisions (call out at review)

1. **`Exit` delivery:** new `InterpErrorKind::Exit(i32)` intercepted by `run` (reuses `?` propagation) vs. an interpreter-side flag checked by `exec_loop`. Leaning toward the variant.
2. **Observer fidelity:** `Observer::on_trap` takes a closed `TrapKind` with **no message variant**, so a runtime `raise_error` can't currently flow through it. Add a new `on_runtime_error(&str)` hook, widen `on_trap`, or accept that runtime errors skip the observer for now.
3. **Out-of-range policy:** per-function, decide clamp-vs-`raise_error` (the legacy `error()`ed; clamping was a stopgap pending this channel).
4. **LLVM stderr parity:** pin the exact message text/stream so a future `std::exit`-style C path matches the interpreter's `CliRenderer` output (the most likely place parity silently breaks).

After landing, retire the three warts: window-close → `request_exit`; out-of-range clamp → `raise_error` (per decision 3); optionally restore ESC safe-exit as an opt-in via `request_exit`.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_runtime.h` | MODIFY | Add `CbHostApi` + `CbRuntimeHooks` structs (each with `size`/`abi_version` guards) and the `cb_runtime_init(const CbHostApi*) -> const CbRuntimeHooks*` entry point; bump `CB_CATALOG_VERSION` 4→5. `CbCatalog`/`CbStringApi` stay `const` and unchanged. |
| `runtime/catalog.cpp` (or a new `cb_host.cpp`) | MODIFY/CREATE | Implement `cb_runtime_init`: stash `host` in a file-static `g_host`, return the static `CbRuntimeHooks`. Provide the runtime-side accessor `cb_rt_*` functions call. |
| `runtime/cb_gfx.cpp` | MODIFY | Replace window-close `exit(0)` with `g_host->request_exit(0)`; register an `about_to_exit` hook for display teardown. |
| `runtime/cb_string.cpp` | MODIFY | Per decision 3, replace out-of-range clamps with `raise_error` where the legacy errored. |
| `runtime/cb_input.cpp` | MODIFY (optional) | Optionally restore ESC safe-exit via `request_exit`. |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | Mirror `CbHostApi`/`CbRuntimeHooks` (`#[repr(C)]`); declare `cb_runtime_init`; assert the new catalog version. |
| `crates/cb-backend-interp/src/interp.rs` | MODIFY | Add the `thread_local! Cell<Option<PendingTrap>>` slot + `extern "C"` `request_exit`/`raise_error` trampolines (under the existing `#[allow(unsafe_code)]`); call `cb_runtime_init` in `Interpreter::new`; drain the slot in **`call_runtime`** right after `ffi::call`. Convert `Exit(c)` → `Ok(c)` in `run`/`exec_loop`. |
| `crates/cb-backend-interp/src/error.rs` | MODIFY | Add `InterpErrorKind::Exit(i32)` (clean exit with code); `RuntimeError(String)` already exists for `raise_error`. |
| `crates/cb-backend-interp/src/observer.rs` | MODIFY (decision 2) | Add `on_runtime_error(&str)` (or widen `on_trap`) so runtime-raised errors are observable — `TrapKind` has no message variant. |
| `crates/cb-ir/src/*` | NONE expected | No new terminator: `request_exit`→`Halt`-equivalent `Ok(code)`, message rides the slot. Keeps IR backend-agnostic. |
| `crates/cb-backend-llvm/*` | NOTE | Document the mapping (no code yet): same `CbHostApi`, `flags`-gated post-call check, flush+`about_to_exit` before any `std::exit`. |
| `crates/cb-driver/tests/*` | CREATE | Fixture(s): a runtime error surfaces with the right exit code/message; window-close path (manual). |

## Verification

- `cargo test --workspace` green.
- New `cb-driver` fixture: a runtime function that raises an error exits with the expected non-zero code and writes the message to stderr (compare to FD-013 Batch 3's `MakeError`/`End` cli tests).
- `cb-runtime-sys` catalog test asserts the new ABI version (v5) and that `cb_runtime_init` round-trips the host API + returns the hook table.
- Interpreter observer fires for a runtime-raised error (unit test) — via the new `on_runtime_error` hook (or widened `on_trap`) per decision 2.
- Manual: closing the window of `examples/bounce.cb` / `examples/input_demo.cb` now terminates via the clean `Halt`/`Ok(0)` path (and runs `about_to_exit`), not `exit(0)`.
- Cross-backend note: once LLVM codegen exists, the same fixture must produce identical behavior (interp is the reference per CLAUDE.md).

## Related

- [FD-013](FD-013_EXTENDING_RUNTIME_SUPPORT.md) — accumulates the three warts this FD resolves (Batch 3 `Halt`, Batch 4 window-close `exit(0)`, Batch 5 ESC pure-query); the File I/O batch depends on this.
- [FD-012](archive/FD-012_CATALOG_CPP_TEMPLATE_DSL.md) — libffi dispatch; the interpreter's pending-trap drain lands in `call_runtime` at this seam.
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) — already specs the plugin DLL loader (`--plugin` + `libloading`, per-DLL `cb_runtime_get_catalog`, driver catalog merge, the C header as the plugin ABI); `cb_runtime_init` extends that contract.
- [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) / [FD-014](archive/FD-014_RUNTIME_STRING_ABI.md) — `CbStringApi` is the precedent for a versioned function-pointer table on `CbCatalog`; note it is a `const` global (runtime→backend), which is *why* the host API needs the mutable `cb_runtime_init` handshake instead of a catalog substruct. Modeled on SQLite's loadable-extension `pApi` handshake.
- [FD-010](archive/FD-010_INTERPRETER_BACKEND.md) — `Observer` hooks and `Result<i32, _>` exit-code plumbing (the receiving half); `Terminator::Halt`/`Trap` distinction (compiler faults vs. this cooperative channel).
- `docs/cb_runtime.md` — termination semantics notes (System/Time, graphics window-close wart).
