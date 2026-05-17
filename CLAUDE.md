# CBCompiler2

## Project intent

A reimplementation of the **CoolBasic** compiler in **Rust**. The original CoolBasic is a BASIC-dialect game programming language; this is a from-scratch compiler for that language.

## Architectural ground rules

These are design constraints the user stated up front. They predate the code, so honor them when adding modules and crate boundaries:

- **Backend is pluggable.** The frontend (lexer, parser, type checker, IR) must not depend on any specific backend. A backend should be selectable at compile time (cargo features) and ideally at runtime (CLI flag).
- **Two backends from day one:**
  - **LLVM backend** (`cb-backend-llvm`) — for AOT/optimized native codegen. Expect to use the `inkwell` crate (safe LLVM bindings) unless a strong reason emerges to go lower-level. `inkwell` is intentionally not yet a dependency so the workspace builds without an LLVM toolchain installed; add it when codegen starts.
  - **Interpreter backend** (`cb-backend-interp`) — explicitly prioritized for **debuggability**, not raw speed. It should be the easy path for stepping, inspecting locals, and reproducing miscompiles found in the LLVM path. Treat it as the reference implementation when the two backends disagree.
- Shared IR between backends is the natural seam. Keep the IR backend-agnostic; do not leak LLVM types into it.

## Workspace layout

Cargo workspace, edition 2024, resolver 3, `unsafe_code = "deny"` workspace-wide:

| Crate                              | Role                                              | Depends on              |
| ---------------------------------- | ------------------------------------------------- | ----------------------- |
| `crates/cb-frontend`               | Lexer, parser, AST, semantic analysis             | —                       |
| `crates/cb-ir`                     | Backend-agnostic IR + passes                      | —                       |
| `crates/cb-backend-interp`         | Interpreter backend (reference impl)              | `cb-ir`                 |
| `crates/cb-backend-llvm`           | LLVM backend                                      | `cb-ir`                 |
| `crates/cb-driver` (binary `cb`)   | CLI; wires frontend, IR, and backends together    | all of the above        |

`cb-frontend` does not yet depend on `cb-ir` — wire that up when lowering exists.

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

Authoritative syntax notes live in [`docs/cb_syntax.md`](docs/cb_syntax.md). Consult it before writing lexer/parser/sema code. If something you need isn't documented there yet, ask the user — don't guess — and then add the answer to `cb_syntax.md` so the next session has it.

## Working notes for future Claude sessions

- For load-bearing structural choices (backend trait shape, IR style — SSA vs. stack vs. tree-walk for the interpreter, error/diagnostic crate, parser strategy), prefer asking the user over picking unilaterally. These are hard to reverse.
- Keep `cb-backend-interp` simple and observable. If a feature is hard to implement cleanly in the interpreter, that is a signal the IR or frontend is wrong, not that the interpreter needs to grow.
