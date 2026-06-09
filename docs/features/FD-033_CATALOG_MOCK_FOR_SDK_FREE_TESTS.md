# FD-033: Catalog Mock for SDK-Free Testing

**Status:** Open
**Priority:** High
**Effort:** Medium-High (2-6 hours)
**Impact:** `cargo test --workspace` passes on any machine with a Rust toolchain — no CMake/Allegro/vcpkg — bringing the interpreter's 47 tests and the driver's 46 tests into every CI run and cloud session.

## Problem

All `cb-backend-interp` integration tests and all `cb-driver` tests funnel through `cb_runtime_sys::load_catalog()`, whose `build.rs` compiles the C++ runtime via CMake and **fails outright without the Allegro SDK** (`build.rs:56 "cmake configure failed"`). Verified 2026-06-09 in a Linux container: `cargo test --workspace` cannot even build; only 499 of ~595 tests are runnable. Consequences:

- The interpreter — the *reference implementation* — has zero executable coverage in any environment without a full C++ graphics toolchain.
- Coverage numbers lie: `lower.rs` measures 54% on host-only tests partly because the driver fixtures that exercise it can't run.
- Most of what those tests check (arithmetic, control flow, structs, traps, observer) doesn't touch graphics at all; the SDK requirement is incidental, via the monolithic catalog.

## Solution

Provide a **synthetic catalog built in pure Rust** for tests, so the interpreter can run language-core programs without the C++ runtime.

Sketch (details to settle at implementation time):

- The interpreter already consumes the backend-agnostic `cb_ir::RuntimeCatalog` (`FuncDesc` with `fn_ptr`, `RuntimeTypeDesc`, `RuntimeConstDesc`). A test-support constructor — e.g. `cb-backend-interp/tests/common/mock_catalog.rs` or a `test-util` feature — can build one from Rust `extern "C"` functions (`unsafe_code = "deny"` is workspace-wide; `extern "C"` *definitions* are safe, but check whether the `fn_ptr` plumbing and libffi dispatch path need the real C ABI — if libffi dispatch is the obstacle, an alternative is an interpreter-level intrinsic override table like the existing `cb_rt_print` test capture).
- Minimum surface: `Print` (capture for assertions), `Str`/`Int`/`Float` conversions, `Abs` — enough for every non-graphics fixture.
- **String ABI is the hard part:** `CbString*` primitives (`retain`/`release`/`from_literal`/`len`/`data`/`concat`, FD-014) live in `cb_string.cpp`. Options: (a) reimplement the 6 primitives in Rust behind the same `CbStringApi` struct for tests (the layout is simple: refcounted single block); (b) carve `cb_runtime_core` (already Allegro-free per FD-016) out of the CMake/Allegro build so `cc` can compile just `cb_string.cpp` — making `cb-runtime-sys` build everywhere and only the *functionality* lib require Allegro. Option (b) is likely less code and keeps one string implementation; it also benefits non-test consumers.
- Gate: tests that genuinely need graphics/input keep requiring the real catalog behind `#[cfg]` or an env check with a clear skip message; everything else uses the mock.
- Driver fixtures: once `cb-runtime-sys` builds SDK-free (option b), `cb-driver` non-graphics fixtures run too; graphics fixtures skip cleanly.

Decision needed at implementation time (ask if unclear): option (a) Rust mock vs. option (b) splitting the core C library out of the Allegro CMake build. Both are sketched above; (b) is recommended.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-runtime-sys/build.rs` | MODIFY | Build Allegro-free core separately (option b), or feature-gate the CMake build |
| `crates/cb-backend-interp/tests/common/mock_catalog.rs` | CREATE | Synthetic catalog / intrinsic overrides for language-core tests |
| `crates/cb-backend-interp/tests/integration.rs` | MODIFY | Switch non-graphics tests to the mock; gate the rest |
| `crates/cb-driver/tests/programs.rs` | MODIFY | Skip-with-message for graphics fixtures when SDK absent |

## Verification

- On a machine **without** Allegro/CMake: `cargo test --workspace` builds and passes (graphics-dependent tests skipped with a visible message, not silently).
- On a full machine: `cargo test --workspace` unchanged — same test count or higher, no behavior change to the real catalog path.
- `cargo llvm-cov` workspace run now includes interp + driver tests; record the new `lower.rs`/`interp.rs` baseline in the FD on completion.

## Related

- FD-014 (string ABI — the 6 primitives a mock must satisfy), FD-016 (core/functionality split — the seam option (b) builds on), FD-024 (FFI ABI hardening — coordinate, it touches the same catalog decode path)
- FD-030/FD-032 depend on this for CI-visible coverage
- Coverage analysis session, 2026-06-09
