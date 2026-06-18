# FD-025: Driver CLI, Backend-Selection & Exit-Code Correctness

**Status:** Pending Verification
**Priority:** Medium
**Effort:** Low–Medium
**Impact:** Stops `--backend llvm` from silently succeeding while doing nothing, pins exit-code truncation behavior, replaces the hand-rolled argument loop with a proper `clap` parser (giving `--help`/`--version`), makes the no-backend "dump-only" build actually usable, and closes the driver's untested flag/error combinations.

> **Scope note (expanded):** The original FD covered the LLVM no-op, the exit-code cast, and untested branches. During implementation the scope was widened (per request) to include **proper command-line parsing**: the driver now uses `clap` (derive) for parsing and a real `--help`/`--version`. Surfacing every branch under test also revealed that backend selection ran *before* dumping, so the advertised `--no-default-features` "dump-only" binary actually errored with "no backend compiled in" — now fixed by resolving the backend lazily.

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

3. **CLI parsing was hand-rolled with no `--help`.** The driver matched args in a `while` loop and printed a one-line `usage:` string only on error. There was no `--help`/`-h` or `--version`/`-V`; discovering the flags meant reading the source.
4. **The no-backend "dump-only" build was broken.** `CLAUDE.md` advertises `--no-default-features` as "a no-backend dump-only binary suitable for AST inspection," but backend selection ran *before* reading/parsing/dumping. With no backend compiled in, `default_backend()` returned `None` and the driver exited 2 ("no backend compiled in") for *every* invocation — including `--dump-ast` — so AST inspection was impossible in that build. (Latent because the driver test suite was only ever run with the default `interp` feature.)

## Solution

In `cb-driver`:

- **CLI parsing → `clap` (derive).** Replace the hand-rolled `while` loop with a `#[derive(Parser)]` `Cli` struct. This gives `--help`/`-h` and `--version`/`-V` for free, accepts both `--backend name` and `--backend=name`, and reports usage errors (unknown flag, missing value, missing `<FILE>`) with exit code 2 — matching the driver's existing usage-error code. `--backend` stays an `Option<String>` validated by the existing feature-gated `parse_backend`, so the "not compiled in" / "unknown backend" diagnostics are preserved.
- **Explicit `Backend::Llvm` arm.** Emit a clear "llvm backend not yet implemented" message and return a distinct exit code (**3**), rather than silently succeeding. Dispatch is a single `match backend { … }` over `Option<Backend>`; once `cb-backend-llvm` codegen exists it gets wired in here.
- **Lazy backend resolution.** Resolve the backend to `Option<Backend>` but only *require* one at the point a program would actually run. `--dump-ast`/`--dump-ir` and error paths no longer need a backend, so the `--no-default-features` dump-only build works as advertised. An explicitly named bad/unavailable backend still fails fast.
- **Exit-code policy.** `clamp_exit(code) = code.clamp(0, 255) as u8`, documented: values >255 saturate to 255 (stay non-zero/failure) instead of the old `as u8` wrap that turned 256→0; negatives clamp to 0. Pinned with tests (256→255, −1→0).
- **Centralised exit codes** in an `exit` module (`USAGE = 2`, `BACKEND_UNIMPLEMENTED = 3`) with the full contract documented (0 success / 1 compile-or-runtime error / 2 usage / 3 unimplemented backend).
- **Backfill `assert_cmd` tests** for every branch, gated so the suite runs green under all four feature combos (`interp`, `interp`+`llvm`, none, `llvm`-only). Program-executing tests are gated on `feature = "interp"`; `programs.rs` is gated whole-file.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `Cargo.toml` (workspace) | MODIFY | Add `clap = { version = "4", features = ["derive"] }` to `[workspace.dependencies]` |
| `crates/cb-driver/Cargo.toml` | MODIFY | Depend on `clap.workspace = true` |
| `crates/cb-driver/src/main.rs` | MODIFY | `clap` `Cli` struct; lazy `Option<Backend>` resolution; single dispatch `match` with explicit `Llvm` (exit 3) and `None` (exit 2) arms; `clamp_exit`; `exit` code module |
| `crates/cb-driver/tests/cli.rs` | MODIFY | `--help`/`-h`/`--version`; unknown flag; `--backend=interp` and empty `--backend=`; `--dump-ir`/`--dump-ast` on erroring input; out-of-range `request_exit` clamp (256→255, −1→0); llvm-not-implemented (gated); no-backend error (gated); existing run-tests gated on `interp` |
| `crates/cb-driver/tests/programs.rs` | MODIFY | Whole-file `#![cfg(feature = "interp")]` (every fixture runs a program) |

## Verification

- `cargo test -p cb-driver` green across all four feature combos: default `interp` (28 cli + 28 programs), `--features llvm` (28+28), `--no-default-features` (20 cli, programs empty), `--no-default-features --features llvm` (19 cli, programs empty). ✅
- `cargo test --workspace` green (28 test binaries, 0 failures). ✅
- `cargo clippy` with `-D warnings` clean on the workspace and on `cb-driver` across all four feature combos; `cargo fmt --all` clean. ✅
- `cb --help` / `cb --version` print usage / version and exit 0; `cb --backend llvm <file>` (llvm build) exits 3 with "not yet implemented" (not silent exit 0); `cb --dump-ast <file>` works under `--no-default-features`.

## Implementation Notes

- **clap displays the binary basename in usage** (`Usage: cb.exe …` on Windows) because it derives `bin_name` from `argv[0]`; the version line uses the configured `name` (`cb 0.1.0`). Tests assert on `Usage: cb` (prefix-matches both) and `CARGO_PKG_VERSION`.
- The exit-code clamp's negative→0 choice is deliberately pinned by a test so it's an intentional policy, not an accident; revisit if CoolBasic ever ascribes meaning to negative `request_exit` codes.

## Related

- Surfaced by the post-FD-018 codebase review (driver area).
- [FD-006](archive/FD-006_DIAGNOSTICS_DRIVER_HARDENING.md) — feature-gated backend selection and `--backend` validation this extends.
- [FD-015](archive/FD-015_RUNTIME_TRAP_CHANNEL.md) — `request_exit(code)` whose value the exit-code clamp governs.
- `CLAUDE.md` — backend pluggability rules (`interp` default, `llvm` opt-in).
