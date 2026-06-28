# FD-049: IR → LLVM Lowering

**Status:** In Progress (Phase 1 complete)
**Priority:** High
**Effort:** High (> 4 hours — multi-phase roadmap; each phase is planned in detail in place, then implemented as its own commit(s))
**Impact:** Makes the LLVM/AOT backend actually *compile programs*. Today it emits a fixed empty `main` (FD-048); this FD walks `cb_ir::Program` and generates real native code, so `cb --backend llvm prog.cb` produces an exe that behaves like `--backend interp`.

## Problem

FD-047 wired in `inkwell`/LLVM 18 and FD-048 stood up the output half of the AOT pipeline — in-memory module → native object → CRT-aware driver link against the full runtime closure → runnable exe. But the module body is hardcoded: `i32 @main() { ret i32 0 }`. The IR is accepted and ignored.

The interpreter (`cb-backend-interp`) is the reference implementation and already executes the full IR surface. The remaining gap for a working second backend is the **instruction-selection middle**: translate each `InstKind` / `Terminator` / `IrType` into LLVM IR, against a native memory + runtime ABI that matches what the interpreter marshals to today. Until that exists the project has one real backend, not the two-from-day-one the architecture calls for.

This is the single largest remaining piece of the compiler. This FD is the **high-level roadmap**; it fixes the cross-cutting architecture and decisions once, then splits the work into phases. Each phase is detail-planned in place (expanding its section below) immediately before it's implemented as its own commit(s), and validated against the interpreter.

## Approach

A new lowering pass in `cb-backend-llvm` (gated by the existing `codegen` feature) consumes `&Program` + `&Interner` and builds the LLVM module, replacing the hardcoded `emit_empty_main`. The object-emit + link back-half (FD-048) is unchanged — only the module body becomes IR-driven.

The interpreter is the oracle throughout: the native backend is **not** the reference, so every phase is validated by running the same `.cb` through both backends and comparing stdout + exit code.

### Cross-cutting architecture (fixed once, here)

- **Translation skeleton.** A two-pass walk of the `Program`: first *declare* all functions, runtime symbols, globals, and the per-function block structure (so forward references — calls, back-edges, function addresses — resolve); then *fill* each function body by translating its blocks' instructions and terminators, threading an SSA-style register→value map. The IR is already CFG-shaped and verified (FD-023), so no control-flow reconstruction is needed — this is a structural walk, not an optimization pass (LLVM's own passes handle quality).
- **Type mapping.** Scalars map directly (`Byte/Short/Int/Long` → integer widths, `Float` → `double` — the runtime ABI and the interpreter already use `double` end-to-end). `String`, `RuntimeType`, arrays, and type-instances are pointers (`RuntimeType` is pointer-sized, and a null pointer must compare equal to CB `Null`); `StructVal` is an inline aggregate (value semantics — cf. `StorePlace`'s in-place write rationale). The easy-to-miss tail of the enum: `Void` → LLVM void (return position — every `Sub`/`Print` returns it, so it's needed from Phase 1); `Null` → a poly-null pointer (the type of `ConstNull`); `FnPtr` → a function-pointer type (materialized in Phase 3 via `FuncAddr`/`CallIndirect`).
- **Memory model — C-ABI heap helpers (decided; native-only).** Strings are *already* shared C: `CbString*` with retain/release lives in the core runtime and both backends use it. Arrays, type-instance linked lists, and value-structs, by contrast, exist today only as Rust data structures inside the interpreter (`heap.rs`) — there is no C++ counterpart. This FD adds **new C-ABI heap helpers** (allocation, indexing, linked-list walk, field address) that the **LLVM backend** calls; it does **not** re-point the interpreter onto them — the interpreter keeps its Rust heap, preserving its observability (a project ground rule). The two backends therefore share a *specification*, not (for arrays/type-instances) the *code*; divergence is caught by the differential harness (below), not prevented structurally. Strings remain the one type where the C code is genuinely shared. Only trivial fixed-offset value-struct fields are emitted inline rather than via a helper.
- **Program entry + runtime lifecycle.** The runtime is stateful. The emitted native entry point brackets the program the way the interpreter *backend* does (`Interpreter::new`/`run`, not the driver): construct a host-API vtable, hand it to `cb_runtime_init`, run the IR's synthetic `@main`, run the `about_to_exit` teardown hook (FD-043), and return the exit code. The handshake is **not** parameterless — `cb_runtime_init(host: *const CbHostApi)` requires a `CbHostApi { size, abi_version = CB_HOST_ABI_VERSION, request_exit, raise_error }` and the runtime panics/null-derefs without a valid one (there is no default host). So the backend supplies (as an emitted/linked C shim) working `request_exit`/`raise_error` callbacks and checks the returned hook table. These callbacks are **no-return**: on a runtime-requested exit or error they flush stdout, run `about_to_exit`, print any message to stderr, and `exit(code)` — control never returns to the runtime function. Because the runtime's error path is thus no-return from its own point of view, the generated code needs **no per-call trap poll or flag check** (the interpreter must record-and-return instead, because it can't unwind a live C call — the AOT exe just exits). Globals are default-initialized at startup, including non-scalar globals (an empty `CbString` for `String` globals, `Null` for reference types).
- **Runtime calls.** `FuncKind::Runtime { symbol }` lowers to a direct call of the declared linker symbol with the C ABI from its `FnSig` — the static analogue of the interpreter's libffi marshalling (`Byte`/`Short` at their native 8/16-bit width and signedness, `Float` as `double`, `String` as a borrowed `CbString*`, returns owned). The full runtime closure is already linked (FD-048), so symbols resolve. `Print` parity caveat: the interpreter *intercepts* `cb_rt_print` and writes to its own sink, whereas the native exe calls the real C `cb_rt_print` — so the differential harness compares the C symbol's bytes + flush ordering against the interpreter's intrinsic, not two copies of one implementation.

## Phases

Each phase below is a roadmap entry. Before implementing one, expand its bullet into a detailed plan **in place** (instruction-by-instruction lowering, exact runtime helpers, edge cases), then implement it as its own commit(s) and add it to the differential test set. Order is dependency-driven; each phase keeps earlier phases green.

### Phase 1 — Scalar core + runtime calls + strings *(first runnable, observable milestone)*

Stands up the whole skeleton and produces programs with **observable stdout** so the differential harness has real output to compare (scalar-only programs aren't observable — `End` always exits 0). Scope: the translation skeleton and type map; scalar `BinOp`/`UnOp`/`Const*`; numeric conversions `Convert` **and** `ConvertExplicit` — including number→string (`Str`), which routes through the shared `cb_rt_*_to_string` symbols (FD-046) and is on the critical path for any numeric program's output; locals and globals (with non-scalar default-init); all terminators except `Trap`; user-function `Call`/`Return`/`Halt`; the entry-point host-API handshake + init/teardown bracket + no-return `request_exit`/`raise_error` shim; runtime-call dispatch by symbol; `ConstString`, string ops (`StrConcat`/`StrLen`/string comparisons), explicit string refcount management (no `Drop` to lean on); and enough runtime surface to make `Print` work. *(The string-refcount lifetime strategy — retain/release placement — is settled in this phase's detailed plan.)*

**Phase 1 — implemented.** Lowering lives in `cb-backend-llvm/src/codegen/{mod,types,regtypes,runtime,func}.rs` and replaces `emit_empty_main` (now the lowering-agnostic `emit::write_module`). Verified by the `diff_llvm` differential suite (16 fixtures — scalar arithmetic, float formatting, control flow, user functions/recursion/mutual-recursion, strings, and Allegro-free Math/String/System runtime calls) plus the `cb-backend-llvm` codegen unit/smoke tests, all green against the interpreter oracle. Three items deferred to this phase are now settled:

- **Decision A — AOT lifecycle is a linked C shim, not emitted LLVM.** `runtime/cb_standalone.cpp` (a core, Allegro-free TU) supplies `cb_rt_standalone_run` (build the default `CbHostApi`, run the `cb_runtime_init` handshake, invoke `cb_user_main`, then exit) and the no-return `cb_rt_exit` (latched `about_to_exit` teardown → libc `exit`, which flushes piped stdout). The backend emits only `int main() { cb_rt_standalone_run(cb_user_main); return 0; }`. The default host's `request_exit`/`raise_error` are the no-return callbacks the roadmap requires.
- **Decision B — string-refcount discipline.** Producers own +1; a String `LoadLocal`/`LoadGlobal` retains; stores release-old-then-move-in; call args borrow; String params are retained into their slot and every String local slot is released at each `Return`. An owned String temp that is neither consumed (stored/returned) nor escapes its defining block is released right after its last in-block use (scheduled by the `regtypes` pass). Temps that escape a block or are dead are conservatively leaked — safe for Phase 1.
- **Decision C — one ordering oracle.** A new bare `cb_rt_string_compare` (lexicographic unsigned-byte compare, null = empty, normalized −1/0/1) in `cb_string.cpp` backs both the native `Str{Eq,NotEq,Lt,Gt,LtEq,GtEq}` lowering and the interpreter's string relations (repointed off the old inline `as_bytes()` compares), so the two backends cannot diverge on ordering. `cb_rt_string_char_len` likewise backs `StrLen` (codepoint count, not byte length).

Two scoped simplifications: String globals are null-initialized rather than given an empty `CbString` (every string primitive null-checks, and top-level `Dim` lowers to `@main` locals, so real String globals are rare); and `Trap` — a Phase-4 concern — lowers to a clean non-zero exit rather than UB.

### Phase 2 — Arrays

`NewArray`, `Redim`/`RedimGlobal`, `GetElement`/`GetElementFlat`, `Len`/`ArrayTotalLen`, and the index projections of `StorePlace`, plus out-of-bounds handling. Built on the new C-runtime array helpers (memory-model decision). `StorePlace` path-walking is introduced here for pure-index paths and **extended in Phase 3** for mixed `arr[i].field = v` paths (which also need field projections) — so a fully general `StorePlace` isn't complete until Phase 3.

### Phase 3 — User types & structs

`NewType`; field access (`GetField`); the type-instance linked list (`First`/`Last`/`Next`/`Previous`); `DeleteLvalue`/`DeleteLvalueGlobal`/`DeleteRvalue`; the field projections of `StorePlace` (completing the mixed `arr[i].field = v` paths begun in Phase 2); value-struct field access; and first-class function pointers (`FuncAddr`/`CallIndirect`). Type-instance allocation/walk via the new C-runtime helpers.

### Phase 4 — Traps

`Terminator::Trap(kind)` and the runtime checks that reach it (null deref, deleted access, division by zero, index out of bounds, null/double-delete), lowered to match the interpreter's trap messages and exit codes. These are sema-emitted *hard* faults in the IR's control flow — distinct from the runtime's cooperative `request_exit`/`raise_error`, which Phase 1 already handles via the no-return host callbacks.

## Crates & Areas Touched

High-level only — the precise module/file breakdown is part of each phase's detailed plan.

| Area | Action | Purpose |
|------|--------|---------|
| `cb-backend-llvm` (codegen) | CREATE/MODIFY | IR→LLVM lowering pass + type map + runtime-call ABI + entry-point host-API handshake/init-teardown bracket + no-return `request_exit`/`raise_error` shim; replaces `emit_empty_main`. Grows phase by phase. |
| `cb-backend-llvm::execute` | MODIFY | Pass `program`/`interner` into the lowering instead of ignoring them. |
| `runtime/` (C++ core) | MODIFY | **New, native-only** C-ABI heap helpers (array + type-instance alloc/index/walk/field-address) the LLVM backend calls; the interpreter is **not** re-pointed onto them. Strings already shared. |
| `cb-driver/tests` (`run_diff`) + fixtures | CREATE | Build the interp-vs-llvm differential harness (extends the existing `programs.rs` stdout-golden infra); per-phase `.cb` fixtures under `tests/fixtures/programs/` (not `examples/`). |
| `.github/` CI | MODIFY | New Linux job: LLVM 18 + SDK-free `cb-runtime-sys` + driver `--features llvm`, runs the diffs headlessly (`Print`/`Str` are Allegro-free). Workspace default stays LLVM-free. |

## Verification

- **Differential against the interpreter (primary).** A new `run_diff` harness — extending the existing `crates/cb-driver/tests/programs.rs` stdout-golden infra (interp-only today) — runs each `.cb` through `--backend interp` and `--backend llvm` and asserts identical stdout + exit code. The interpreter is the oracle; any divergence is an LLVM-backend bug by definition. The corpus is the pure scalar/string/Print fixtures under `tests/fixtures/programs/` (not the windowed `examples/`); each phase adds fixtures for its new instructions and keeps them in the set so later phases can't regress earlier ones.
- **CI.** Add a dedicated Linux job (LLVM 18 + SDK-free `cb-runtime-sys` + driver built `--features llvm`) running the diffs headlessly — `Print`/`Str` are Allegro-free, so no display/vcpkg needed. Without it the differential gate is dev-machine-only; with it the workspace default still stays LLVM-free (no `--all-features` on the default job — unchanged rule).
- `cargo test -p cb-backend-llvm --features codegen` for lowering unit/smoke tests; the FD-048 emit→link→run smoke test must keep passing.
- Default `cargo build` / `cargo test --workspace` stay LLVM-free.
- `bounce.cb` and the other `examples/*.cb` are **windowed/looping with no stdout** — not automated differential targets; keep `bounce.cb` only as a manual Windows visual smoke against the full Allegro closure.

## Decisions (resolved for this roadmap)

- **Memory model:** **native-only** C-ABI heap helpers for arrays/type-instances, called by the LLVM backend only; the interpreter keeps its Rust heap (strings remain the one genuinely shared C type). Inline only fixed-offset struct fields. *(Preserves interpreter observability; divergence is caught by the differential harness, not by shared code.)*
- **Runtime error channel:** the backend's `request_exit`/`raise_error` host callbacks are **no-return** (flush → teardown → exit), so the generated code does **no** per-call trap polling or flag-checking. *(Simpler than the interpreter's record-and-return; valid because the AOT exe is itself the program and may exit immediately.)*
- **First milestone:** Phase 1 includes strings + `Print` (writes to stdout via `cb_rt_print`), so the differential harness has observable output from the start. *(Scalar-only programs exit 0 with nothing to compare.)*
- **Phase tracking:** kept inside this FD; each phase's detail is expanded in place before its commit, rather than spun into separate FDs.
- **Resolved in Phase 1** (see the *Phase 1 — implemented* note): the string-refcount lifetime strategy is decision B; the host-API `request_exit`/`raise_error` shim is a small linked C file (`cb_standalone.cpp`), not emitted LLVM (decision A).

## Related

- [FD-048](archive/FD-048_BASIC_LLVM_CODEGEN_AND_TOOLING_DRIVER.md) — the object-emit + link back-half this builds on (and the `emit_empty_main` it replaces).
- [FD-047](archive/FD-047_LLVM_DEPENDENCY_SETUP.md) — `inkwell`/LLVM 18 dependency + `codegen` feature gate.
- [FD-044](archive/FD-044_BACKEND_TRAIT_SEAM.md) — the `Backend` trait this dispatches through.
- [FD-045](archive/FD-045_CATALOG_METADATA_DECOUPLING.md) — catalog metadata vs. binding split, so the native backend can emit runtime calls without linking Allegro to type-check.
- [FD-046](archive/FD-046_STRING_NUMBER_CONVERSION_PRIMITIVES.md) / [FD-016](archive/FD-016_RUNTIME_CORE_FUNCTIONALITY_SPLIT.md) — the shared-core-runtime precedent that applies to **strings/conversions** (the genuinely shared C types); arrays/type-instances instead get native-only helpers (see the memory-model decision).
- [FD-043](archive/FD-043_INTERPRETER_TEARDOWN_HOOK.md) / [FD-015](archive/FD-015_RUNTIME_TRAP_CHANNEL.md) / [FD-024](archive/FD-024_RUNTIME_FFI_ABI_HARDENING.md) — runtime init/teardown, the `CbHostApi` handshake, and the trap channel the native entry point must satisfy.
- `cb-backend-interp/src/{ffi,value,heap,string_handle}.rs` — the reference ABI/memory semantics to match.
- `docs/cb_syntax.md` — language semantics (e.g. §6.3 row-major `For Each`, field access).
