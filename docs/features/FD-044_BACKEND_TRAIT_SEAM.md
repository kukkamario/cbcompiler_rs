# FD-044: Backend Trait Seam

**Status:** In Progress
**Created:** 2026-06-24
**Priority:** Medium (blocking for LLVM codegen)
**Effort:** Medium (a load-bearing structural decision — trait shape, crate placement)
**Impact:** Materializes the "pluggable backend" architecture the project committed to up front, so adding the LLVM backend is implementing a trait rather than threading new `cfg`'d `match` arms through the driver.

> Surfaced by the Bundle 6 code-review cleanup (finding **DR-R10**). This is a structural/architectural decision (CLAUDE.md flags backend-trait shape as one to decide with the user, not unilaterally), so it is filed for a dedicated design pass rather than folded into a cleanup bundle. **All open decisions (D1–D5) are confirmed (user, 2026-06-24) and the executable-topology question is settled; the design is ready for implementation.**

## Problem

CLAUDE.md states the backend is pluggable: "The frontend … must not depend on any specific backend. A backend should be selectable at compile time (cargo features) and ideally at runtime (CLI flag)." Today there is **no materialized `Backend` trait**. The driver dispatches each backend through hand-written, `cfg`-gated `match` arms (`crates/cb-driver/src/main.rs:284-319`): `Some(Backend::Interp) => cb_backend_interp::interpret(...)`, `#[cfg(feature = "llvm")] Some(Backend::Llvm) => <prints "not yet implemented", exits 3>`. `cb-backend-llvm` is a 3-line doc-comment stub (`crates/cb-backend-llvm/src/lib.rs`).

This works for one real backend, but every backend-specific concern (how a program is run, the success/trap result shape, where the exit code comes from) lives inline in the driver's run block. When LLVM codegen begins it will accrete a *second* parallel set of driver arms — and, unlike interp, it "produces an artifact" rather than "runs and returns an exit code," so it does not even fit the current arm's shape. The "selectable at runtime" goal has no interface to hang on: there is no type that says "a backend is a thing that can take an IR `Program` and do something with it."

## Open decisions (all confirmed — user, 2026-06-24)

CLAUDE.md: *"For load-bearing structural choices (backend trait shape, …) prefer asking the user over picking unilaterally."* Each row gives the confirmed recommendation and the alternative(s) considered. **D1–D5 are all signed off; the recommended column is the decision.**

| # | Decision | Recommendation | Alternatives / trade-off |
|---|----------|----------------|--------------------------|
| D1 | **Where does the trait live?** | **✓ Confirmed (user, 2026-06-24):** new `cb-backend-api` crate (deps: `cb-ir`, `cb-diagnostics`). Both backends + driver depend on it. | (a) A `backend` module in `cb-ir` — lighter (no new crate) but mixes "execute a program / exit codes" execution semantics into the pure-data IR crate, which CLAUDE.md asks to keep clean. (b) `cb-driver` — **rejected**: backends already depend on the driver's *consumers*, so a trait there would be a dependency cycle and defeats pluggability. |
| D2 | **Trait surface — how to fit "run" (interp) and "produce an artifact" (LLVM AOT)?** | **✓ Confirmed (user, 2026-06-24):** one object-safe method returning an outcome enum: `execute(&self, &Program, &Interner) -> Result<BackendOutcome, BackendError>`, where `BackendOutcome` is `Ran { exit_code: i32 }` *or* `Produced { artifact: PathBuf }`. Minimal, accommodates both, lets the driver's call site stay backend-agnostic. | Capability split — separate `Run` and `Compile` traits a backend implements one or both of. More honest if/when `cb run` vs `cb build` become distinct commands, but more surface than needed while interp is the only real backend. Can be refactored to this later behind the same crate seam. |
| D3 | **Dispatch mechanism in the driver.** | **✓ Confirmed (user, 2026-06-24):** `Box<dyn Backend>` resolved once by a feature-gated factory; the run site calls `backend.execute(...)` with no `match`. | An `enum Backend { Interp, Llvm }` with a dispatch `match` — keeps it monomorphic but reintroduces the per-backend arms this FD exists to remove. |
| D4 | **Error type.** | **✓ Confirmed (user, 2026-06-24):** `BackendError { kind: BackendErrorKind, message: String }` with `kind ∈ { Unimplemented, Failed }`. The driver maps `Unimplemented → exit 3`, `Failed → exit 1`, preserving the FD-025 contract. Can later carry a `Diagnostic` instead of a bare message. | Return `Box<dyn std::error::Error>` — simpler but loses the kind distinction the exit-code contract needs. |
| D5 | **`BackendOptions` param now or later?** | **✓ Confirmed (user, 2026-06-24): defer.** interp needs none; add an `opts: &BackendOptions` argument when LLVM AOT first needs output-path / opt-level / target-triple. Only one impl exists today, so the future signature change is trivial. | Add an (initially empty) options struct now to freeze the signature — avoids a later break at the cost of an empty-struct smell. |

## Executable topology (decided via `/fd-deep`, 2026-06-24)

A four-angle deep analysis weighed splitting the project into **two executables** (a native/AOT compiler and a compiler+interpreter) versus keeping one `cb` binary. **Decision: keep a single `cb` binary now; do not split.** Rationale, all evidence-verified:

1. **A split cannot quarantine the heavy dependency.** The C++/Allegro runtime (`cb-runtime-sys`) is a **semantic-analysis** dependency, not a backend concern: `cb-driver` depends on it unconditionally (`crates/cb-driver/Cargo.toml:21`) and the catalog is fetched from the *linked* runtime at runtime — `load_catalog()` → `cb_runtime_get_catalog()` (`crates/cb-runtime-sys/src/lib.rs:165-196,266`), feeding `cb_sema::analyze(...)`. *Both* a native compiler and an interpreter must run sema, so both link the runtime. Splitting executables along native-vs-interp does not remove it from either.
2. **Cargo features already isolate the one thing a split would (LLVM).** With `llvm` behind `dep:cb-backend-llvm`, an interp-only `cb` never pulls LLVM; an `llvm`-only build never compiles `libffi`/the interpreter. The runtime-selectable `--backend` goal (CLAUDE.md) is satisfied today.
3. **`BackendOutcome::Produced` already absorbs the AOT-vs-run asymmetry inside one binary** — the very thing two binaries would express. (This validates D2 above.)
4. **YAGNI.** `cb-backend-llvm` is a 3-line stub; splitting now freezes a packaging/CLI decision around code that doesn't exist.

**Structured for a future split (in scope for this FD).** Promote `cb-driver` to a thin `main.rs` over a `lib.rs` core (the compile pipeline — read → tokenize → parse → sema → lower → verify → dumps — is already `#[cfg]`-free, so extraction is mechanical). This gives the fat-lib/thin-bin shape (the rustc+miri / Lua `lua`+`luac` precedent) **without committing to two binaries**, so a later split is "add a bin crate + one `make_backend` line," not a rewrite.

**If a split is ever warranted** (once LLVM codegen exists and distribution isolation is actually wanted): use separate **bin crates**, not two `[[bin]]` in one crate (cargo feature unification forces this), and cut **compiler-vs-runner** (`cb` emits an artifact / `cb-run` interprets), *not* native-vs-interp — that axis matches where the runtime-execution asymmetry actually lies (libffi + the `runtime_init` host handshake are interp-only). Note CLAUDE.md makes interp the reference implementation for cross-checking LLVM miscompiles, so the debug tool likely wants *both* backends anyway — another reason to keep `--backend` selection in one driver.

**The real prerequisite for a runtime-free native compiler is not the binary split** but decoupling catalog *metadata* (symbol + signature, no `fn_ptr`) from the executable runtime — tracked as **[[FD-045]]**. Until that lands, any "native compiler" still links the full C++ runtime just to type-check.

## Proposed design (recommended)

### 1. New crate: `cb-backend-api`

A small, always-compiled crate holding the cross-backend contract. Depends only on `cb-ir` and `cb-diagnostics` (for `Program` / `Interner`). Contains **no** process/exit-code policy and **no** backend-specific types — those stay in the driver and the concrete backends respectively.

```rust
//! Backend-agnostic contract shared by every CoolBasic backend.
//! Holds no execution policy of its own — the driver maps outcomes to OS
//! exit codes (see FD-025); each backend owns its internal machinery.

use std::path::PathBuf;
use cb_diagnostics::Interner;
use cb_ir::Program;

/// A pluggable code-generation/execution backend for lowered CoolBasic IR.
/// Object-safe: the driver holds a `Box<dyn Backend>`.
pub trait Backend {
    /// Stable backend identifier, matching the `--backend <name>` value.
    fn name(&self) -> &'static str;

    /// Consume the lowered IR and either run it or emit an artifact.
    fn execute(
        &self,
        program: &Program,
        interner: &Interner,
    ) -> Result<BackendOutcome, BackendError>;
}

/// What a backend did with the program.
pub enum BackendOutcome {
    /// The program executed to completion (interpreter, or a future JIT).
    /// `exit_code` is the program's own code (`End` → 0, `MakeError` → 1,
    /// `request_exit(n)` → n); the driver clamps it to an OS code.
    Ran { exit_code: i32 },
    /// The backend produced a build artifact rather than running (AOT).
    Produced { artifact: PathBuf },
}

/// A backend-side failure. The `kind` drives the driver's exit code.
pub struct BackendError {
    pub kind: BackendErrorKind,
    pub message: String,
}

pub enum BackendErrorKind {
    /// Recognised backend with no codegen yet (today: llvm) → driver exit 3.
    Unimplemented,
    /// A genuine trap / internal error while running or compiling → exit 1.
    Failed,
}
```

### 2. `cb-backend-interp` implements `Backend`

Add a unit struct that wraps the existing entry point — no interpreter logic moves or changes.

```rust
pub struct InterpBackend;

impl cb_backend_api::Backend for InterpBackend {
    fn name(&self) -> &'static str { "interp" }

    fn execute(&self, program: &Program, interner: &Interner)
        -> Result<BackendOutcome, BackendError>
    {
        match interpret(program, interner) {          // existing fn, unchanged
            Ok(code) => Ok(BackendOutcome::Ran { exit_code: code }),
            Err(e)   => Err(BackendError {
                kind: BackendErrorKind::Failed,
                message: e.to_string(),
            }),
        }
    }
}
```

The observer/debuggability machinery (`Observer`, `with_observer`) stays **internal** to `cb-backend-interp` — it is interp-specific and deliberately not part of the cross-backend trait, so the LLVM backend incurs no debug-hook obligations. (`interpret()` itself remains for direct/test use.)

### 3. `cb-backend-llvm` implements `Backend` (stub)

The stub gains a type that satisfies the trait and preserves today's "exit 3, not silent" behavior, so the LLVM crate compiles against the seam *now* and codegen fills in `execute` later.

```rust
pub struct LlvmBackend;

impl cb_backend_api::Backend for LlvmBackend {
    fn name(&self) -> &'static str { "llvm" }

    fn execute(&self, _program: &Program, _interner: &Interner)
        -> Result<BackendOutcome, BackendError>
    {
        Err(BackendError {
            kind: BackendErrorKind::Unimplemented,
            message: "the llvm backend is not yet implemented; \
                      run with --backend interp to execute programs".into(),
        })
    }
}
```

### 4. Driver changes (`cb-driver/src/main.rs`)

- Add `cb-backend-api` as an unconditional dependency.
- Keep `--backend` name validation (`parse_backend` / `available_backends` / `default_backend`) — the "not compiled in" / "unknown backend" diagnostics are unchanged.
- Replace the resolved `Backend` enum value with a **feature-gated factory** that returns `Option<Box<dyn cb_backend_api::Backend>>`:

  ```rust
  fn make_backend(sel: Backend) -> Box<dyn cb_backend_api::Backend> {
      match sel {
          #[cfg(feature = "interp")] Backend::Interp => Box::new(cb_backend_interp::InterpBackend),
          #[cfg(feature = "llvm")]   Backend::Llvm   => Box::new(cb_backend_llvm::LlvmBackend),
      }
  }
  ```

- Collapse the run block's three `match` arms into one backend-agnostic call site:

  ```rust
  match backend {
      Some(sel) => match make_backend(sel).execute(&ir_program, &sema_result.interner) {
          Ok(BackendOutcome::Ran { exit_code })   => ExitCode::from(clamp_exit(exit_code)),
          Ok(BackendOutcome::Produced { artifact }) => { println!("cb: wrote {}", artifact.display()); ExitCode::SUCCESS }
          Err(e) => {
              eprintln!("cb: {}", e.message);
              ExitCode::from(match e.kind {
                  BackendErrorKind::Unimplemented => exit::BACKEND_UNIMPLEMENTED, // 3
                  BackendErrorKind::Failed        => 1,
              })
          }
      },
      None => { eprintln!("cb: no backend compiled in; rebuild with --features interp or --features llvm");
                ExitCode::from(exit::USAGE) }   // 2, unchanged
  }
  ```

- **Exit-code policy stays in the driver** (per FD-025): `clamp_exit`, the `exit` module, and the `Unimplemented → 3` / `Failed → 1` / no-backend → 2 mapping all remain here. `cb-backend-api` carries no OS-exit knowledge. Note `clamp_exit` and `exit::BACKEND_UNIMPLEMENTED` may no longer need their current `#[cfg]` gates once the mapping is centralised — to be confirmed during implementation so all four combos stay warning-clean.

## Migration plan (no behavior change first, LLVM second)

1. Create `cb-backend-api` with the trait + `BackendOutcome` + `BackendError`.
2. Implement `Backend` for `InterpBackend` in `cb-backend-interp` (wraps existing `interpret`).
3. Rewire the driver to the factory + single call site. **No observable behavior change** — every exit code (0/1/2/3), diagnostic, and dump path identical to today. Confirm with the existing `cb-driver` test suite across all four feature combos.
4. Implement `Backend` for `LlvmBackend` (stub returning `Unimplemented`) and route the `llvm` arm through it — `--backend llvm` still exits 3 with the same message.
5. **Extract the compile pipeline into `cb-driver/src/lib.rs`**, leaving `main.rs` a thin shell (clap parsing → `run_pipeline()` → `make_backend(...).execute(...)` → map `BackendOutcome` to `ExitCode`). The pipeline body is already `#[cfg]`-free (only `#[cfg(debug_assertions)]` on `verify`), so this is mechanical and behavior-preserving. This is the "structured for a future split" prep from the Executable-topology decision; a later second binary becomes a new bin crate over this `lib.rs` plus a one-line factory change. *(May be deferred to an immediate fast-follow if it would bloat the no-behavior-change diff in step 3 — but keeping it in this FD keeps the seam and the thin-shell structure landing together.)*
6. *(Future, separate FD/work)* Real LLVM codegen fills in `LlvmBackend::execute`, returning `Produced { artifact }`; the driver already handles that outcome.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-backend-api/Cargo.toml` | CREATE | New crate; deps `cb-ir`, `cb-diagnostics`; workspace lints |
| `crates/cb-backend-api/src/lib.rs` | CREATE | `Backend` trait, `BackendOutcome`, `BackendError`, `BackendErrorKind` |
| `Cargo.toml` (workspace) | MODIFY | Add `crates/cb-backend-api` to `members` |
| `crates/cb-backend-interp/Cargo.toml` | MODIFY | Depend on `cb-backend-api` |
| `crates/cb-backend-interp/src/lib.rs` | MODIFY | Add `InterpBackend` implementing `Backend` (keep `interpret`) |
| `crates/cb-backend-llvm/Cargo.toml` | MODIFY | Depend on `cb-backend-api` |
| `crates/cb-backend-llvm/src/lib.rs` | MODIFY | Add `LlvmBackend` implementing `Backend` (stub → `Unimplemented`) |
| `crates/cb-driver/Cargo.toml` | MODIFY | Add unconditional `cb-backend-api` dependency |
| `crates/cb-driver/src/lib.rs` | CREATE | Extracted compile-pipeline core (`run_pipeline` + outcome enum); the shared "fat lib" a future second binary reuses |
| `crates/cb-driver/src/main.rs` | MODIFY | Slim to a thin shell: clap parsing, `make_backend` factory → `Box<dyn Backend>`, call `run_pipeline`, map `BackendOutcome`/`BackendError` to `ExitCode`; keep exit-code policy & `None` path |
| `crates/cb-driver/tests/cli.rs` | MODIFY (if needed) | Assertions should be unchanged; adjust only if message text shifts |

## Verification

- `cargo test -p cb-driver` green across **all four feature combos** (FD-025 matrix): default `interp`, `--features llvm`, `--no-default-features`, `--no-default-features --features llvm`. Exit codes 0/1/2/3 and dump-only paths byte-for-byte unchanged.
- `cargo test --workspace` green.
- `cargo clippy --workspace --all-targets -- -D warnings` clean across all four combos (watch for newly-dead `#[cfg]` gates on `clamp_exit` / `BACKEND_UNIMPLEMENTED`); `cargo fmt --all` clean.
- Manual smoke: `cb --backend llvm <file>` (llvm build) still exits 3 with "not yet implemented"; `cb <file>` (interp) runs and returns the program exit code; `cb --dump-ast <file>` works under `--no-default-features`.
- Confirms the architectural goal: a hypothetical third backend would need a new crate implementing `Backend` + one line in `make_backend`, with **no** new arms in the driver's run logic.

## References

- CLAUDE.md — architectural ground rules ("backend is pluggable", "two backends from day one", "shared IR is the natural seam", "ask before load-bearing structural choices").
- [[FD-025]] Driver CLI, Backend-Selection & Exit-Code Correctness — current `Backend` enum, `--backend` flag, exit-code contract (0/1/2/3), four-feature-combo test matrix this design must preserve.
- [[FD-006]] Diagnostics & driver hardening — original optional-backend cargo-feature wiring.
- [[FD-043]] Interpreter Runtime Teardown Hook — sibling cleanup also feeding toward LLVM readiness.
- [[FD-045]] Catalog Metadata Decoupling — the real prerequisite (surfaced by this FD's `/fd-deep` topology analysis) for a native compiler that type-checks/emits without linking the executable C++ runtime.
- Code review finding DR-R10.
