# FD-033: Catalog Mock for SDK-Free Testing

**Status:** Pending Verification
**Priority:** High
**Effort:** Medium-High (2-6 hours)
**Impact:** `cargo test --workspace` passes on any machine with a Rust toolchain — no CMake/Allegro/vcpkg — bringing the interpreter's tests and the driver's fixtures into every CI run and cloud session.

> **Resolved at implementation time (2026-06-16):** Implemented as **option (b)** —
> carve the Allegro-free runtime out of the CMake/Allegro build rather than
> writing a pure-Rust mock — and **not** a separate mock catalog. The decode path
> was already pure-Rust testable; the real blockers were (1) `build.rs`
> hard-failing without CMake/Allegro and (2) integration tests needing a working
> catalog. Both are solved by compiling the Allegro-free TUs (+ `catalog.cpp`
> with `-DCB_NO_ALLEGRO`) via the `cc` crate. **Key discovery:** building "just
> `cb_string.cpp`" is insufficient — `catalog.cpp`'s `CB_FN` macro takes the
> address of every `cb_rt_*` symbol, so the catalog assembly itself is what drags
> in Allegro. The fix is a single `#ifndef CB_NO_ALLEGRO` guard around exactly the
> graphics/text/input `CB_FN` rows (one source of truth, no second catalog). Build
> path is **auto-detected** with env overrides. See the Solution and Verification
> sections below for the as-built design.

## Problem

All `cb-backend-interp` integration tests and all `cb-driver` tests funnel through `cb_runtime_sys::load_catalog()`, whose `build.rs` compiles the C++ runtime via CMake and **fails outright without the Allegro SDK** (`build.rs:56 "cmake configure failed"`). Verified 2026-06-09 in a Linux container: `cargo test --workspace` cannot even build; only 499 of ~595 tests are runnable. Consequences:

- The interpreter — the *reference implementation* — has zero executable coverage in any environment without a full C++ graphics toolchain.
- Coverage numbers lie: `lower.rs` measures 54% on host-only tests partly because the driver fixtures that exercise it can't run.
- Most of what those tests check (arithmetic, control flow, structs, traps, observer) doesn't touch graphics at all; the SDK requirement is incidental, via the monolithic catalog.

## Solution (as built)

Build the **Allegro-free slice of the real runtime** via the `cc` crate when the
full CMake/Allegro toolchain is unavailable, so the interpreter runs language-core
programs against a genuine `cb_runtime_get_catalog` — same string implementation,
no Rust mock.

**1. Catalog partition (`runtime/catalog.cpp`).** The catalog assembly is the only
thing that link-resolves every `cb_rt_*` symbol (`CB_FN` takes each function's
address), so it — not `cb_string.cpp` alone — is what pulls in Allegro. A single
`#ifndef CB_NO_ALLEGRO` guard wraps exactly the graphics/text/input `CB_FN` rows
and the `Image`/`Font` type entries. With the define set, the catalog references
only the Allegro-free symbols; one source of truth, no divergent second catalog.

The Allegro-free TU set (verified): `cb_string.cpp`, `cb_host.cpp`, `cb_math.cpp`,
`cb_strfuncs.cpp`, `cb_system.cpp` (its only "Allegro" hits are a comment and the
substring in `local_now`). Graphics/input/font (`cb_gfx.cpp`, `cb_input.cpp`,
`cb_font.cpp`) are the genuine Allegro consumers, excluded from the SDK-free build.

**2. `build.rs` path selection (auto-detect + env override).**

| Situation | Path |
|-----------|------|
| `cmake` present and configures/builds cleanly | full Allegro build (CMake) |
| `cmake` absent, or configure/build fails | SDK-free `cc` build (fallback, emits `cargo:warning`) |
| `CB_RUNTIME_FORCE_SDK_FREE=1` | SDK-free `cc` build, no probing |
| `CB_RUNTIME_REQUIRE_ALLEGRO=1` | full build, **fatal** if it fails |

The full build is refactored into `build_full(...) -> Result` that emits no
`cargo:` directives until every fallible step succeeds, so a failed probe leaves
no half-applied link state before the fallback runs. SDK-free path emits
`--cfg cb_no_allegro`.

**3. Test gating.** `cb-runtime-sys` exposes `pub const HAS_GRAPHICS = cfg!(not(cb_no_allegro))`.
The interpreter integration tests are all language-core and need **no** gating.
The `cb-runtime-sys` catalog unit test guards its graphics assertions behind
`#[cfg(cb_no_allegro)]`. The driver's 7 graphics/input fixtures (incl.
`runtime_constants_fd029`, which calls `KeyDown`) route through a `run_graphics`
helper that skips when `!HAS_GRAPHICS`.

## Files to Create/Modify (as built)

| File | Action | Purpose |
|------|--------|---------|
| `runtime/catalog.cpp` | MODIFY | `#ifndef CB_NO_ALLEGRO` guard around graphics/text/input `CB_FN` rows + `Image`/`Font` type entries |
| `crates/cb-runtime-sys/build.rs` | MODIFY | Auto-detect + env-override path selection; `cc`-based SDK-free build of the Allegro-free TUs; emit `cb_no_allegro` cfg |
| `crates/cb-runtime-sys/Cargo.toml` | MODIFY | Add `cc` build-dependency |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | `pub const HAS_GRAPHICS`; gate the catalog unit test's graphics assertions behind `#[cfg(cb_no_allegro)]` |
| `crates/cb-driver/tests/programs.rs` | MODIFY | `run_graphics` helper; route the 7 graphics/input fixtures through it |
| `docs/cb_runtime.md` | MODIFY | Document the SDK-free build, the env vars, and `HAS_GRAPHICS` |

No `mock_catalog.rs` was created (option (b) builds the real catalog, so a synthetic one is unnecessary). `cb-driver/Cargo.toml` needed no change either: `cb-runtime-sys` is already a normal dependency, and integration tests can read `HAS_GRAPHICS` through it — no dev-dependency required.

## Verification

Both build paths exercised on Windows 11 + MSVC (2026-06-16):

- **SDK-free path** (`CB_RUNTIME_FORCE_SDK_FREE=1 cargo test --workspace`): builds via `cc` and passes — `cb-runtime-sys` 19/19 (incl. the real string-primitive roundtrip and `runtime_init` handshake against the `cc`-compiled core), all interpreter integration tests, and the driver language-core fixtures. The 7 graphics/input fixtures skip via `run_graphics`. No failures.
- **Full path** (default `cargo test --workspace`): unchanged — CMake/Allegro build, complete catalog, all graphics fixtures run. No failures, no regressions.
- `cargo clippy --workspace --all-targets`: exit 0, no lint warnings introduced (the per-crate "1 warning" lines are the Windows incremental-compilation "Access is denied" finalize quirk, not code).

Outstanding for `/fd-verify`: confirm on a genuinely SDK-less machine (Linux container) that `cargo test --workspace` builds with no CMake/vcpkg present; record the `cargo llvm-cov` `lower.rs`/`interp.rs` baseline now that interp+driver tests run in coverage.

## Related

- FD-014 (string ABI — the 6 primitives a mock must satisfy), FD-016 (core/functionality split — the seam option (b) builds on), FD-024 (FFI ABI hardening — coordinate, it touches the same catalog decode path)
- FD-030/FD-032 depend on this for CI-visible coverage
- Coverage analysis session, 2026-06-09
