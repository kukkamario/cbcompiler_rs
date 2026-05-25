# Feature Design Index

Planned features and improvements for CBCompiler2 — a Rust reimplementation of the CoolBasic compiler.

See `CLAUDE.md` for FD lifecycle stages and management guidelines.

## Active Features

| FD | Title | Status | Effort | Priority |
|----|-------|--------|--------|----------|
| [FD-012](FD-012_CATALOG_CPP_TEMPLATE_DSL.md) | Catalog DSL via C++ Templates and Function Pointers | Open | Medium | Medium |

## Completed

| FD | Title | Completed | Notes |
|----|-------|-----------|-------|
| [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) | Runtime Custom Types | 2026-05-24 | Catalog ABI v2 with `CbTypeDesc` type declarations. `IrType::RuntimeType`, `Type::RuntimeType`, `DeclKind::RuntimeTypeDef`, `Value::OpaqueHandle(u64)`. Opaque handles support assignment, null comparison, identity comparison; sema rejects arithmetic, ordering, field access. TestHandle test type in catalog. |
| [FD-010](archive/FD-010_INTERPRETER_BACKEND.md) | Interpreter Backend Implementation | 2026-05-24 | `cb-backend-interp` crate: stack-based interpreter executing `cb_ir::Program`. Value enum (14 variants: primitives, Rc<str> strings, Rc<RefCell<ArrayObj>> arrays, slab-allocated TypeInstances, Box<StructObj> structs, FnPtr). Execution loop dispatches ~22 InstKind arms. Slab-based Type instance heap with doubly-linked lists (First/Last/Next/Previous, Delete with rewind). Frame pooling for call performance. Observer<O> trait (generic, zero-cost NoopObserver via monomorphization) with before/after_inst, on_call, on_return, on_trap hooks. IR changes: GlobalId + LoadGlobal/StoreGlobal, TypeDefId replacing Symbol in NewType/First/Last, terminator_span on BasicBlock. Sema: First/Last/Next/Previous as intrinsic calls, ForEach type inference. 23 integration tests covering all instruction types. |
| [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) | Runtime Library Architecture | 2026-05-24 | Catalog-only milestone: C runtime library (`cb-runtime-sys`) with `cc`-based build, `#[repr(C)]` FFI bindings, `cb_runtime_get_catalog()` entry point. Three stub functions (`Print`, `Abs(Int)`, `Abs(Float)`). Sema: `DeclKind::RuntimeFn`/`OverloadSet`, overload resolution by exact-match scoring, `ResolvedCall` tracking. Lowerer: `FuncId`-based dispatch with `func_table` (`FuncKind::UserDefined`/`Runtime`). IR: `FuncId`, `FuncDecl`, `FuncKind` types; verifier validates `FuncId` bounds; printer resolves names from `func_table`. Driver loads catalog via FFI at startup. |
| [FD-008](archive/FD-008_IR.md) | Intermediate Representation | 2026-05-24 | `cb-ir` crate: `Program`/`Function`/`BasicBlock`/`Inst` types, `IrType`, full `InstKind` enum (arithmetic, memory, type-linked-list, intrinsics, constants). AST→IR lowering in `cb-sema::lower`: expression/statement lowering, control-flow desugaring (If/ElseIf/Else, While, Repeat/Forever, Repeat/While, For with direction-aware step, ForEach over types and arrays, Select/Case with multi-value and Default), short-circuit And/Or via branch chains, Break/Continue/Goto/Label, implicit conversion insertion, constant inlining. IR text printer for `--dump-ir`. Structural verifier (debug builds): terminators, register def-before-use, block targets, local bounds. Driver: `--dump-ir`/`--dump-ast` flags, lowering + verify integration. |
| [FD-007](archive/FD-007_Semantic_Analysis.md) | Semantic Analysis | 2026-05-23 | New `cb-sema` crate: two-pass analysis (declaration hoisting + type checking), `Symbol`/`Interner` in `cb-diagnostics` with case-insensitive dedup, 22 diagnostic codes (E0300–E0321), implicit conversion tracking with narrowing warnings, `Const` literal folding, compiler intrinsics (`Len`/`Int`/`Float`/`Str`/`Bool`), `Delete` lvalue/rvalue classification, Goto-into-For restriction, driver integration. Parser extended to allow type keywords (`Int`/`Float`/`Bool`) as intrinsic call identifiers. |
| [FD-006](archive/FD-006_DIAGNOSTICS_DRIVER_HARDENING.md) | Diagnostics & driver hardening | 2026-05-23 | Driver: optional `cb-backend-{interp,llvm}` deps behind `interp` (default) / `llvm` cargo features; `--no-default-features` produces a dump-only binary; `--backend <name>` flag rejects values whose feature is not compiled in. AST printer moved to `cb-frontend::ast_print` with explicit arms across `Expr`/`Stmt`/`TypeExpr`/`CaseArm` so new variants fail the build instead of silently being skipped (the FD-005 `Stmt::Delete` regression mode). `Renderer::emit` now returns `io::Result<()>` and `CliRenderer` validates `Span::end >= start`, `FileId` existence, and `end <= text_len` before handing to codespan-reporting. Driver tests: 9 new (1 ignored — `errors_dominate_warnings` blocked on first warning-producing sema construct). Renderer tests: 5 new (3 snapshots + 2 failure-path asserts). A-phase diagnostics API polish was already landed on master before this branch. |
| [FD-005](archive/FD-005_DELETE_STATEMENT.md) | `Delete` statement (§3.3) | 2026-05-23 | Added `Kw::Delete`, `Stmt::Delete { operand }`, and `parse_delete`. Parser is permissive on operand shape (matches `Stmt::Assign`'s `target` boundary); lvalue/rvalue classification and §9.2 trap diagnostics deferred to sema + the future interpreter FD. Stops `Delete x` from being silently misparsed as a paren-less call. |
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
