# FD-012: Catalog DSL via C++ Templates and Function Pointers

**Status:** Complete
**Completed:** 2026-05-26
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** Adding a runtime function becomes a one-line macro invocation. Symbol names, parameter types, and parameter counts can no longer drift from the C++ implementation — drift becomes a compile/link error rather than a runtime miscall.

## Problem

Each entry in `runtime/catalog.c` requires 3-4 hand-synchronized edits:

1. The C body (in `gfx.c`, `input.c`, or `catalog.c`).
2. A prototype in `cb_runtime.h`.
3. A `CbParamDesc[]` array, with names and type tags written by hand.
4. A `CbFuncDesc{}` entry repeating the symbol name as a string, the param count as an integer, the return type tag, and a pointer to the param array.

In addition, `catalog.c` holds a hand-counted `func_count` (the trailing `13` at line 188 of the current file) and runtime-type tags are bare `#define CB_TYPE_TEST_HANDLE 10` magic numbers.

Failure modes this produces:

- **Symbol-name typo** in the `CbFuncDesc.symbol` string → caught only when the interpreter (or LLVM-emitted binary) tries to dispatch the call.
- **Wrong parameter count** → out-of-bounds reads when sema introspects the param array.
- **Wrong type tag** → silent ABI mismatch at call time.
- **Forgetting to bump `func_count`** → entries silently truncated.
- **Type-tag collisions** in the `CB_TYPE_*` numbering have to be policed by hand.

The boilerplate also discourages adding runtime functions, which the project will need many more of as the language grows beyond the current ~13 stubs.

## Solution

Move the catalog *construction* (not the ABI) from C to C++. The C ABI exposed to Rust stays byte-identical; only the way `catalog_funcs[]` is initialized changes.

**1. Single-line registration via a `CB_FN` macro.** Define a variadic macro that accepts the CB-visible name, the runtime function, and zero-or-N parameter name strings:

```cpp
#define CB_FN(cb_name, fn, ...) \
    CbFuncDesc{ cb_name, #fn, reinterpret_cast<void(*)()>(fn), \
                FuncTraits<fn>::template params_with_names<__VA_ARGS__>(), \
                FuncTraits<fn>::param_count, \
                FuncTraits<fn>::return_tag, 0 }
```

Usage becomes:

```cpp
constexpr CbFuncDesc catalog_funcs[] = {
    CB_FN("drawscreen", cb_rt_drawscreen),                          // 0-arg, no names
    CB_FN("print",      cb_rt_print,        "text"),                // 1 named
    CB_FN("abs",        cb_rt_abs_int,      "value"),
    CB_FN("abs",        cb_rt_abs_float,    "value"),
    CB_FN("screen",     cb_rt_screen,       "width", "height"),
    CB_FN("color",      cb_rt_color,        "r", "g", "b"),
    CB_FN("line",       cb_rt_line,         "x1", "y1", "x2", "y2"),
    // …
};
```

**2. `FuncTraits<auto Fn>` template** deduces the parameter and return type tags from the function pointer's type. A primary template plus partial specialization on `R(*)(Args...)` lets `sizeof...(Args)` give the param count and a `type_tag_v<T>` variable template give each tag.

Parameter *names* aren't recoverable from the signature, so they're accepted as the trailing `__VA_ARGS__` of `CB_FN` and threaded through a `constexpr` helper:

```cpp
template<typename R, typename... Args, R(*Fn)(Args...)>
struct FuncTraits<Fn> {
    static constexpr CbTypeTag return_tag = type_tag_v<R>;
    static constexpr uint32_t  param_count = sizeof...(Args);

    template<const char*... Names>
    static constexpr std::array<CbParamDesc, param_count> params_with_names() {
        static_assert(sizeof...(Names) == 0 || sizeof...(Names) == param_count,
                      "CB_FN: supply zero parameter names or exactly one per parameter");
        if constexpr (sizeof...(Names) == 0) {
            return { CbParamDesc{ nullptr, type_tag_v<Args> }... };
        } else {
            return { CbParamDesc{ Names, type_tag_v<Args> }... };
        }
    }
};
```

This makes partial naming a compile error rather than silent corruption (e.g., `CB_FN("line", cb_rt_line, "x1", "y1")` won't build). Anonymous parameters (`nullptr` name) are already what sema falls back to today for positional diagnostics, so the zero-names path is a drop-in.

**3. Function pointer in the catalog.** Add `void (*fn_ptr)(void)` to `CbFuncDesc` alongside the existing `symbol` string:

- **Interpreter** uses `fn_ptr` directly (eliminating any name→pointer dispatch table).
- **LLVM backend** uses `symbol` to emit `declare`/`call` instructions resolved by the linker (the pointer is meaningless across process boundaries).
- The `#fn` / `fn` pair in `CB_FN` guarantees the string and the pointer reference the same identifier — they cannot drift. `extern "C"` on every runtime function keeps the linker symbol unmangled and equal to `#fn`.

**4. Auto-size the catalog.** Replace the hand-maintained `func_count: 13` with `sizeof(catalog_funcs) / sizeof(catalog_funcs[0])` (same for `type_count`).

**5. Type tags via `enum class`, opaque handles as forward-declared structs reached by pointer.** Replace `#define CB_TYPE_TEST_HANDLE 10` (and any future opaque types) with `constexpr` tags starting at 10, removing the manual collision-avoidance.

**Convention: runtime functions must take and return custom types via pointer (`T*` or `const T*`), never by value.** The `type_tag<T>` primary template is left undefined; only the pointer forms are specialized. Any function that names a custom type by value fails to compile at the `CB_FN` site — the convention is enforced at compile time:

```c
/* cb_runtime.h — forward-declared struct, never defined */
typedef struct CbTestHandle CbTestHandle;

CbTestHandle* cb_rt_create_test_handle(void);                /* owning return */
int32_t       cb_rt_use_test_handle(const CbTestHandle* h); /* borrowing read-only */
```

```cpp
// catalog.cpp — two specializations per runtime-defined type
constexpr CbTypeTag CB_TYPE_TEST_HANDLE = 10;
template<> struct type_tag<      CbTestHandle*> { static constexpr CbTypeTag value = CB_TYPE_TEST_HANDLE; };
template<> struct type_tag<const CbTestHandle*> { static constexpr CbTypeTag value = CB_TYPE_TEST_HANDLE; };
```

`const` is a C-side documentation/correctness convention only — the catalog ABI and IR don't distinguish const from mutable. If/when read-only-only operations need policing, a follow-up FD can introduce `IrType::RuntimeTypeRef { name, mutable: bool }` and thread mutability through sema.

Function bodies cast back to whatever the handle actually is (`reinterpret_cast<uintptr_t>(h)` for a slab index, etc.). `CB_FN("usetesthandle", cb_rt_use_test_handle)` produces the right param tag automatically; the C++ function signature is the single source of truth.

Two consequences to flag:

- The handle payload must fit in `sizeof(void*)`. Fine on x64 (the current build target). 32-bit support would need a wrapper struct or a different convention.
- The catalog ABI continues to report opaque handles to Rust as 64-bit values regardless of in-process pointer width — the `Value::OpaqueHandle(u64)` representation on the Rust side does not change.

**6. Optional startup sanity check.** Add a debug assert in `load_catalog()` that, for each function, `dladdr(fn_ptr)` (or `SymFromAddr` on Windows) resolves to a symbol equal to `symbol` — catches any case where `extern "C"` was forgotten on a runtime function.

The runtime header (`cb_runtime.h`) stays C — only `catalog.c` becomes `catalog.cpp`. `gfx.c` and `input.c` remain C; their function declarations live behind `extern "C" { … }` blocks in the C++ side or are included via the existing C header (which the C++ TU includes inside its own `extern "C"`).

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/catalog.c` | DELETE | Replaced by `catalog.cpp`. |
| `runtime/catalog.cpp` | CREATE | Template-driven catalog construction with `CB_FN` macro and `FuncTraits`. |
| `runtime/cb_runtime.h` | MODIFY | Add `void (*fn_ptr)(void)` to `CbFuncDesc`; bump `CB_CATALOG_VERSION` to 3. |
| `runtime/CMakeLists.txt` | MODIFY | Add `CXX` to `project()` languages, set `CMAKE_CXX_STANDARD 20` (needed for `template<auto Fn>` deduction), swap `catalog.c` → `catalog.cpp` in `add_library`. |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | Add `fn_ptr: Option<unsafe extern "C" fn()>` to `CbFuncDesc`, propagate to `FuncDesc` for the interpreter; bump version check. |
| `crates/cb-runtime-sys/build.rs` | MODIFY | Replace `catalog.c` with `catalog.cpp` in the `rerun-if-changed` list. |
| `crates/cb-ir/src/lib.rs` | MODIFY | Extend `FuncDesc` with the optional `fn_ptr` (kept `Option` because user-defined functions don't have one). |
| `crates/cb-backend-interp/**` | MODIFY | Use `FuncDesc::fn_ptr` for FFI dispatch instead of looking up the symbol string. |
| `crates/cb-sema/**` | MODIFY | Only if `c_symbol` accesses need to be repointed; otherwise leaves the field alone. |

## Verification

- `cargo test -p cb-runtime-sys` — `load_catalog_returns_expected_entries` continues to pass with the new field; add assertions that `fn_ptr` is non-null for every entry.
- `cargo test -p cb-backend-interp` — the existing 23 integration tests cover every catalog function call path; passing them after switching to `fn_ptr`-based dispatch is the load-bearing check.
- `cargo test -p cb-driver` — end-to-end fixture tests (the recently-added Allegro `Screen`/`Color`/`Line` fixtures) confirm the runtime still links and dispatches.
- Add one negative test: deliberately make `CB_FN`'s string drift from the symbol (in a feature-gated test build) and confirm the debug `dladdr` assert fires.
- Cross-platform: confirm the build still works on Windows (MSVC, the current target) and document the C++20 requirement.

## Related

- [[FD-009_RUNTIME_LIBRARY]] — established the catalog ABI this FD refines (catalog version bumps from 2 → 3).
- [[FD-011_RUNTIME_CUSTOM_TYPES]] — introduced `CbTypeDesc` and the `tag >= 10` convention this FD replaces with `enum class`.
- `docs/cb_runtime.md` — runtime function catalog reference.
