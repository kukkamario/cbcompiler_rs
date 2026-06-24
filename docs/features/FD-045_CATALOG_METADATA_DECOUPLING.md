# FD-045: Catalog Metadata Decoupling

**Status:** Planned
**Created:** 2026-06-24
**Priority:** Medium (prerequisite for a runtime-free native compiler; not blocking interp work)
**Effort:** High (build-system + ABI surface change; needs a design pass)
**Impact:** Lets semantic analysis and a future native/AOT (LLVM) compiler obtain the runtime function/type/constant **catalog as metadata** (symbol name + signature) **without linking the executable C++ runtime**. Today every binary that type-checks must link the full C++/Allegro runtime just to read the catalog — this is the real coupling behind "a native compiler that doesn't need the runtime to run," not the executable-topology question.

> Surfaced by FD-044's `/fd-deep` executable-topology analysis (2026-06-24). The deep analysis concluded that splitting the compiler into separate native/interp executables would **not** shed the runtime dependency, because the catalog (a sema input) is only obtainable from the linked runtime. This FD captures the decoupling that actually unblocks a lightweight native compiler. Filed **Planned** — it is a load-bearing build/ABI decision that needs its own design pass (CLAUDE.md: ask before load-bearing structural choices).

## Problem

Semantic analysis requires the **runtime catalog** — the list of runtime-provided functions, opaque types, and constants with their signatures — so it can resolve builtin calls, check argument types, and seed runtime constants. `cb_sema::analyze(... runtime_catalog: &RuntimeCatalog)` takes it as input (`crates/cb-sema/src/lib.rs`), and the driver loads it on every non-`--dump-ast` path (`crates/cb-driver/src/main.rs:245,261`).

The **only** way to obtain a real catalog today is to link the C++ runtime and call into it at runtime:

- `cb-runtime-sys` declares `extern "C" fn cb_runtime_get_catalog() -> *const CbCatalog` (`crates/cb-runtime-sys/src/lib.rs:168-169`); `load_catalog()` → `fetch_catalog()` calls it and decodes the result (`lib.rs:190-196,266-268`). The comment is explicit: *"The catalog is the only entry point Rust needs from the runtime library"* (`lib.rs:165-167`).
- The catalog is assembled in C++ as static arrays in `runtime/catalog.cpp` (`catalog_funcs[]`, `catalog_types[]`, `catalog_consts[]`), via the `CB_FN(cb_name, fn)` template DSL (`catalog.cpp:206-216`, FD-012).
- Critically, each `CB_FN` entry stores **`reinterpret_cast<void(*)(void)>(fn)`** — a live function pointer (`catalog.cpp:209`, mirrored as `CbFuncDesc::fn_ptr` in `crates/cb-runtime-sys/src/lib.rs:33-37` and `cb_ir::FuncDesc::fn_ptr`). Taking that address **ODR-uses** the function, so producing the catalog forces the entire runtime (and its Allegro transitive closure) to be linked.

Consequence: **any** executable that runs sema links the full C++ runtime — even a pure type-check, a `--dump-ir`, or a future native compiler that never executes a single runtime function. But a native/AOT backend needs only **metadata**: the CB name, the linker **symbol** string (`CbFuncDesc::symbol`, the `#fn` stringification at `catalog.cpp:208`), and the signature (param/return `IrType`s) — enough to emit LLVM `declare`/`call`. The IR already encodes this split: `FuncKind::Runtime { symbol, fn_ptr }` with the note that *"the interpreter dispatches through [`fn_ptr`]; the LLVM backend uses `symbol` for declare/call"* (`crates/cb-ir/src/lib.rs:42-50`). Only the interpreter — which executes runtime functions in-process via libffi — actually needs `fn_ptr`.

The signature data does **not** force linking: `FuncTraits<fn>` / `cb_anon_params<fn>` derive param/return tags from the function **type** (`decltype`), not its address. Only the `fn_ptr` field does. So a metadata-only catalog is mechanically feasible.

## Goal

Provide the catalog **metadata** (functions: name + symbol + param/return types; opaque types: name + tag; constants: name + type + value) to the compiler **without** linking the executable runtime, while keeping the **C++ catalog DSL as the single source of truth** (no hand-maintained Rust duplicate — that is exactly what FD-012's DSL exists to avoid). The interpreter keeps using the full catalog (with `fn_ptr`) by linking the runtime, as today.

Concretely, after this FD:

- `cb-sema` / a native compiler can type-check and emit `declare`/`call` from metadata alone — no C++/Allegro toolchain required for that path.
- `cb-backend-interp` still resolves `fn_ptr` by linking the runtime (it must, to execute in-process).
- The C++ `CB_FN(...)` line stays the one place a runtime function is registered.

## Solution (sketch — to be designed)

Candidate approaches, to evaluate in the design pass:

1. **Build-time metadata generation from the C++ catalog (recommended lean).**
   A small metadata emitter compiled from `catalog.cpp` under a `-DCB_METADATA_ONLY` define that builds the descriptor arrays with `fn_ptr` set to `nullptr` (omitting the `reinterpret_cast<void(*)(void)>(fn)`), so it references no function addresses and links with only `catalog.cpp` + headers — not the subsystem TUs or Allegro. It serializes name/symbol/tags/constants to a stable format (JSON or a packed table) that `cb-runtime-sys`'s `build.rs` turns into static Rust data (e.g. a generated `catalog_meta.rs`). Sema consumes the generated metadata; the interpreter overlays `fn_ptr`s from the linked runtime by matching on `symbol`. **Pro:** C++ stays source of truth; metadata path needs no runtime link. **Con:** a second build artifact + a symbol-keyed reconciliation step for interp.

2. **Two catalog entry points in the linked runtime.** Add `cb_runtime_get_catalog_meta()` returning `fn_ptr`-free descriptors. **Rejected for the stated goal:** it still lives in the linked library, so it does *not* decouple — no win for a runtime-free compiler. (Could still be a cheap intermediate step.)

3. **Promote the catalog DSL to a language-neutral description** (e.g. a `.def`/data file, like the existing `cb_keys.def` at `runtime/cb_keys.def`) that *both* the C++ build and a Rust generator read. **Pro:** cleanest single source. **Con:** largest rewrite; loses the C++ `FuncTraits` type-deduction that currently makes signatures impossible to mis-declare.

Data-model change common to all: split `cb_ir::FuncDesc` (and the `CbFuncDesc` mirror) so the **metadata** (name, c_symbol, params, return_ty) is separable from the **execution binding** (`fn_ptr`). `RuntimeCatalog` becomes constructible from metadata alone; `fn_ptr` becomes an interp-only overlay resolved by `symbol`.

## Open questions (design pass)

- **Target-vs-host catalog content.** `catalog.cpp` content is **build-config-dependent** — graphics/audio rows are behind `#ifndef CB_NO_ALLEGRO` (`catalog.cpp:229-244`), and the SDK-free build emits a smaller catalog (FD-033). A native compiler's metadata must describe the **runtime it links its output against**, which may differ from the host build (and, eventually, cross-compilation targets). How is the metadata config/target selected and versioned? Does `CB_CATALOG_VERSION` need to gate metadata too?
- **`fn_ptr` reconciliation for interp.** If metadata is `fn_ptr`-free and the interpreter overlays addresses from the linked runtime by `symbol`, the two must agree exactly. How is drift detected (the current DSL ties symbol↔pointer so they "cannot drift" — `catalog.cpp:205`; a split reintroduces that risk)? A startup assert that every metadata symbol resolves?
- **Where does generated metadata live / how is it tested?** `cb-runtime-sys` `OUT_DIR` vs a committed artifact; how does the existing catalog **mock** (FD-033) coexist with generated metadata in SDK-free CI?
- **Scope vs. FD-044.** FD-044 ships first and is independent; this FD is only needed once a native backend wants to avoid the runtime link. Confirm sequencing (likely: FD-044 → LLVM codegen begins → this FD when the runtime-link weight actually bites).

## Files to Create/Modify (preliminary)

| File | Action | Purpose |
|------|--------|---------|
| `runtime/catalog.cpp` | MODIFY | `CB_METADATA_ONLY` path (or a metadata emitter) producing `fn_ptr`-free descriptors |
| `crates/cb-runtime-sys/build.rs` | MODIFY | Generate static Rust catalog **metadata** at build time (approach 1) |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | Expose metadata without `cb_runtime_get_catalog()`; keep `fn_ptr` overlay for interp |
| `crates/cb-ir/src/lib.rs` | MODIFY | Split `FuncDesc` metadata vs. `fn_ptr` execution binding; `RuntimeCatalog` constructible from metadata alone |
| `crates/cb-backend-interp/*` | MODIFY | Resolve `fn_ptr` from the linked runtime by `symbol` |
| `crates/cb-sema/*` | (likely no change) | Already consumes `RuntimeCatalog`; benefits transparently |

## Verification (preliminary)

- A build/test that type-checks a program (sema) **without** linking the executable runtime (e.g. metadata-only, `--dump-ir` path) — proving the C++/Allegro link is no longer a type-check prerequisite.
- Interpreter still runs every existing fixture — `fn_ptr` overlay resolves all symbols; a startup check fails loudly on any missing/extra symbol (drift guard).
- Catalog metadata matches the linked runtime's catalog under both full-Allegro and SDK-free configs (`HAS_GRAPHICS` gating preserved).
- `cargo test --workspace` green across the FD-025 four-feature matrix; clippy/fmt clean.

## Related

- [[FD-044]] Backend Trait Seam — surfaced this FD; the executable-topology decision identifies catalog decoupling (not a binary split) as the real native-compiler prerequisite.
- [[FD-012]] Catalog C++ Template DSL — the `CB_FN`/`FuncTraits` source of truth this must preserve.
- [[FD-009]] Runtime Library — documents the intended LLVM-vs-interp runtime relationship (interp links + dispatches; LLVM declares symbols + links the runtime into its *output*).
- [[FD-033]] Catalog Mock for SDK-Free Tests — existing mechanism for catalogs without the full toolchain; must coexist with generated metadata.
- [[FD-029]] Runtime-Defined Constants — constants are part of the catalog metadata this decouples.
- `crates/cb-ir/src/lib.rs` — `FuncKind::Runtime { symbol, fn_ptr }`, `FuncDesc`; the data-level split point.
