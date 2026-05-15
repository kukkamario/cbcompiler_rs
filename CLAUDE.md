# CBCompiler2

## Project intent

A reimplementation of the **CoolBasic** compiler in **Rust**. Greenfield — the directory is currently empty and git has not yet been initialized.

The original CoolBasic is a BASIC-dialect game programming language; this project is a from-scratch compiler for that language.

## Architectural ground rules

These are design constraints the user has stated up front. They predate any code, so honor them when scaffolding modules and crate boundaries:

- **Backend is pluggable.** The frontend (lexer, parser, type checker, IR) must not depend on any specific backend. A backend should be selectable at compile time (cargo features) and ideally at runtime (CLI flag).
- **Two backends from day one:**
  - **LLVM backend** — for AOT/optimized native codegen. Expect to use the `inkwell` crate (safe LLVM bindings) unless a strong reason emerges to go lower-level.
  - **Interpreter backend** — explicitly prioritized for **debuggability**, not raw speed. It should be the easy path for stepping, inspecting locals, and reproducing miscompiles found in the LLVM path. Treat it as the reference implementation when the two backends disagree.
- Shared IR between backends is the natural seam. Keep the IR backend-agnostic; do not leak LLVM types into it.

## Conventions for the early scaffolding

- Workspace layout (suggested, confirm with user before committing):
  - `crates/cb-frontend` — lexer, parser, AST, semantic analysis
  - `crates/cb-ir` — backend-agnostic IR + passes
  - `crates/cb-backend-interp` — interpreter backend
  - `crates/cb-backend-llvm` — LLVM backend (gated behind a `llvm` feature)
  - `crates/cb-driver` (or `cb`) — CLI binary that wires it all together
- Until cargo is initialized, common commands are just the standard `cargo build` / `cargo test` / `cargo run -p <crate>`. There are no project-specific build steps to document yet — update this section once they exist.

## Working notes for future Claude sessions

- The repo is **not yet a git repo**. Before making non-trivial changes, run `git init` and make an initial commit so subsequent work is reviewable.
- When the repo is still empty or near-empty, prefer asking the user about structural choices (crate split, backend trait shape, IR style — SSA vs. stack vs. tree-walk for interpreter) over picking unilaterally. These decisions are load-bearing and hard to reverse later.
- Do not invent CoolBasic language semantics from memory. When a language detail is needed, ask the user or request a reference — the original CoolBasic has quirks (e.g. its type sigils, `Type ... EndType` blocks, function/sub distinction) that are easy to get subtly wrong.
