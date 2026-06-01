# FD-025: Driver Backend-Selection & Exit-Code Correctness

**Status:** Open
**Priority:** Medium
**Effort:** Low (< 1 hour)
**Impact:** Stops `--backend llvm` from silently succeeding while doing nothing, pins exit-code truncation behavior, and closes the driver's untested flag/error combinations.

## Problem

The post-FD-018 review found `cb-driver` small, readable, and well-covered on the interp happy path — but every gap is on the `llvm` branch and the dump/error combinations.

1. **Selecting the LLVM backend is a silent no-op that exits 0.** The program-execution block is gated `#[cfg(feature = "interp")]` *and* `matches!(_backend, Backend::Interp)` (`main.rs:204-217`). Built with `--features llvm`, `cb --backend llvm valid.cb` (no dump flags) parses, lowers and verifies IR, then runs **no backend** and falls through to return `ExitCode::SUCCESS`. The user gets exit 0 with no output and no indication the requested backend did nothing. There is no LLVM execution path and no "not yet implemented" diagnostic anywhere. Holds in both the default (`llvm`+`interp`) and `--no-default-features --features llvm` builds.
2. **Interpreter exit code truncated with a lossy `as u8` cast.** `interpret` returns `i32`; the driver does `ExitCode::from(code as u8)` (`main.rs:211`). `request_exit(256)` becomes exit 0; `request_exit(-1)` becomes 255. Some clamping is unavoidable (OS codes are 0–255) but the silent wrap can mask the requested value (the tested case, 7, happens to round-trip).

Untested combinations the review flagged (regression risk):

- `--backend llvm` with llvm compiled in — the silent no-op above (`main.rs:204-218`).
- `--no-default-features` "no backend compiled in" error path — the `None` branch of `default_backend()` (`main.rs:124-132`).
- `--dump-ir`/`--dump-ast` on an *erroring* program: IR lowering is skipped behind `if !had_error` (`main.rs:193`), so `--dump-ir bad.cb` prints diagnostics and exits 1 with empty stdout, while `--dump-ast` (runs before the error gate, `:182`) still emits — an easy-to-regress asymmetry.
- `load_catalog` failure path (`main.rs:155-161`) — exit 2 on catalog load error.
- The `--backend=name` equals-form (`main.rs:100-102`) and a malformed empty `--backend=` — only the space-separated form is tested.
- Unknown flag / extra positional → exit 2 (`main.rs:104-107`).

## Solution

In `cb-driver`:

- Add an explicit `Backend::Llvm` arm that emits a clear "llvm backend not yet implemented" message and returns a distinct non-zero exit code, rather than silently succeeding. Once `cb-backend-llvm` codegen exists, wire it here and drop the `_backend` underscore (dispatch on `backend` in one match).
- Decide exit-code policy: clamp explicitly (`code.clamp(0, 255) as u8`) with a documented rationale, or surface out-of-range `request_exit` values as a diagnostic. Pin it with a test so the behavior is intentional.
- Backfill `assert_cmd` tests for the untested branches above.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-driver/src/main.rs` | MODIFY | Explicit `Backend::Llvm` "not implemented" arm + exit code; documented exit-code clamp |
| `crates/cb-driver/tests/cli.rs` | MODIFY | Tests: `--backend llvm` (llvm build) errors clearly; `--backend=interp` and empty `--backend=`; `--dump-ir`/`--dump-ast` on an erroring input; no-backend build error; out-of-range `request_exit` clamp |

## Verification

- `cargo test -p cb-driver` green, with new tests covering each branch.
- `cargo build --features llvm && cargo run -p cb-driver --features llvm -- --backend llvm examples/bounce.cb` exits non-zero with a clear message (not silent exit 0).
- `cargo test --workspace` + `clippy -- -D warnings` green.

## Related

- Surfaced by the post-FD-018 codebase review (driver area).
- [FD-006](archive/FD-006_DIAGNOSTICS_DRIVER_HARDENING.md) — feature-gated backend selection and `--backend` validation this extends.
- [FD-015](archive/FD-015_RUNTIME_TRAP_CHANNEL.md) — `request_exit(code)` whose value the exit-code clamp governs.
- `CLAUDE.md` — backend pluggability rules (`interp` default, `llvm` opt-in).
