# FD-054: LLVM Optimization Passes & `-O` Flag

**Status:** Pending Verification
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** The LLVM/AOT backend actually optimizes. Today it emits whatever the IR→LLVM lowering produces with **both** optimizers off, so native code is needlessly slow despite the LLVM backend existing specifically "for AOT/optimized native codegen."

## Problem

The LLVM backend runs **zero** optimization. Two independent knobs are both off:

1. **IR-level passes** — there is no `Module::run_passes` / pass-manager invocation anywhere in `cb-backend-llvm`. The naive, un-mem2reg'd, un-inlined IR from the lowering goes straight to object emission (every local is a stack `alloca`, no constant folding, no dead-code elimination, no inlining).
2. **Codegen-level optimization** — `emit.rs` creates the `TargetMachine` with `OptimizationLevel::None`, so instruction selection, scheduling, and register allocation also run unoptimized.

There is also no way for a user to request optimization: the driver has no `-O` flag.

## Solution

Add a single optimization-level concept, threaded from the CLI into the backend, that drives **both** knobs together.

**Optimization level type (`cb-backend-llvm`).** Define a small `OptLevel` enum (`O0/O1/O2/O3/Os/Oz`) **unconditionally** in `lib.rs` (not behind `codegen`), so the LLVM-free stub build still compiles `LlvmBackend::new`. It maps to:

- the new-pass-manager pipeline string — `default<O0>` … `default<O3>`, `default<Os>`, `default<Oz>` — passed to `Module::run_passes(passes, &machine, PassBuilderOptions::create())` (inkwell 0.9 / LLVM 18 new PM);
- the inkwell `TargetMachine` `OptimizationLevel`: `O0→None`, `O1→Less`, `O2→Default`, `O3→Aggressive`, and `Os/Oz→Default` (the `TargetMachine` has no size knob — size is expressed purely through the `default<Os|Oz>` pipeline string).

**Where passes run (`emit.rs`).** `write_module` already owns the `TargetMachine`, so it is the natural home. Give it an `OptLevel` parameter: create the machine at the mapped codegen level (instead of the hardcoded `None`), then — when the level is **not** `O0` — call `module.run_passes("default<Ox>", &machine, …)` before `write_to_file`. At `O0`, skip `run_passes` and keep the machine at `None`, preserving today's exact "no transformation" behavior (useful for debugging native output). Pass ordering: lower → `module.verify()` (unchanged) → `run_passes` → emit object.

**Construction injection (`lib.rs`).** `OptLevel` is injected at `LlvmBackend::new(source, output, opt)`, the same pattern already used for `source`/`output` — the `Backend::execute` trait signature carries neither, so they ride on the struct. `build_object`/`write_module` thread the level through.

**CLI (`cb-driver`).** Add a `-O` short flag taking a required level (`-O0`/`-O2`/`-Os` …; value parser accepts `0|1|2|3|s|z`). **Absent ⇒ O2** (the LLVM backend's reason for existing is optimized codegen). A level is required rather than allowing a bare `-O` alias: with a positional `FILE` arg, `cb -O file.cb` would otherwise be ambiguous — requiring the level turns that into a clear "invalid optimization level 'file.cb'" error. The flag is parsed in every build, like `-o`; it only affects the llvm backend and is silently ignored by the interpreter (again like `-o`). Because the driver depends on `cb-backend-llvm` only under the gated `llvm` feature, the CLI parses into a driver-local `OptLevelArg` and converts it to `cb_backend_llvm::OptLevel` inside the existing `#[cfg(feature = "llvm")]` arm of `make_backend`.

Out of scope: link-time optimization (LTO), per-pass tuning flags, and profile-guided optimization. This FD is the standard `-O` pipeline only.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-backend-llvm/src/lib.rs` | MODIFY | Define `OptLevel` (unconditional); add `opt` param to `LlvmBackend::new`; thread into `build_object`. |
| `crates/cb-backend-llvm/src/emit.rs` | MODIFY | `write_module` takes `OptLevel`: set `TargetMachine` opt level (was `None`); run `default<Ox>` pipeline via `run_passes` for non-`O0` before emit. |
| `crates/cb-backend-llvm/src/codegen/mod.rs` | MODIFY | `build_object` accepts and forwards `OptLevel` to `write_module`. |
| `crates/cb-driver/src/main.rs` | MODIFY | Add `-O` flag (default O2), parse `0/1/2/3/s/z`, convert + pass into `LlvmBackend::new` in the llvm arm. |

## Verification

- **Differential suite is the optimization-correctness gate.** `crates/cb-driver/tests/diff_llvm.rs` drives the real `cb` binary via `--backend llvm` over the CLI, so flipping the default to `-O2` makes the existing 54-fixture interp-vs-native suite exercise the optimized path automatically: `cargo test -p cb-driver --features llvm diff_llvm`. Interp (oracle) == native output proves the passes preserve semantics.
- **Level sweep (optional but recommended).** Spot-check a fixture compiled at `-O0` and `-O3` still matches the interpreter, confirming all mapped levels are wired correctly.
- **Smoke / unit:** `cargo test -p cb-backend-llvm --features codegen` (object emit + link path stays green at the new default).
- **Manual:** `cb --backend llvm -O3 examples/...cb` emits a runnable exe; inspect that optimization actually happened (e.g. dump `--dump-ir` is pre-LLVM, so eyeball with `llvm-objdump`/size delta vs `-O0`).

## Related

- `docs/features/archive/FD-049_IR_TO_LLVM_LOWERING.md` — produced the un-optimized IR this FD now optimizes.
- `docs/features/archive/FD-048_BASIC_LLVM_CODEGEN_AND_TOOLING_DRIVER.md` — the emit/link toolchain `-O` plugs into.
- `docs/features/FD-050_OPTIONAL_TRAP_GENERATION.md` — sibling backend-policy knob; trap checks interact with what optimization can assume/elide.
- `CLAUDE.md` → "AOT codegen & linking" — describes the LLVM backend as the optimized-native path.
