# FD-053: Include Processing — Driver-Level Include-Resolution Pass

**Status:** Pending Verification
**Priority:** Medium
**Effort:** High (> 4 hours)
**Impact:** Makes `Include "file.cb"` actually pull in another source file, so multi-file programs (libraries + main) compile and run. Previously the statement parsed and was then silently discarded.

## Problem

`Include` (`cb_syntax.md` §2.2) is lexed (`Kw::Include`) and parsed into `Stmt::Include { path }`, but **nothing downstream resolves it**. Sema (`check.rs`) and lowering (`lower.rs`) both list `Stmt::Include` in a no-op match arm (their "handled at the program level" comments are misleading — no such handling exists), and the driver (`cb-driver/src/lib.rs::compile`) reads exactly **one** file. An `Include "other.cb"` therefore compiles without error and has zero effect: the included file is never read, so its functions/types/code never enter the compilation.

This was the documented design in FD-007 ("the driver resolves includes, parses each file, and merges the resulting ASTs into a single program before calling `analyze()`") but the pass was never built. None of the §2.2 semantics exist: relative-path resolution, include-at-most-once / cycle suppression, or the top-level-only error.

## Solution

Add an **include-resolution pass in `cb-driver`** that runs after reading the main file and before `cb_sema::analyze()`. `Stmt::Include` nodes are consumed by this pass and never reach sema.

Key architecture choices:

1. **One shared `Arena` for all files.** `NodeId`s are arena-relative indices, so merging separately-parsed arenas would require offsetting every embedded `NodeId` — fragile and a maintenance tax as the AST grows. Instead, parse every file into a *single* arena so all ids stay globally valid. This needs a frontend entry that parses tokens into an existing arena (e.g. `parse_into(&mut Arena, &tokens, src, file) -> (program, diagnostics)`); the current `parse()` becomes a thin wrapper that allocates a fresh arena and delegates.

2. **`SourceMap` is the single source of truth for text.** Each included file is registered with `SourceMap::add`, getting its own `FileId`. Spans already carry their `FileId`, and the diagnostic renderer already reads through `SourceMap`, so **cross-file diagnostics render correctly for free** once every file is registered.

3. **`analyze()` evolves from `source: &str` to `&SourceMap`** (anticipated by FD-007). Sema slices identifier text out of spans; with multiple files it must resolve each span's text via its `FileId` rather than indexing one string. This is the largest ripple — `cb-sema`'s span-slicing / `intern_span` and the `analyze` signature.

4. **In-place, depth-first expansion (textual-paste order).** Each top-level `Stmt::Include` in the program list is replaced by the included file's recursively-expanded top-level statements. Included top-level code thus runs at the include site, while functions/types stay globally hoisted (§2.1) regardless of which file they came from — matching CoolBasic's textual-include model and §2.3 ("execution begins at the first statement of the main file").

5. **§2.2 rules enforced in the resolver:**
   - **Path resolution:** relative to the *including* file's directory; absolute paths accepted as-is. Canonicalize (`std::fs::canonicalize`) to key the visited set.
   - **At-most-once / cycles:** a `HashSet` of canonical paths (seeded with the main file). An already-seen path expands to nothing — repeats and cycles terminate silently after the first inclusion.
   - **Top-level-only (new E0333):** an `Include` anywhere but the top-level program list is a compile error. Because the resolver only expands top-level includes, a nested one must be *diagnosed*, not silently dropped — scan nested statement bodies for stray `Stmt::Include`.
   - **Unreadable / missing file (new E0334):** points at the include's path span. (A missing string-literal path is already the parser's `E_EXPECTED_TOKEN`.)

After this pass, sema and lowering can drop their `Stmt::Include` arms (the node is now unreachable in their input; keep as a defensive `unreachable!` if preferred).

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-driver/src/include.rs` | CREATE | The include-resolution pass: read → resolve path → tokenize → `parse_into` shared arena → recursively expand top-level `Include`s depth-first → dedupe via canonical-path set; emits E0333/E0334. |
| `crates/cb-driver/src/lib.rs` | MODIFY | `compile()` builds the merged `(arena, program, SourceMap)` via the resolver, registers every file, then calls sema/lowering with the `SourceMap`. |
| `crates/cb-frontend/src/parser.rs` | MODIFY | Add `parse_into(&mut Arena, …)` so multiple files share one arena; keep `parse()` as a wrapper. |
| `crates/cb-sema/src/lib.rs` (`analyze`) | MODIFY | Signature `source: &str` → `&SourceMap` (drop/repurpose the single `file` arg). |
| `crates/cb-sema/src/check.rs` | MODIFY | Resolve span text per-`FileId` from the `SourceMap` (`intern_span`/slicing); remove the `Stmt::Include` no-op arm. |
| `crates/cb-sema/src/lower.rs` | MODIFY | Remove the `Stmt::Include` no-op arm (unreachable post-resolution). |
| `crates/cb-sema/src/diagnostics.rs` | MODIFY | Define `E0333` (Include not at top level) and `E0334` (cannot read included file). |
| `crates/cb-driver/tests/include.rs` + `tests/fixtures/include/**` | CREATE | Multi-file integration fixtures and tests (see Verification). |

## Verification

Driver integration tests over on-disk fixtures (run through the interp backend so output is observable):

- **Happy path:** `main.cb` includes `lib.cb` (a function + Type/Struct); compile + run, assert program output uses the included definitions.
- **Relative resolution:** include from a subdirectory (`Include "graphics/sprite.cb"`), and a nested include resolved relative to *its* file, not the main file.
- **Repeat include:** including the same file twice → second is a no-op (no duplicate-definition error).
- **Cycle:** A includes B includes A → compiles once, terminates (no infinite loop / stack overflow).
- **Missing file → E0334**; **nested `Include` (inside a function/`If`) → E0333**.
- **Cross-file diagnostics:** an error in an included file renders with that file's name/line.
- **No regression:** `cargo test --workspace` green; the single-file path is unchanged (the `parse()`/`analyze()` wrappers preserve existing behavior). `cargo clippy --workspace --all-targets -- -D warnings` clean.

## Related

- [FD-007](archive/FD-007_Semantic_Analysis.md) — specified this driver-resolves-then-merges design (§"Include handling") and the `analyze(&str) → &SourceMap` evolution; never implemented.
- [FD-002](archive/FD-002_PARSER.md) — `Stmt::Include` / `parse_include` and the arena model.
- `docs/cb_syntax.md` §2.2 (Includes), §2.1 (top-level hoisting), §2.3 (entry point).
