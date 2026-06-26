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

There are no project-specific build steps yet. Update this section when there are.

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
