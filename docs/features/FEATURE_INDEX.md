# Feature Design Index

Planned features and improvements for cbcompiler_rs — a Rust reimplementation of the CoolBasic compiler.

See `CLAUDE.md` for FD lifecycle stages and management guidelines.

## Active Features

| FD | Title | Status | Effort | Priority |
|----|-------|--------|--------|----------|
| [FD-050](FD-050_OPTIONAL_TRAP_GENERATION.md) | Optional Trap Generation | Planned | Medium | Medium |
| [FD-056](FD-056_USER_FUNCTION_OVERLOADING.md) | User Function Overloading | Open | High | Medium |

## Deferred / Closed

| FD | Title | Status | Notes |
|----|-------|--------|-------|
| - | - | - | No deferred features yet |

## Backlog

Low-priority or blocked items. Promote to Active when ready to design.

| FD | Title | Notes |
|----|-------|-------|
| - | Video Playback | After Sound ([FD-041](FD-041_SOUND_RUNTIME_FUNCTIONS.md)) — video mixes audio through the sound interface. |
| - | DATA / `Read` / `Restore` | Compiler-side `DATA` statements + runtime cursor. |
| - | `Encrypt` / `Decrypt` | Independent utility. |
| - | Plumbing System funcs | `Crc32`, `SetWindow`, `FrameLimit`, `Errors` — window/loop/error-display plumbing. |
| - | `CallDLL` | Plugin/FFI; lowest priority. |

## Completed

| FD | Title | Completed | Notes |
|----|-------|-----------|-------|
| [FD-055](archive/FD-055_SELF_CONTAINED_WINDOWS_TOOLCHAIN.md) | Self-Contained Windows AOT Toolchain | 2026-06-29 | A released `cb` AOT-compiles on Windows with **no Visual Studio / Windows SDK installed**. A one-time `cb --setup-toolchain` runs a bundled `xwin` to fetch the MS CRT + Windows SDK *import libs* into `%LOCALAPPDATA%\cb\winsdk`; `link.rs`'s `win_sdk_dir()` discovers that sysroot and injects `/libpath:` into the lld link (discovery alone gates it, so a dev box with a real SDK links byte-for-byte as before). Only the VC++ Redistributable is still needed to *run* output. CI ships only the MIT/Apache `xwin.exe` (never MS content) + a Server Core no-SDK smoke job. Verified: `diff_llvm` 54/54 + Spike 1 (lld resolves the `/MD` recipe + closure libs from an xwin splat alone). The no-SDK Server Core smoke job is a first cut, pending a tag-triggered run. |
| [FD-054](archive/FD-054_LLVM_OPTIMIZATION_PASSES.md) | LLVM Optimization Passes & `-O` Flag | 2026-06-29 | The LLVM/AOT backend now actually optimizes. A single `OptLevel` (`O0/O1/O2/O3/Os/Oz`) threads from a new driver `-O` flag (default `O2`) into the backend, driving **both** knobs together: the IR-level `default<Ox>` pass pipeline via `Module::run_passes` and the codegen-level `TargetMachine` optimization. `O0` preserves the prior raw lowering; interp ignores `-O` like `-o`. Verified by the 54-fixture `diff_llvm` differential suite at the new default plus an `-O0…-Oz` level sweep. |
| [FD-053](archive/FD-053_INCLUDE_PROCESSING.md) | Include Processing — Driver-Level Include-Resolution Pass | 2026-06-29 | `Include "file"` now actually pulls in another source file (was parsed then silently dropped). A driver-level resolver parses every reachable file into one shared arena + `SourceMap`, splicing each top-level `Include` in place; relative paths resolve against the including file, each file is included at most once (repeats/cycles dropped), unreadable → E0334, nested `Include` → E0333. `analyze`/`lower` take `&SourceMap` so spans (and cross-file diagnostics) resolve per-file. |
| [FD-052](archive/FD-052_REPEAT_UNTIL_LOOP.md) | `Repeat … Until` Loop — Loop Until Condition Is Truthy | 2026-06-28 | Adds the post-test `Repeat … Until cond`: body runs once, loops while `cond` is falsy, exits when truthy — the dual of `Repeat … While`. Frontend (`Kw::Until`, `Stmt::RepeatUntil`) + sema lowering (`lower_repeat_until`, a `lower_repeat_while` clone with the `BranchIf` arms swapped) only; no new IR instruction, no backend work. Verified by the 54-fixture `diff_llvm` differential suite (interp == native). |
| [FD-051](archive/FD-051_STRING_LITERAL_ESCAPE_SYNTAX.md) | String Literal Syntax — Verbatim `"..."`, Escapes Move to `$"..."` | 2026-06-28 | Plain `"..."` is now verbatim (Windows paths like `"C:\new"` stop mangling); the C-style escape set moves to a new `$"..."` form (the `$` is a mode marker, not interpolation). `"""..."""` raw strings unchanged. Scoped to `cb-frontend` — decoding bakes the finished `String` at parse time, so IR + both backends untouched. |
| [FD-049](archive/FD-049_IR_TO_LLVM_LOWERING.md) | IR → LLVM Lowering | 2026-06-28 | Walks `cb_ir::Program` and emits real native code — every `InstKind` + terminator lowered (scalar core, strings, arrays, user Types, value structs, function pointers) over native-only C-ABI heap helpers; `cb --backend llvm <file>` now behaves like `--backend interp`, gated by the `diff_llvm` differential suite (53 fixtures) + a two-phase multi-agent review-fix pass (13 findings). Trap *policy* (the div/mod-by-zero guard, `Terminator::Trap` lowering, checked/unchecked switch) split to FD-050. |
| [FD-048](archive/FD-048_BASIC_LLVM_CODEGEN_AND_TOOLING_DRIVER.md) | Basic LLVM Codegen & Tooling Driver | 2026-06-26 | First native exe: in-memory `inkwell` module → native object → CRT-aware driver link (`clang`/`cc`) against the full runtime closure → runnable exe. IR not read yet (fixed empty `main`); `/MD` dynamic CRT on Windows verified (no `libcmt`). `cb --backend llvm <file> [-o out]`; default build/CI stay LLVM-free. |
| [FD-047](archive/FD-047_LLVM_DEPENDENCY_SETUP.md) | LLVM Dependency Setup | 2026-06-26 | Adds `inkwell`/LLVM 18 as an opt-in, feature-gated dependency (`cb-backend-llvm/codegen`, driver `llvm`); default build/test/CI stay LLVM-free. Linkage smoke test + Linux CI job; Windows toolchain via vcpkg `x64-windows-static-md`. No codegen yet (`--backend llvm`→exit 3 preserved). |
| [FD-043](archive/FD-043_INTERPRETER_TEARDOWN_HOOK.md) | Interpreter Runtime Teardown Hook (`about_to_exit`) | 2026-06-26 | Fires the reserved `cb_runtime_init` `about_to_exit` hook exactly once on every interpreter exit path; full build runs a coarse Allegro teardown via a core registration seam, SDK-free build is a no-op. |
| [FD-046](archive/FD-046_STRING_NUMBER_CONVERSION_PRIMITIVES.md) | Core-Runtime String↔Number Conversion Primitives | 2026-06-26 | Moves String-crossing conversions into shared C++ core-runtime symbols so interp and a future native backend can't diverge on float→string formatting. |
| [FD-045](archive/FD-045_CATALOG_METADATA_DECOUPLING.md) | Catalog Metadata Decoupling | 2026-06-25 | Splits catalog metadata (name/symbol/signature) from the executable binding (`fn_ptr`) so a native backend can type-check and emit calls without linking the Allegro runtime. |
| [FD-044](archive/FD-044_BACKEND_TRAIT_SEAM.md) | Backend Trait Seam | 2026-06-25 | Replaces the driver's `cfg`-gated match arms with a real `Backend` trait in a new `cb-backend-api` crate; a new backend is now a crate impl + one factory line. |
| [FD-042](archive/FD-042_DEFAULT_TYPE_INFERENCE.md) | Default Type Inference for Implicit Declarations | 2026-06-23 | A sigil-less, `As`-less first assignment infers the variable's type from the value; `For` vars infer from their bounds. New E0331 when no type can be inferred. |
| [FD-041](archive/FD-041_SOUND_RUNTIME_FUNCTIONS.md) | Sound Runtime Functions and Types | 2026-06-23 | Sample-based audio subsystem (6 commands, opaque `Sound`/`SoundChannel` types) in `cb_sound.cpp`, gated behind the `CB_NO_ALLEGRO` switch. Unblocks Video. |
| [FD-040](archive/FD-040_FILE_IO_RUNTIME_FUNCTIONS.md) | File I/O Runtime Functions and Types | 2026-06-21 | File-I/O subsystem (31 commands, opaque `File` type) in Allegro-free `cb_file.cpp`; lenient EOF reads, little-endian wire format, traps on bad handles. |
| [FD-039](archive/FD-039_MEMORY_BLOCK_RUNTIME_FUNCTIONS.md) | Memory Block Runtime Functions | 2026-06-21 | Memory-block subsystem (13 functions, opaque `Memblock` type) in Allegro-free `cb_memblock.cpp`; little-endian Peek/Poke, traps on OOB/null. |
| [FD-038](archive/FD-038_PARTICLE_SYSTEM_RUNTIME_FUNCTIONS.md) | Particle System Runtime Functions | 2026-06-20 | Particle "Effects" subsystem; an emitter *is* an `Object` (no new type), simulation in Allegro-free `cb_particle.h`. Non-emitter handles trap. |
| [FD-037](archive/FD-037_RUNTIME_CODE_CLEANUP.md) | C++ Runtime Code Cleanup — `extern "C"` Hygiene, Namespaces, Comments | 2026-06-20 | Form-only cleanup of the C++ runtime: `extern "C"` diet, per-subsystem `cb::*` namespaces, comment de-porting, `k_snake_case` constants. No behavior/ABI change. |
| [FD-036](archive/FD-036_RUNTIME_GAME_OBJECTS.md) | Game-Object Runtime Cluster — Multi-frame Images, Camera, Tile Maps, Objects & Game Loop | 2026-06-20 | Multi-frame Images, Camera, Tile Maps (`Map` tag 14), Objects (`Object` tag 13, 57 entry points) and the game loop across 5 phased PRs. Opaque raw-pointer handles. |
| [FD-025](archive/FD-025_DRIVER_BACKEND_SELECTION_AND_EXIT_CODES.md) | Driver CLI, Backend-Selection & Exit-Code Correctness | 2026-06-19 | clap-based CLI, lazy backend resolution (dump works backend-less), `--backend llvm`→exit 3, exit-code clamp; exit policy centralized in an `exit` module. |
| [FD-026](archive/FD-026_INTERNER_SPEC_COMPLIANCE.md) | Identifier Interner Spec Compliance | 2026-06-19 | Interner keys on Unicode simple case folding (not lowercasing) and preserves original spelling; intrinsic dispatch now matches on `fold(name)`. |
| [FD-031](archive/FD-031_DIAGNOSTIC_ASSERTION_SWEEP.md) | Diagnostic Assertion Sweep | 2026-06-18 | Every defined diagnostic code now has a test or a documented reason; implemented E0311 (type-as-value), retired E0207. |
| [FD-035](archive/FD-035_TYPE_SYSTEM_SIMPLIFICATION.md) | Type System Simplification — Classic Types + Long | 2026-06-17 | Scalar set is now Byte/Short/Int/Long/Float/String; booleans are just Int; UInt/Bool dropped as types (reserved → E0330). Supersedes the original FD-035. |
| [FD-032](archive/FD-032_INTERPRETER_HARDENING_TESTS.md) | Interpreter Hardening Tests | 2026-06-17 | Direct tests for untested interp paths, plus a first-class function-address feature (`InstKind::FuncAddr`, E0329). Spun off FD-035. |
| [FD-033](archive/FD-033_CATALOG_MOCK_FOR_SDK_FREE_TESTS.md) | Catalog Mock for SDK-Free Testing | 2026-06-16 | `cargo test --workspace` runs with only Rust + a C++ compiler by guarding the graphics catalog rows behind `CB_NO_ALLEGRO`; Linux CI added. |
| [FD-024](archive/FD-024_RUNTIME_FFI_ABI_HARDENING.md) | Runtime FFI ABI-Handshake & Catalog-Decode Hardening | 2026-06-16 | `runtime_init` returns `Result` and validates the handshake live; `decode_catalog` split out and hardened (duplicate tags / bad UTF-8 now error). New `CB_HOST_ABI_VERSION`. |
| [FD-034](archive/FD-034_SEMA_LOWERING_CORRECTNESS_FD030_FINDINGS.md) | Sema/Lowering Correctness — FD-030 Findings | 2026-06-16 | Fixes the three FD-030 defects: array-of-structs element type, `Delete` on field/index, For-Each over rank ≥ 2 (new `ArrayTotalLen`/`GetElementFlat`). |
| [FD-030](archive/FD-030_SEMA_LOWERING_SNAPSHOT_COVERAGE.md) | Sema Lowering Snapshot Coverage | 2026-06-15 | 20 new `insta` lowering snapshots pinning every major construct (`lower.rs` 53.8%→80.56%). Surfaced the three defects fixed in FD-034. |
| [FD-027](archive/FD-027_RUNTIME_COMMAND_NAME_COLLISION_DIAGNOSTIC.md) | Runtime-Command Name Collisions Produce an Unrenderable Diagnostic | 2026-06-04 | `Dim box As Int` (Box = runtime command) no longer swallows the error: the renderer degrades synthetic spans, and explicit decls may shadow commands (new E0328 for implicit). |
| [FD-023](archive/FD-023_IR_VERIFIER_HARDENING.md) | IR Verifier Hardening | 2026-06-03 | Hardens four CFG invariants (dense blocks, `body_index` bijection, operand-before-result, documented dominance) and backfills verifier + printer tests. |
| [FD-022](archive/FD-022_RUNTIME_ALLEGRO_LAYER_FIXES.md) | C++ Runtime Allegro-Layer Fixes | 2026-06-03 | Fixes a font use-after-free, compiles the fontconfig path, folds in render-target/OOM cleanups, and stands up the first native gtest target (`ctest`). |
| [FD-029](archive/FD-029_RUNTIME_DEFINED_CONSTANTS.md) | Runtime-Defined Constants | 2026-06-03 | Adds a runtime constants table to the catalog (ABI v5→v6): `On`/`Off`/`PI` + key scancodes from a single `cb_keys.def` X-macro. Folds at compile time (no IR change). |
| [FD-021](archive/FD-021_PARSER_AND_SPAN_PANIC_SAFETY.md) | Parser & Span Panic-Safety | 2026-06-02 | Recursion guard (E0218) + eliminated Pratt-table panics, bounds-checked span slicing, AST-print depth cap — restores "never abort on untrusted input". |
| [FD-020](archive/FD-020_SEMA_NUMERIC_AND_FOR_LOOP_SEMANTICS.md) | Sema Numeric & For-Loop Semantics | 2026-06-02 | `For` bounds coerced to the loop-var type, integer-literal overflow → E0326, `^` always Float; fixed interp `Int(Float)` rounding (round half away from zero). |
| [FD-028](archive/FD-028_SYNTAX_FIDELITY_FOR_LEGACY_CODE.md) | Syntax Fidelity for Legacy Code (`\`/`.` field access, `^` exponent, `'` comments) | 2026-06-02 | Realigns the frontend with real CoolBasic: `\`/`.` are field accessors (not int-div), `^` is exponent, `'` starts a comment, unary `+` is Abs. |
| [FD-019](archive/FD-019_INTERPRETER_CORRECTNESS_FIXES.md) | Interpreter Correctness & Memory-Safety Fixes | 2026-06-01 | Four interp bugs: width-correct shifts, value-struct writes wired end-to-end (`StorePlace`), array-dim overflow guards, struct-array defaults. Spun off FD-027. |
| [FD-018](archive/FD-018_RUNTIME_TEXT_AND_FONT_SUPPORT.md) | Runtime Text and Font Support | 2026-05-31 | Text & Fonts subsystem (new `Font` type tag 12, 11 commands) via `cb_font`/`cb_gfx`; null opaque returns now map to `Value::Null`. |
| [FD-017](archive/FD-017_RUNTIME_MODULE_COMPLETENESS.md) | Runtime Module Completeness Pass | 2026-05-31 | Completeness pass over the six shipped runtime modules (String/Math/System/Graphics/single-frame Images/Input) to match the CoolBasic runtime surface. |
| [FD-015](archive/FD-015_RUNTIME_TRAP_CHANNEL.md) | Runtime Trap Channel | 2026-05-31 | Backend-agnostic channel for a runtime function to ask the host to exit or raise an error (records intent, never unwinds). `cb_runtime_init` handshake; ABI v4→v5. |
| [FD-016](archive/FD-016_RUNTIME_CORE_FUNCTIONALITY_SPLIT.md) | Runtime Core / Functionality Split | 2026-05-31 | Splits the C++ runtime into Allegro-free `cb_runtime_core` (plugin SDK: string + catalog structs) and `cb_runtime` (functionality + Allegro). Structural only. |
| [FD-013](archive/FD-013_EXTENDING_RUNTIME_SUPPORT.md) | Extending Runtime Support | 2026-05-30 | Ports five runtime subsystems (Math, String, System/Time, Graphics & Images, Input) as `CB_FN` catalog batches; adds `Terminator::Halt` + exit codes. |
| [FD-014](archive/FD-014_RUNTIME_STRING_ABI.md) | Runtime String ABI | 2026-05-28 | Catalog ABI v4: strings flow as opaque refcounted `CbString*` (port of legacy `LString`); interp `Value::String` becomes a RAII handle. |
| [FD-012](archive/FD-012_CATALOG_CPP_TEMPLATE_DSL.md) | Catalog DSL via C++ Templates and Function Pointers | 2026-05-26 | Catalog ABI v3: `catalog.cpp` with a `FuncTraits<auto Fn>` template DSL (`CB_FN`); interp dispatches runtime calls via libffi. |
| [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) | Runtime Custom Types | 2026-05-24 | Catalog ABI v2: opaque runtime types (`IrType::RuntimeType`, `Value::OpaqueHandle`); supports assignment/null/identity, rejects arithmetic/field access. |
| [FD-010](archive/FD-010_INTERPRETER_BACKEND.md) | Interpreter Backend Implementation | 2026-05-24 | `cb-backend-interp`: stack-based interpreter over `cb_ir::Program`; 14-variant Value enum, slab Type-instance heap, generic Observer trait. |
| [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) | Runtime Library Architecture | 2026-05-24 | Catalog-only milestone: `cb-runtime-sys` with `cc` build + FFI bindings; overload resolution and `FuncId`-based dispatch in sema/lower/IR. |
| [FD-008](archive/FD-008_IR.md) | Intermediate Representation | 2026-05-24 | `cb-ir` crate: Program/Function/BasicBlock/Inst, AST→IR lowering with full control-flow desugaring, text printer, debug-build verifier. |
| [FD-007](archive/FD-007_Semantic_Analysis.md) | Semantic Analysis | 2026-05-23 | `cb-sema`: two-pass analysis, 22 diagnostics (E0300–E0321), implicit conversions, const folding, intrinsics, `Delete` classification. |
| [FD-006](archive/FD-006_DIAGNOSTICS_DRIVER_HARDENING.md) | Diagnostics & driver hardening | 2026-05-23 | Driver backend cargo features (interp/llvm/dump-only), AST printer moved to `cb-frontend` with exhaustive arms, renderer span validation. |
| [FD-005](archive/FD-005_DELETE_STATEMENT.md) | `Delete` statement (§3.3) | 2026-05-23 | Added `Kw::Delete`/`Stmt::Delete`/`parse_delete`; classification + trap diagnostics deferred to sema. |
| [FD-004](archive/FD-004_PARSER_CORRECTNESS.md) | Parser correctness & small spec gaps | 2026-05-23 | Closed 17 post-FD-002 review issues (line-continuation transparency, empty-`If` diagnosis, implicit decls, recovery hardening, E0299 promotion). |
| [FD-003](archive/FD-003_LEXER_CORRECTNESS.md) | Lexer correctness & robustness pass | 2026-05-23 | Closed 10 post-FD-001 review issues (UTF-8 recovery, `IntLit→u64`, `FloatBits` newtype, hex/binary UX, invariant tests). |
| [FD-002](archive/FD-002_PARSER.md) | Parser | 2026-05-21 | Hand-written recursive descent + Pratt, arena-allocated AST, recovering on `Newline`/`Colon`/`End*`. |
| [FD-001](archive/FD-001_LEXER.md) | Lexer | 2026-05-17 | Hand-written recovering lexer + `cb-diagnostics` crate. |
