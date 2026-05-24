# FD-009: Runtime Library Architecture

**Status:** Complete
**Completed:** 2026-05-24
**Priority:** High
**Effort:** High (> 4 hours)
**Impact:** Enables both backends to execute runtime-provided functions (Print, math, I/O, graphics, audio) through a shared, pluggable runtime interface with a C ABI boundary

## Problem

The compiler currently has no mechanism for runtime-provided functions. Five intrinsics (`Len`, `Int`, `Float`, `Str`, `Bool`) are hard-coded via string matching in both sema (`check.rs`) and the lowerer (`lower.rs`). This approach doesn't scale and conflates two distinct categories:

1. **Compiler intrinsics** — operations the compiler understands structurally (type conversions, Type linked-list ops, string ops, `Len`, `Redim`). These map to dedicated IR instructions and each backend implements them directly. The compiler owns these.

2. **Runtime-provided functions** — everything else (`Print`, math, string manipulation, file I/O, graphics, input, audio). These are defined by the runtime, not the compiler. The runtime is replaceable: a minimal runtime might provide only `Print` and math; a full game runtime adds graphics, audio, input.

The compiler must be **runtime-agnostic**. It reads whatever function catalog the runtime provides, type-checks calls against it, and emits call instructions. It never hard-codes knowledge of specific runtime functions.

## Solution

### Architecture overview

The runtime is a **C library** (written in C or C++), built separately via CMake, and linked into the Rust compiler as a static library. Both the function catalog and the runtime functions use the C calling convention with typed signatures.

This design choice is deliberate: the runtime's core data structures (ref-counted strings with aliased mutable pointers, Type objects as plain pointers on linked lists) are fundamentally shared-mutable-pointer patterns. Implementing these in Rust would mean fighting the borrow checker behind `unsafe` blocks with no real safety benefit. In C, these patterns are natural and performant.

The C ABI also means:
- The runtime is implementable in any C-compatible language
- Plugin extensions are DLLs/SOs with the same C interface
- Both the interpreter (via FFI) and LLVM-compiled code (via linking) call the same functions the same way — no marshalling layer

```
                    ┌──────────────────────┐
                    │  runtime/            │
                    │  (C library, CMake)  │
                    │                      │
                    │ • C ABI catalog      │──── cb_runtime_get_catalog()
                    │   (CbCatalog struct) │     returns function names, types
                    │                      │
                    │ • C functions with   │──── typed signatures, e.g.
                    │   typed signatures   │     double cb_rt_sin(double)
                    │                      │
                    │ • String/Type memory │──── ref-counted strings,
                    │   management         │     pointer-based Type objects
                    └──────────┬───────────┘
                               │ static lib (.lib/.a)
                ┌──────────────┼──────────────┐
                ▼              ▼              ▼
        ┌────────────┐  ┌───────────┐  ┌──────────────┐
        │ cb-driver  │  │ interp    │  │ llvm backend │
        │            │  │ backend   │  │              │
        │ Loads      │  │           │  │ Links against│
        │ catalog →  │  │ FuncId →  │  │ extern "C"   │
        │ passes to  │  │ typed fn  │  │ symbols      │
        │ sema as    │  │ pointer   │  │              │
        │ plain Rust │  │ (via FFI) │  │              │
        └────────────┘  └───────────┘  └──────────────┘
```

### Build system

The runtime is built with **CMake** and linked into the Rust workspace via a build script:

```
runtime/
├── CMakeLists.txt           # builds static lib + optional shared lib
├── include/
│   └── cb_runtime.h         # public C header (catalog types, string API, type tags)
├── src/
│   ├── catalog.c            # cb_runtime_get_catalog() implementation
│   ├── string.c             # CbString ref-counted string implementation
│   ├── math.c               # math functions (sin, cos, abs, etc.)
│   ├── io.c                 # Print, file I/O
│   └── ...
└── tests/
    └── test_catalog.c       # C-level unit tests (ctest)
```

The Rust side links the static library:

- `cb-driver`'s `build.rs` invokes CMake to build the runtime, then tells cargo to link the resulting `.lib`/`.a`
- Alternatively, the user pre-builds the runtime and points to it via an env var (for CI or cross-compilation)
- The Rust crate `cb-runtime-sys` (thin FFI bindings) provides `#[repr(C)]` mirror types and `extern "C"` declarations so the Rust compiler can call the C functions

### Three tiers of function calls

| Tier | Examples | IR representation | Who owns it |
|------|----------|-------------------|-------------|
| Compiler intrinsics | `Int()`, `Float()`, `Len()`, `Convert`, `NewType`, `First`/`Last`/`Next`/`Previous`, `Delete` | Dedicated `InstKind` variants | Compiler (hard-coded) |
| Runtime functions | `Print`, `Sin`, `Mid`, `ReadFile`, `LoadImage` | `Call { callee: FuncId, args }` | Runtime (pluggable) |
| User-defined functions | `Sub Foo()`, `Function Bar()` | `Call { callee: FuncId, args }` | User source code |

### Runtime overload resolution

CoolBasic does not support user-defined function overloading, but the runtime catalog can declare multiple variants of the same CoolBasic-visible name with different parameter types (e.g., `Abs` for both Int and Float). This requires a new `DeclKind` variant and overload resolution in sema.

The current symbol table is `HashMap<Symbol, Declaration>` — one entry per name. To support runtime overloads without changing this structure, we use an `OverloadSet`:

```rust
pub enum DeclKind {
    Variable,
    Constant { value: ConstValue },
    Function { params: Vec<ParamInfo>, return_ty: Type, scope: Option<ScopeId> },
    TypeDef { fields: Vec<FieldInfo> },
    StructDef { fields: Vec<FieldInfo> },
    Label,
    // New: runtime function (single variant, no overloading)
    RuntimeFn { params: Vec<ParamInfo>, return_ty: Type, c_symbol: String },
    // New: multiple runtime functions sharing the same CoolBasic name
    OverloadSet { variants: Vec<OverloadVariant> },
}

pub struct OverloadVariant {
    pub params: Vec<ParamInfo>,
    pub return_ty: Type,
    pub c_symbol: String,
}
```

**Registration** (driver → sema): When the driver passes runtime catalog entries to sema, entries with the same CoolBasic name are grouped. If a name has one entry, it becomes `DeclKind::RuntimeFn`. If it has multiple entries, they become `DeclKind::OverloadSet`. User-defined functions cannot collide with either — attempting to define a function named `Abs` when the runtime provides it is a duplicate-declaration error.

**Resolution** (in `check_call`): When `check_call` encounters an `OverloadSet`, it scores each variant against the actual argument types:
- Exact type match → best candidate
- Match via implicit widening conversion (e.g., Int → Float) → acceptable but lower priority
- No valid conversion → variant eliminated

If exactly one variant survives, it wins. If multiple tie, prefer the one with the most exact matches. If still ambiguous, emit an error diagnostic. The resolved variant's `c_symbol` is what the lowerer uses to create the `FuncKind::Runtime { symbol }` in the IR func_table.

This keeps the `HashMap<Symbol, Declaration>` structure intact — one entry per name, with `OverloadSet` holding the variants internally.

### Numeric function IDs

All `Call` instructions use a numeric `FuncId(u32)`, not a `Symbol`. The IR `Program` contains a function symbol table:

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct FuncId(pub u32);

pub struct FuncDecl {
    pub name: Symbol,           // for diagnostics / IR printing
    pub sig: FnSig,             // param types + return type
    pub kind: FuncKind,         // user-defined vs runtime-provided
}

pub enum FuncKind {
    /// User-defined function — has a body in Program::functions
    UserDefined { body_index: usize },
    /// Runtime-provided — no body in IR, resolved by backend
    Runtime { symbol: String },   // C symbol name for dlsym/linking
}

pub struct Program {
    pub func_table: Vec<FuncDecl>,   // FuncId → declaration
    pub functions: Vec<Function>,    // bodies (user-defined only)
    pub type_defs: Vec<TypeDefInfo>,
    pub struct_defs: Vec<StructDefInfo>,
}
```

The backend builds its own dispatch from `FuncId`:
- **Interpreter:** `FuncId → extern "C" fn pointer` table (typed, cast at call time)
- **LLVM:** `FuncId → extern symbol name`, declared in the LLVM module and linked

### C ABI catalog format

The runtime exposes its function catalog through a C ABI. Every runtime (whether the default static library or a plugin DLL) exports a `cb_runtime_get_catalog()` function.

#### C types (defined in `runtime/include/cb_runtime.h`, mirrored as `#[repr(C)]` in Rust)

```c
#define CB_CATALOG_VERSION 1

// Type tags — maps to CoolBasic/IR types
typedef uint32_t CbTypeTag;
#define CB_TYPE_VOID    0
#define CB_TYPE_BYTE    1
#define CB_TYPE_SHORT   2
#define CB_TYPE_INT     3
#define CB_TYPE_UINT    4
#define CB_TYPE_LONG    5
#define CB_TYPE_ULONG   6
#define CB_TYPE_FLOAT   7
#define CB_TYPE_BOOL    8
#define CB_TYPE_STRING  9

typedef struct {
    const char* name;       // parameter name, null-terminated UTF-8 (may be NULL)
    CbTypeTag   type;       // parameter type
} CbParamDesc;

typedef struct {
    const char*        name;        // CoolBasic-visible name, null-terminated UTF-8, lowercase
    const char*        symbol;      // C linker symbol name (e.g. "cb_rt_sin")
    const CbParamDesc* params;      // pointer to array of param descriptors
    uint32_t           param_count; // number of parameters
    CbTypeTag          return_type; // return type (CB_TYPE_VOID for subs)
    uint32_t           flags;       // reserved for future use
} CbFuncDesc;

typedef struct {
    uint32_t           version;     // = CB_CATALOG_VERSION
    uint32_t           func_count;  // number of functions
    const CbFuncDesc*  funcs;       // pointer to array of func descriptors
} CbCatalog;

// Every runtime exports this entry point:
const CbCatalog* cb_runtime_get_catalog(void);
```

The `name` field is what CoolBasic source code writes (case-folded by the interner). The `symbol` field is the actual C linker symbol the backend uses for `dlsym`/`GetProcAddress` or LLVM extern declarations.

#### Versioning

The `version` field in `CbCatalog` guards against ABI mismatches. The compiler rejects catalogs with unknown versions. The `flags` field on `CbFuncDesc` is reserved for future additions (variadic, deprecated, etc.) without changing the struct layout.

### Typed function signatures

Runtime functions use **normal typed C signatures** — no boxing or tagged unions. Each function is a plain C function with its natural parameter and return types:

```c
// Math — native scalar types
double cb_rt_sin(double x);
double cb_rt_cos(double x);
double cb_rt_abs_float(double x);
int32_t cb_rt_abs_int(int32_t x);

// String operations — CbString is a pointer (see Value Representation)
CbString cb_rt_mid(CbString s, int32_t start, int32_t length);
CbString cb_rt_left(CbString s, int32_t count);
int32_t  cb_rt_instr(CbString haystack, CbString needle);

// I/O
void cb_rt_print(CbString text);
```

The **interpreter** stores runtime function pointers as opaque `*const ()`. At call time, since the parameter types and return type are known from the catalog, the interpreter casts the pointer to the correct typed `extern "C" fn(...)` signature and calls it directly. No boxing, no tagged union dispatch — just a `transmute` to the right function pointer type based on the catalog metadata.

The **LLVM backend** declares the typed extern symbols directly in the LLVM module and emits properly typed `call` instructions. Scalar arguments pass in registers (XMM for doubles, GPR for integers).

### Value representation at the C boundary

**Primitives** cross the ABI as their natural C types:

| CoolBasic type | C type | Notes |
|---|---|---|
| Byte | `uint8_t` | |
| Short | `int16_t` | |
| Int | `int32_t` | |
| UInt | `uint32_t` | |
| Long | `int64_t` | |
| ULong | `uint64_t` | |
| Float | `double` | |
| Bool | `int32_t` | 0 or 1; avoids C `_Bool` ABI ambiguity |

**Strings** are pointers to a ref-counted buffer. The buffer is a single allocation containing metadata and the UTF-8 string data:

```c
// Internal layout (not exposed to callers, managed by the runtime):
// ┌──────────┬──────────┬──────────┬───────────────────┐
// │ refcount │ byte_len │ capacity │ data[] (UTF-8)    │
// │ uint32_t │ uint32_t │ uint32_t │ char[capacity+1]  │
// └──────────┴──────────┴──────────┴───────────────────┘
//                                    ^ CbString points here

// CbString is a pointer to the data portion of the buffer.
// The metadata (refcount, length, capacity) lives at negative
// offsets from the pointer. NULL represents the empty/null string.
typedef char* CbString;

// Runtime exports these string management functions:
CbString    cb_string_new(const char* utf8_data, uint32_t byte_len);
const char* cb_string_cstr(CbString s);        // s itself, or "" if NULL
uint32_t    cb_string_len(CbString s);          // byte length (from header)
CbString    cb_string_retain(CbString s);       // increment refcount, return s
void        cb_string_release(CbString s);      // decrement refcount; frees at 0
```

This representation is efficient: `CbString` is a plain `char*` that can be passed directly to C standard library functions (`printf`, `strcmp`, etc.). The metadata at negative offsets is an internal implementation detail — callers use `cb_string_len()` to get the length and `cb_string_retain()`/`cb_string_release()` for ownership.

Ownership protocol:
- **Returned strings:** refcount = 1, caller takes ownership and must eventually call `cb_string_release`.
- **Parameter strings:** borrowed for the duration of the call. If the callee needs to keep a reference, it calls `cb_string_retain`.
- **String literals:** the compiler (or interpreter) creates them via `cb_string_new()` at program start or on first use.

**Type objects** are plain C pointers to heap-allocated structs managed by the runtime:

```c
// Opaque pointer to a Type instance. The runtime manages the
// doubly-linked list internally. NULL = null reference.
typedef void* CbTypeRef;
```

Both `CbString` and `CbTypeRef` are pointer-sized values that pass through the C ABI in a single register — no indirection, no boxing.

On the **Rust side**, `cb-runtime-sys` defines `#[repr(C)]` mirror types. The interpreter works with these as opaque pointer values — it doesn't need to understand the internal layout, just pass them through to runtime functions and call `retain`/`release` at the right times.

### How each component uses the catalog

**Driver** (compile time):
- Links against the default runtime static library (built by CMake)
- At startup, calls `cb_runtime_get_catalog()` via FFI to get the catalog
- For plugin DLLs (`--plugin <path>`): uses `libloading` to load the DLL, resolve `cb_runtime_get_catalog`, call it, read the C structs
- Converts catalog entries to plain Rust `FuncDesc` data (name string, param types, return type, C symbol name)
- Passes `Vec<FuncDesc>` to sema — sema never sees C ABI types
- Errors on name collisions between default runtime and plugins (or between plugins)

**Sema** (compile time):
- Receives `Vec<FuncDesc>` from the driver
- Groups entries by CoolBasic name: single-variant names become `DeclKind::RuntimeFn`, multi-variant names become `DeclKind::OverloadSet`
- Registers these in the top-level scope of the symbol table
- Calls to runtime functions flow through normal `check_call` — for `OverloadSet`, overload resolution picks the best variant based on argument types
- No more `check_intrinsic_call` string matching for runtime functions

**Lowerer** (compile time):
- When lowering a call to `DeclKind::RuntimeFn` (or a resolved `OverloadSet` variant), allocates a `FuncId` in the func_table with `FuncKind::Runtime { symbol }`
- Emits `InstKind::Call { callee: FuncId, args }`
- Same instruction as user function calls — the `FuncKind` flag distinguishes them

**Interpreter backend** (execution time):
- Builds a `FuncId → FuncEntry` dispatch table from the IR's func_table
- For `FuncKind::Runtime` entries: resolves the C symbol name to a function pointer (from the statically linked runtime or a loaded DLL)
- At call time: marshals interpreter `Value`s to C-typed arguments, casts the opaque fn pointer to the correct `extern "C" fn(...)` type based on the known signature, calls it, and converts the return value back
- For `FuncKind::UserDefined` entries: interprets the IR function body

**LLVM backend** (codegen time):
- For `FuncKind::Runtime` entries: declares `extern "C"` symbols with typed signatures in the LLVM module
- For `FuncKind::UserDefined` entries: emits LLVM IR from the function body
- Links the final binary against the runtime static library

### Plugin extension model

The default runtime is statically linked (always available). Plugin DLLs extend the function set:

- **Loading:** `--plugin <path.dll>` CLI flag, feature-gated behind `plugins` cargo feature (pulls in `libloading`)
- **Protocol:** Plugin DLL exports `cb_runtime_get_catalog()` with the same ABI as the default runtime
- **Name collisions:** Error by default. `--allow-override` flag lets plugins override default runtime functions (last wins in CLI order)
- **Symbol resolution:** The interpreter resolves each plugin function's `symbol` field via `dlsym`/`GetProcAddress` from the loaded DLL
- **C header:** `runtime/include/cb_runtime.h` is the public API for plugin developers — everything needed to write a plugin in C/C++
- **Plugins can also use runtime string/type APIs:** A plugin DLL that manipulates strings can call `cb_string_new()`, `cb_string_len()`, etc. from the statically linked runtime (the compiler binary exports these symbols, or the plugin links against the runtime library independently)

### Dependency graph

```
runtime/  (C library, built with CMake)
├── produces: cb_runtime.lib / libcb_runtime.a (static)
├── produces: cb_runtime.dll / libcb_runtime.so (shared, optional)
├── exports:  cb_runtime_get_catalog() + all cb_rt_* symbols
├── exports:  cb_string_*() string management API
└── header:   runtime/include/cb_runtime.h

cb-runtime-sys  (thin Rust FFI crate, #![allow(unsafe_code)])
├── #[repr(C)] mirror types: CbCatalog, CbFuncDesc, CbTypeTag, CbString, CbTypeRef
├── extern "C" declarations for all runtime symbols
├── build.rs: invokes CMake, links the static library
└── safe Rust wrappers where practical (e.g. CbString Drop/Clone)

cb-driver
├── depends on cb-runtime-sys (reads catalog via FFI at startup)
├── optionally depends on libloading (behind "plugins" feature, for DLL plugins)
├── converts catalog to plain Rust data, passes to sema
└── merges default + plugin catalogs, checks for name collisions

cb-sema
├── does NOT depend on cb-runtime-sys
├── receives Vec<FuncDesc> from driver (plain Rust types)
└── registers runtime functions in symbol table as DeclKind::RuntimeFn / OverloadSet

cb-backend-interp
├── depends on cb-ir, cb-runtime-sys
├── resolves FuncId → extern "C" fn pointer from runtime
└── casts to typed signature at call time, calls directly

cb-backend-llvm
├── depends on cb-ir (does NOT need cb-runtime-sys at compile time)
├── declares typed extern symbols in LLVM module
└── links compiled binary against runtime static lib at link time

cb-ir
├── defines FuncId, FuncDecl, FuncKind
└── does NOT depend on cb-runtime-sys
```

### Resolved design decisions

1. **Overloaded functions:** The catalog lists **separate entries with distinct C symbols** for type-specific variants. For example, `Abs` appears as two catalog entries: `cb_rt_abs_int` (Int → Int) and `cb_rt_abs_float` (Float → Float), both with CoolBasic name `"abs"`. Sema groups these into a `DeclKind::OverloadSet` and resolves the correct variant based on argument type at the call site (see "Runtime overload resolution" section above). This preserves type precision (e.g., `Abs(myInt)` returns Int, not Float). Note: user-defined function overloading is still not supported — overloads are runtime-only.

2. **Runtime state:** The runtime manages state internally via **global or thread-local storage**. No context parameter on any function. Functions like `OpenFile`, `LoadImage`, `DrawImage` access shared state (file handles, graphics context) through the runtime's own globals. This keeps function signatures clean and matches the original CoolBasic model (single-threaded, global state).

3. **String encoding:** **UTF-8** at the C ABI boundary. Matches `cb_syntax.md` §3.1 ("UTF-8 string"), the frontend's `string_value.rs` encoding, and modern convention. `CbString` data is always valid UTF-8, null-terminated.

4. **Runtime implementation language:** **C (or C++)**, not Rust. The runtime's core data structures (ref-counted strings with aliased mutable pointers, Type objects as plain pointers on linked lists) are shared-mutable-pointer patterns that are natural in C but fight Rust's ownership model. Keeping the runtime in C avoids `unsafe` Rust that provides no safety benefit, simplifies the ABI (no Rust-to-Rust-through-C awkwardness), and makes the runtime genuinely language-agnostic.

5. **Build system:** **CMake** for the C runtime. Rust links the static library via a `build.rs` in `cb-runtime-sys`.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/` | CREATE | C runtime library — CMakeLists.txt, include/, src/, tests/ |
| `runtime/include/cb_runtime.h` | CREATE | Public C header: catalog types, string API, type tags |
| `runtime/src/catalog.c` | CREATE | `cb_runtime_get_catalog()` implementation |
| `runtime/src/string.c` | CREATE | `CbString` ref-counted string implementation |
| `runtime/src/math.c` | CREATE | Math functions (sin, cos, abs, etc.) |
| `runtime/src/io.c` | CREATE | Print, file I/O |
| `crates/cb-runtime-sys/` | CREATE | Thin Rust FFI crate: `#[repr(C)]` types, extern declarations, build.rs |
| `crates/cb-ir/src/lib.rs` | MODIFY | Add `FuncId`, `FuncDecl`, `FuncKind`, `func_table` to `Program` |
| `crates/cb-ir/src/inst.rs` | MODIFY | Change `Call { callee: Symbol }` to `Call { callee: FuncId }` |
| `crates/cb-ir/src/verify.rs` | MODIFY | Verify `FuncId` resolves in func_table, check arity/types |
| `crates/cb-ir/src/print.rs` | MODIFY | Print `FuncId` with resolved name from func_table |
| `crates/cb-sema/src/scope.rs` | MODIFY | Add `DeclKind::RuntimeFn`, `DeclKind::OverloadSet`, `OverloadVariant` |
| `crates/cb-sema/src/check.rs` | MODIFY | Register runtime functions in symbol table, add overload resolution to `check_call` |
| `crates/cb-sema/src/lower.rs` | MODIFY | Allocate `FuncId` for runtime functions, emit `Call { callee: FuncId }` |
| `crates/cb-backend-interp/src/lib.rs` | MODIFY | Build `FuncId → typed fn pointer` dispatch, cast and call at execution time |
| `crates/cb-driver/src/main.rs` | MODIFY | Load runtime catalog via FFI, convert to Rust data, pass to sema |
| `Cargo.toml` | MODIFY | Add `cb-runtime-sys` to workspace members, add `plugins` feature to driver |

## Verification

- `ctest` (in `runtime/build/`) — C-level unit tests for string operations, catalog well-formedness
- `cargo test -p cb-runtime-sys` — FFI bindings compile, catalog readable from Rust, string wrappers work
- `cargo test -p cb-ir` — FuncId/FuncDecl structures, verifier checks FuncId validity
- `cargo test -p cb-sema` — runtime functions resolve in symbol table, type-check correctly, overload resolution works, lower to `Call { callee: FuncId }`
- `cargo test -p cb-backend-interp` — runtime function calls execute correctly (start with `Print`, basic math)
- IR snapshot tests — `--dump-ir` shows `call @func_name(...)` with resolved names
- Swap test — a no-runtime build compiles language-only programs but errors on `Print` calls
- C plugin test — a minimal C plugin DLL loaded via `--plugin`, its functions callable from CoolBasic

## Related

- [FD-007](archive/FD-007_Semantic_Analysis.md) — sema symbol table, `DeclKind`, `check_call`
- [FD-008](archive/FD-008_IR.md) — IR instruction set, `Call` instruction, `Program` structure
- `docs/cb_syntax.md` §8 — standard library surface, compiler-known vs runtime-provided
- `CLAUDE.md` — backend pluggability, interpreter as reference impl
