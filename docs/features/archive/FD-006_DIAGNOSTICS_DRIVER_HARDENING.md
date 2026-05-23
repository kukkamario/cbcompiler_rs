# FD-006: Diagnostics & Driver Hardening

**Status:** Complete
**Completed:** 2026-05-23
**Priority:** Medium (driver cargo-feature gating is time-sensitive — should land before `inkwell` is added)
**Effort:** Medium (1-4 hours)
**Impact:** Cleans up `cb-diagnostics` API issues, gates the LLVM backend behind a cargo feature so the workspace stays buildable without an LLVM toolchain, lifts the AST printer out of `cb-driver` (where its catch-all `_ => {}` arms silently swallow new AST variants), and adds the first integration tests for both the driver and the diagnostics renderer.

## Problem

A workspace-wide code review (2026-05-23) flagged the post-FD-001/FD-002 driver and diagnostics layers as the next-most-fragile area after the lexer/parser. Three categories of issue:

### A. `cb-diagnostics` API polish

A1. **`offset_to_line_col` returns a byte column, not a character or visual column** (`source.rs:158-175`). The doc admits it; the name invites misuse by any future consumer (LSP, IDE).

A2. **Bare-`\r` handling diverges from codespan-reporting.** `LineIndex::new` (`source.rs:117-126`) treats `\r` as its own terminator; codespan's default `Files` impl does not. Any code that compares positions from both paths on a `\r`-only file will disagree.

A3. **`CliRenderer::emit` swallows all errors via `let _ = term::emit(...)`** (`render.rs:56-64`). Hides malformed `Span`s (`start > end`) and labels pointing at unregistered `FileId`s — both of which silently drop the user's diagnostic.

A4. **`Span::new` doesn't validate `end >= start`** (`diagnostic.rs:19-22`). `Span::len` uses `saturating_sub` (`:24-26`) which masks construction bugs.

A5. **`SourceMap::add` doesn't dedupe duplicate paths** (`source.rs:66-76`). Two `FileId`s for the same file render identically.

A6. **`Diagnostic.code: &'static str`** (`diagnostic.rs:84`) blocks generated/namespaced codes. Easier to change now than later.

A7. **`Severity::Help` has no factory** (`diagnostic.rs:99-111`). Either add `help()` or drop the variant.

A8. **`tests/line_index.rs:43-47`** (`out_of_bounds_clamps`) asserts nothing — only checks non-panic.

### B. Driver hardening

B1. **Driver depends on both backends unconditionally.** `crates/cb-driver/Cargo.toml:15-17` depends on `cb-backend-interp` *and* `cb-backend-llvm`. When `inkwell` is added (per FD-001/CLAUDE.md ground rules), the whole workspace becomes unbuildable without an LLVM toolchain. CLAUDE.md's architectural rule says the backend must be selectable at compile time. **Time-sensitive: must land before `inkwell` is added.**

B2. **AST pretty-printer is ~180 lines of hand-rolled `match` in `cb-driver/src/main.rs:63-242`,** with `_ => {}` catch-alls in `children_of` (`:108, :212, :223`). New AST variants compile silently and their children are skipped — confirmed by FD-005, whose `Stmt::Delete` variant would not appear in the dump until someone updates the driver. The printer belongs in `cb-frontend` (next to `ast.rs`).

B3. **No tests for the `cb` binary at all.** No assertion that valid files exit 0, that errors exit non-zero, that diagnostics reach stderr, that the AST dump matches a fixture, or that missing-arg / missing-file produce the right exit codes.

### C. `CliRenderer` has zero direct tests

The renderer is load-bearing for user-visible errors but is only exercised transitively through the (also untested) driver. A simple "file.cb:1:1 error[E0101]: …" assertion against a known fixture would catch a class of silent regressions.

## Solution

Three loosely-coupled subprojects; A and B/C can be reviewed independently.

### A. Diagnostics API polish

| # | Approach |
|---|----------|
| A1 | Rename `offset_to_line_col` to `offset_to_line_byte_col`. Add a separate `offset_to_line_char_col(offset, source: &str)` that counts chars. Do not add a "visual column" helper now — tab expansion belongs in the renderer. |
| A2 | Document `LineIndex`'s bare-`\r` behaviour in the type doc, and add an explicit test crossing it with codespan's column reporting on a `\r`-only file. If they disagree, file a separate FD; for now, ship the divergence-documented version. |
| A3 | Change `Renderer::emit` to return `io::Result<()>`. Driver propagates failures (already returns an exit code). Inside `CliRenderer::emit`, validate `Span::end >= start` and that every `FileId` referenced by labels exists in the `SourceMap`; emit an internal-error log line and return `Err` if not. |
| A4 | Add `debug_assert!(end >= start)` to `Span::new`. Leave `saturating_sub` in `len` so release builds don't panic on bad data, but the debug-assert catches construction bugs in tests. |
| A5 | `SourceMap::add` returns the existing `FileId` if a file with the same path is already registered. Add a `add_anonymous(text: String) -> FileId` for callers that genuinely want a fresh slot (REPL, synthetic inputs). |
| A6 | Introduce `DiagnosticCode(&'static str)` newtype. Constructors take `Into<DiagnosticCode>`. `&'static str` still works via `From` to keep migration trivial. |
| A7 | Add `Diagnostic::help(msg)`. Used by the renderer to render the `help:` line — no other change. |
| A8 | Change `out_of_bounds_clamps` to `assert_eq!(out, offset_to_line_byte_col(text.len()))` and rename. Add tests for `line_byte_range`, `line_index_of_offset`, `line_count`, multi-byte UTF-8 offsets, mixed line endings in one source, trailing-newline behaviour. |

### B. Driver hardening

B1. **Cargo feature gating.** In `crates/cb-driver/Cargo.toml`:

```toml
[features]
default = ["interp"]
interp = ["dep:cb-backend-interp"]
llvm = ["dep:cb-backend-llvm"]

[dependencies]
cb-backend-interp = { workspace = true, optional = true }
cb-backend-llvm = { workspace = true, optional = true }
# … other deps unchanged
```

`main.rs` uses `#[cfg(feature = "interp")]` and `#[cfg(feature = "llvm")]` blocks around backend imports and CLI dispatch. The CLI accepts `--backend interp|llvm` and rejects values whose feature is not compiled in. Default-built binary supports `interp` only; LLVM users opt in with `cargo build -p cb-driver --features llvm`.

B2. **Move the AST printer to `cb-frontend`.** New module `crates/cb-frontend/src/ast_print.rs` exporting:

```rust
pub fn debug_print(out: &mut dyn fmt::Write, arena: &Arena, root: NodeId) -> fmt::Result;
```

Replace every `_ => {}` catch-all in the moved code with explicit arms for every current AST variant. Add `#![deny(non_exhaustive_omitted_patterns_lint)]` (or equivalent — clippy's `exhaustive_patterns` lint pinned per crate) so that any future AST variant addition breaks the build, not silently the dump.

`cb-driver` calls `cb_frontend::ast_print::debug_print(&mut stdout, …)` instead of hand-rolling the match.

B3. **Driver integration tests.** New file `crates/cb-driver/tests/cli.rs` using `assert_cmd` (add as dev-dep) or a hand-rolled `Command` wrapper:

- Valid file → exit 0, AST dump matches stored snapshot.
- File with lex error → exit 1, stderr contains the error code.
- File with parse error → exit 1, stderr contains the error code.
- Missing arg → exit 2, "usage: cb <file.cb>".
- Missing file → exit 2, "failed to read".
- File where both warnings and errors are produced → exit 1 (errors dominate).

### C. `CliRenderer` direct tests

New file `crates/cb-diagnostics/tests/render.rs`:

- Render a single-line, single-label error → string matches snapshot.
- Render a span crossing two lines.
- Render a diagnostic with primary + secondary labels in two different files.
- Render a label pointing at a `FileId` not in the `SourceMap` → returns `Err`, captured `String` empty (post-A3).
- Render a label with a span past EOF → returns `Err`.

### Out of scope

- LSP renderer.
- JSON-output renderer for tests (defer until needed).
- Removing `text.clone()` in `main.rs:33` — minor, defer.
- Replacing the ad-hoc arg parser with `clap` — defer until `--backend` and `--emit` accumulate friends.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-diagnostics/src/source.rs` | MODIFY | A1 rename + char-col helper; A5 dedupe; doc A2. |
| `crates/cb-diagnostics/src/diagnostic.rs` | MODIFY | A4 debug-assert; A6 `DiagnosticCode` newtype; A7 `help()` factory. |
| `crates/cb-diagnostics/src/render.rs` | MODIFY | A3 `emit` returns `io::Result<()>`, validates spans/file-ids. |
| `crates/cb-diagnostics/src/lib.rs` | MODIFY | Re-export new types. |
| `crates/cb-diagnostics/tests/line_index.rs` | MODIFY | A8 expanded coverage. |
| `crates/cb-diagnostics/tests/render.rs` | CREATE | C: direct renderer tests with snapshot output. |
| `crates/cb-driver/Cargo.toml` | MODIFY | B1 features; add `assert_cmd` (or pin a hand-rolled approach) as dev-dep. |
| `crates/cb-driver/src/main.rs` | MODIFY | B1 `#[cfg]` gating + `--backend` CLI flag; B2 use moved printer; A3 propagate `Renderer::emit` errors. |
| `crates/cb-driver/tests/cli.rs` | CREATE | B3 integration tests with fixtures. |
| `crates/cb-frontend/src/ast_print.rs` | CREATE | B2 moved AST printer, exhaustive variant arms. |
| `crates/cb-frontend/src/lib.rs` | MODIFY | `pub mod ast_print;`. |
| `Cargo.toml` | MODIFY | Add `assert_cmd` (or alternative) to `[workspace.dependencies]` if used. |
| `CLAUDE.md` | MODIFY | Note that the LLVM backend is opt-in via `--features llvm`; default workspace `cargo build` no longer pulls it in. |

## Verification

- `cargo build -p cb-driver` (default features, `interp` only) succeeds without LLVM installed.
- `cargo build -p cb-driver --features llvm` still builds today (no inkwell yet); the gating just proves the structure works.
- `cargo build -p cb-driver --no-default-features` builds (driver with no backend, used for AST-dump-only use cases).
- `cargo test --workspace` green, including the new `tests/cli.rs` and `tests/render.rs` files.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.
- AST-printer extensibility: add a fake `Stmt::Foo` variant temporarily, confirm `cargo build` fails (not silently passes). Revert.
- Driver integration tests cover each `.claude/missing_tests.md` "cb-driver" bullet.

## Related

- `CLAUDE.md` — "Backend is pluggable" architectural rule (the cargo-feature work satisfies it)
- `.claude/missing_tests.md` — "cb-diagnostics" and "cb-driver" sections
- `docs/features/archive/FD-001_LEXER.md` — introduced `cb-diagnostics`
- `docs/features/archive/FD-002_PARSER.md` — introduced `cb-driver`
- FD-003, FD-004, FD-005 — AST changes there may bump the AST-printer's explicit-arms list; minor coordination needed
- Future interpreter FD — will add `--backend interp` runtime semantics behind the same feature flag
