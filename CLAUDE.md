# cbcompiler_rs

## Project intent

A reimplementation of the **CoolBasic** compiler in **Rust**. The original CoolBasic is a BASIC-dialect game programming language; this is a from-scratch compiler for that language.

## Architectural ground rules

These are design constraints the user stated up front. They predate the code, so honor them when adding modules and crate boundaries:

- **Backend is pluggable.** The frontend (lexer, parser, type checker, IR) must not depend on any specific backend. A backend should be selectable at compile time (cargo features) and ideally at runtime (CLI flag).
- **Two backends from day one:**
  - **LLVM backend** (`cb-backend-llvm`) — for AOT/optimized native codegen. Expect to use the `inkwell` crate (safe LLVM bindings) unless a strong reason emerges to go lower-level. `inkwell` is intentionally not yet a dependency so the workspace builds without an LLVM toolchain installed; add it when codegen starts.
  - **Interpreter backend** (`cb-backend-interp`) — explicitly prioritized for **debuggability**, not raw speed. It should be the easy path for stepping, inspecting locals, and reproducing miscompiles found in the LLVM path. Treat it as the reference implementation when the two backends disagree.
- Shared IR between backends is the natural seam. Keep the IR backend-agnostic; do not leak LLVM types into it.
- The driver (`cb-driver`) wires the backends behind cargo features — `interp` is the default, `llvm` is opt-in via `cargo build --features llvm`. `cargo build` without overrides produces an interp-only `cb` binary; `--no-default-features` yields a no-backend dump-only binary suitable for AST inspection. The runtime `--backend <name>` flag rejects values whose feature is not compiled in.

## Workspace layout

Cargo workspace, edition 2024, resolver 3, `unsafe_code = "deny"` workspace-wide:

| Crate                              | Role                                              | Depends on              |
| ---------------------------------- | ------------------------------------------------- | ----------------------- |
| `crates/cb-diagnostics`           | Diagnostics, `Span`, `FileId`, `Symbol`, `Interner` | —                     |
| `crates/cb-frontend`               | Lexer, parser, AST                                | `cb-diagnostics`        |
| `crates/cb-ir`                     | Backend-agnostic IR + passes                      | `cb-diagnostics`        |
| `crates/cb-sema`                   | Semantic analysis + AST→IR lowering               | `cb-frontend`, `cb-ir`  |
| `crates/cb-backend-interp`         | Interpreter backend (reference impl)              | `cb-ir`                 |
| `crates/cb-backend-llvm`           | LLVM backend                                      | `cb-ir`                 |
| `crates/cb-driver` (binary `cb`)   | CLI; wires frontend, sema, IR, and backends       | all of the above        |

## Commands

```sh
cargo build                       # build the whole workspace
cargo build -p <crate>            # build a single crate
cargo check                       # type-check only, faster
cargo test                        # run all tests
cargo test -p <crate> <pattern>   # run a specific crate's tests (or a name pattern)
cargo run -p cb-driver -- <args>  # run the `cb` driver binary
cargo clippy --workspace --all-targets -- -D warnings   # lint (when clippy is desired)
cargo fmt --all                   # format
```

### Building the LLVM backend (opt-in)

The default build is LLVM-free: `cargo build` / `cargo test --workspace` need no LLVM toolchain (`cb-backend-llvm` compiles as a stub, `inkwell` stays unbuilt). LLVM codegen is opt-in via the driver's `llvm` feature, which enables `cb-backend-llvm/codegen` and pulls in `inkwell` (FD-047).

```sh
cargo build --features llvm                       # driver with the llvm backend
cargo test -p cb-backend-llvm --features codegen  # llvm-backend linkage smoke test
```

Requirements for the `llvm`/`codegen` feature (only then):

- **LLVM 18** — pinned via inkwell `0.9` feature `llvm18-1` → `llvm-sys 181.x`, matching the vendored vcpkg `llvm` port (18.1.6). Point `llvm-sys` at the install with the `LLVM_SYS_181_PREFIX` env var (set it via a user env var or a **git-ignored** `.cargo/config.toml [env]` — never commit a machine-specific path):
  - Windows: build LLVM via the vendored vcpkg with the **dynamic-CRT** triplet — `runtime/vcpkg/vcpkg.exe install llvm:x64-windows-static-md` (static LLVM libs, `/MD` CRT). The `static-md` triplet matches Rust's default CRT (no `libcmt`/`msvcrt` conflict) and is required so a future plugin DLL (`CallDLL`) shares one CRT with the EXE; it's the same triplet the Allegro runtime uses. Use the **MSVC** Rust toolchain. vcpkg puts `llvm-config.exe` under `…/tools/llvm/`, but `llvm-sys` wants `<prefix>/bin/llvm-config.exe`, so expose it with a one-time junction placed directly under the install root, then point the env var at that prefix:

    ```sh
    # <inst> = runtime\vcpkg\installed\x64-windows-static-md
    mkdir <inst>\llvm-sys-prefix
    mklink /J <inst>\llvm-sys-prefix\bin <inst>\tools\llvm
    # then set (user env var, or a git-ignored .cargo/config.toml [env]):
    #   LLVM_SYS_181_PREFIX=<inst>\llvm-sys-prefix
    ```

    With `LLVM_SYS_181_PREFIX` set, `llvm-sys` ignores `PATH`, so any stale LLVM elsewhere on `PATH` won't interfere.
  - Linux/CI: `apt-get install llvm-18-dev`, `LLVM_SYS_181_PREFIX=/usr/lib/llvm-18`.
- **Do not use a static-CRT (`/MT`) LLVM** (e.g. a stock prebuilt) — it conflicts with Rust's dynamic CRT and breaks the plugin-DLL model. Stick to the `x64-windows-static-md` triplet, the same one the Allegro runtime uses.

Keep the default `cargo test --workspace` / CI path LLVM-free — never add `--all-features` to the workspace-wide job (it would force `inkwell` and require an LLVM toolchain everywhere).

### AOT codegen & linking (FD-048)

With `--features llvm`, `cb --backend llvm <file>.cb [-o <out>]` emits a **native executable**. (The IR is not read yet — a fixed empty `main` returning 0 is emitted; real IR→LLVM lowering is a later FD. The full runtime closure is still linked, so that FD adds only codegen, not toolchain work.) The artifact defaults to the source stem + the platform exe suffix, next to the source; `-o`/`--output` overrides it.

- **Link driver.** Linking goes through a compiler *driver* (`clang.exe` on Windows, `cc` on Unix), not a bare `ld`/`lld`, so the CRT/SDK lib paths are auto-discovered (no `vcvars`). On Windows the driver is found via `LLVM_SYS_181_PREFIX` (`<prefix>/bin/clang.exe`, then `<prefix>/tools/llvm/clang.exe`) before any PATH clang — so the pinned LLVM 18 clang is used, never a stray newer clang on PATH.
- **Runtime closure.** The CoolBasic runtime is linked from whatever `cb-runtime-sys` built — the full Allegro closure, or the SDK-free core (CI / no-Allegro). `cb-runtime-sys` (manifest `links = "cb_runtime"`) publishes the lib dir, archive names, and closure-list path as `DEP_CB_RUNTIME_*`; `cb-backend-llvm`'s build script re-exports them as `CB_RT_*` env so the link step never hardcodes a lib list.
- **Unix system libs.** Because the runtime is C++, the link step names `-lstdc++` (`-lc++` on macOS) **and `-lm`** explicitly — the `cc` driver pulls in neither, and the SDK-free closure list is empty, so a whole-archived `cb_math.o` (or real codegen calling runtime math) otherwise fails with `floor … DSO missing from command line`. `cb-runtime-sys` never names libm itself (the interp binary gets it from Rust's std); the standalone AOT link drives its own libs. Windows needs neither (the MSVC CRT supplies math).
- **Windows `/MD` recipe (refined).** To keep the dynamic CRT (matching Rust + the plugin-DLL model): `-Xlinker /nodefaultlib:libcmt` (drop clang's static-CRT default) **plus explicit** `-Xlinker /defaultlib:msvcrt /defaultlib:vcruntime /defaultlib:ucrt`. The explicit defaultlibs are load-bearing: `-fms-runtime-lib=dll` only sets a clang *compile*-phase dependent-lib, and no compile phase runs when linking a pre-built LLVM object — without them `mainCRTStartup` (which calls `main`) is unresolved (LNK2001) for a lazily-linked empty `main`. The produced exe imports `VCRUNTIME140.dll` + `api-ms-win-crt-*` (dynamic CRT), no `libcmt`.

## Language reference

**Do not invent CoolBasic semantics from memory.** The original language has quirks (type sigils, `Type ... EndType` blocks, sub vs. function distinction, etc.) that are easy to get subtly wrong.

Authoritative syntax notes live in `docs/cb_syntax.md`. Consult it before writing lexer/parser/sema code. If something you need isn't documented there yet, ask the user — don't guess — and then add the answer to `cb_syntax.md` so the next session has it.

## Working notes for future Claude sessions

- For load-bearing structural choices (backend trait shape, IR style — SSA vs. stack vs. tree-walk for the interpreter, error/diagnostic crate, parser strategy), prefer asking the user over picking unilaterally. These are hard to reverse.
- Keep `cb-backend-interp` simple and observable. If a feature is hard to implement cleanly in the interpreter, that is a signal the IR or frontend is wrong, not that the interpreter needs to grow.

---

## Feature Design (FD) Management

Features are tracked in `docs/features/`. Each FD has a dedicated file (`FD-XXX_TITLE.md`) and is indexed in `FEATURE_INDEX.md`.

### FD Lifecycle

| Stage | Description |
|-------|-------------|
| **Planned** | Identified but not yet designed |
| **Design** | Actively designing (exploring code, writing plan) |
| **Open** | Designed and ready for implementation |
| **In Progress** | Currently being implemented |
| **Pending Verification** | Code complete, awaiting verification |
| **Complete** | Verified working, ready to archive |
| **Deferred** | Postponed (low priority or blocked) |
| **Closed** | Won't implement (superseded or not needed) |

### Slash Commands

| Command | Purpose |
|---------|---------|
| `/fd-new` | Create a new feature design |
| `/fd-explore` | Explore project - overview, FD history, recent activity |
| `/fd-deep` | Deep parallel analysis — 4 agents explore a hard problem from different angles, verify claims, synthesize |
| `/fd-status` | Show active FDs with status and grooming |
| `/fd-verify` | Post-implementation: commit, proofread, verify |
| `/fd-close` | Complete/close an FD, archive file, update index |

### Conventions

- **FD files**: `docs/features/FD-XXX_TITLE.md` (XXX = zero-padded number)
- **Commit format**: `FD-XXX: Brief description`
- **Numbering**: Next number = highest across all index sections + 1
- **Source of truth**: FD file status > index (if discrepancy, file wins)
- **Archive**: Completed FDs move to `docs/features/archive/`

### Managing the Index

The `FEATURE_INDEX.md` file has four sections:

1. **Active Features** — All non-complete FDs, sorted by FD number
2. **Completed** — Completed FDs, newest first
3. **Deferred / Closed** — Items that won't be done
4. **Backlog** — Low-priority or blocked items parked for later

### Inline Annotations (`%%`)

Lines starting with `%%` in any file are **inline annotations from the user**. When you encounter them:
- Treat each `%%` annotation as a direct instruction — answer questions, develop further, provide feedback, or make changes as requested
- Address **every** `%%` annotation in the file; do not skip any
- After acting on an annotation, remove the `%%` line from the file
- If an annotation is ambiguous, ask for clarification before acting

This enables a precise review workflow: the engineer annotates FD files or plan docs directly in the editor, then asks Claude to address all annotations — tighter than conversational back-and-forth for complex designs.
