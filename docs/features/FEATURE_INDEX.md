# Feature Design Index

Planned features and improvements for CBCompiler2 — a Rust reimplementation of the CoolBasic compiler.

See `CLAUDE.md` for FD lifecycle stages and management guidelines.

## Active Features

| FD | Title | Status | Effort | Priority |
|----|-------|--------|--------|----------|
| [FD-005](FD-005_DELETE_STATEMENT.md) | `Delete` statement (§3.3) | Pending Verification | Medium | Medium |
| [FD-006](FD-006_DIAGNOSTICS_DRIVER_HARDENING.md) | Diagnostics & driver hardening | Open | Medium | Medium |

## Completed

| FD | Title | Completed | Notes |
|----|-------|-----------|-------|
| [FD-004](archive/FD-004_PARSER_CORRECTNESS.md) | Parser correctness & small spec gaps | 2026-05-23 | Closed 17 post-FD-002 review issues: `\` line-continuation made transparent to the parser; empty single-line `If` body now diagnosed (E0215) instead of panicking; `Int`/`UInt` spelling-preserving aliases; implicit decl `z As String = "asd"` (§4.1); sigilled `Next` name with mismatch diagnostic (E0217); empty `New T[]`/`arr[]` return `Expr::Error`; stray `Field` recovery stops at `:`; `Redim` element type accepts array rank markers; `Stmt::Error` span merging from original error to recovered token; duplicate `Default` arm rejected (E0216); `Expr::Field` carries `name_span: Span`; internal-error promotion (E0299) replaces `unreachable!`/silent fallbacks; `STMT_LHS_MIN_BP` derived from `CMP_LBP`; phase-marker noise removed; duplicate error-code constants consolidated. |
| [FD-003](archive/FD-003_LEXER_CORRECTNESS.md) | Lexer correctness & robustness pass | 2026-05-23 | Closed 10 post-FD-001 review issues: `bump_char` panic reachability, `scan_one` UTF-8 recovery, `IntLit→u64` (typing moved to sema), `FloatBits` newtype (`Token: Eq`), bare-`\r` test pinning, hex/binary `$_`/`%_` UX, block-comment label coverage, `LONGEST_KEYWORD_LEN` invariant test, `UnexpectedChar`/`InvalidChar` collapse, `u32`-offset `debug_assert!`. Raw-string mid-file recovery deferred. |
| [FD-002](archive/FD-002_PARSER.md) | Parser | 2026-05-21 | Hand-written recursive descent + Pratt, arena-allocated AST, recovering on `Newline`/`Colon`/`End*` |
| [FD-001](archive/FD-001_LEXER.md) | Lexer | 2026-05-17 | Hand-written recovering lexer + `cb-diagnostics` crate |

## Deferred / Closed

| FD | Title | Status | Notes |
|----|-------|--------|-------|
| - | - | - | No deferred features yet |

## Backlog

Low-priority or blocked items. Promote to Active when ready to design.

| FD | Title | Notes |
|----|-------|-------|
| - | - | No backlog items yet |
