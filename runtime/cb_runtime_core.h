#ifndef CB_RUNTIME_CORE_H
#define CB_RUNTIME_CORE_H

/* Runtime CORE ABI (FD-016).
 *
 * The irreducible runtime surface that the compiler/backends cannot function
 * without and that PLUGINS must reference to accept/return String parameters
 * and to register catalog entries: the type-tag vocabulary, the opaque
 * `CbString` type and its primitives, the catalog descriptor structs, and
 * (added by FD-015) the host/hook handshake types.
 *
 * This header has ZERO Allegro / functionality dependency — it is the plugin
 * SDK header. A plugin includes only this and statically links the
 * Allegro-free `cb_runtime_core` library; cross-module string safety rests on
 * the shared dynamic /MD CRT and value-based identity (emptiness = len==0,
 * immortality = refcount<0 — never an address compare).
 *
 * Functionality prototypes (Math, the String library, System, Graphics,
 * Input) live in cb_runtime_func.h. */

#include <stddef.h>
#include <stdint.h>

#define CB_CATALOG_VERSION 6

/* Host trap-channel ABI version (FD-015). Versions the CbHostApi/CbRuntimeHooks
 * handshake independently of the catalog data format: a catalog bump (e.g. v6
 * for runtime constants) must NOT force hosts to re-version when CbHostApi is
 * unchanged. cb_runtime_init rejects hosts whose abi_version != this. */
#define CB_HOST_ABI_VERSION 1

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
/* Tags >= 10 are runtime-defined opaque types (see CbTypeDesc). */

/* Opaque handle types. Each runtime-defined type is a forward-declared
   struct that is never defined — only pointers to it ever appear in
   function signatures. The C++ catalog's type_tag<T> is specialized for
   the pointer forms only (`T*` and `const T*`); passing a custom type by
   value is a compile error because the primary template is undefined.

   CONVENTION: Runtime functions MUST take and return custom types via
   pointer:
       MyType* cb_rt_make_thing(void);          // owning return
       double  cb_rt_thing_x(const MyType* t);  // borrowing read-only
       void    cb_rt_set_x(MyType* t, double x);// borrowing mutate
   The runtime is free to encode the handle as a slab index, an actual
   pointer, etc.; only the bit pattern matters.

   The concrete opaque handle types defined by the bundled functionality
   (CbImage, CbTestHandle) live in cb_runtime_func.h — they are not part of
   the core ABI. A plugin defining its own opaque type follows the same
   convention and declares it in its own header. */

/* String handle. Opaque refcounted object; flows across the FFI boundary
   as `CbString*`. Backends call retain/release/from_literal/len/data/concat
   through the `CbStringApi` substruct on `CbCatalog` (see below) — these
   are NOT registered as CB-visible runtime functions.

   Static-data sentinel: a CbString whose internal refcount is negative is
   treated as immortal — retain/release are no-ops. The global empty-string
   instance (referenced via CbStringApi::empty) is the canonical sentinel
   and is the value backends use to default-initialize String locals.
   This invariant lets the runtime skip null checks: backends MUST NOT
   produce a null CbString*. */
typedef struct CbString CbString;

typedef struct {
    CbString*       (*retain)      (CbString*);
    void            (*release)     (CbString*);
    CbString*       (*from_literal)(const uint8_t* data, size_t len);
    size_t          (*len)         (const CbString*);
    const uint8_t*  (*data)        (const CbString*);
    CbString*       (*concat)      (const CbString*, const CbString*);
    const CbString* empty;
} CbStringApi;

typedef struct {
    const char* name;
    CbTypeTag   tag;
} CbTypeDesc;

typedef struct {
    const char* name;
    CbTypeTag   type;
} CbParamDesc;

typedef struct {
    const char*        name;
    const char*        symbol;
    /* Statically-linked address of the runtime function. The interpreter
       dispatches through this; the LLVM backend uses `symbol` for
       declare/call emission. The CB_FN macro guarantees `symbol` (via #)
       and `fn_ptr` reference the same identifier. */
    void             (*fn_ptr)(void);
    const CbParamDesc* params;
    uint32_t           param_count;
    CbTypeTag          return_type;
    uint32_t           flags;
} CbFuncDesc;

/* A global constant predeclared by the runtime and seeded into the compiler's
   global scope (FD-029). The compiler folds these like a user `Const`, so they
   never reach the backend at runtime. `tag` is restricted to CB_TYPE_INT and
   CB_TYPE_FLOAT for now; the union grows (and other tags become legal) behind a
   future CB_CATALOG_VERSION bump. */
typedef struct {
    const char* name;
    CbTypeTag   tag;            /* CB_TYPE_INT or CB_TYPE_FLOAT only for now */
    union {
        int64_t i;
        double  f;
    } v;
} CbConstDesc;

typedef struct {
    uint32_t            version;
    uint32_t            type_count;
    const CbTypeDesc*   types;
    uint32_t            func_count;
    const CbFuncDesc*   funcs;
    uint32_t            const_count;
    const CbConstDesc*  consts;
    /* Backend-only API for the String primitive. Not callable from CB
       source; see CbStringApi above for rationale. Always non-null in v4+. */
    const CbStringApi*  strings;
} CbCatalog;

/* ─── Runtime Trap Channel (FD-015) ──────────────────────────────────────
   A cooperative, runtime-originated signalling channel. A `cb_rt_*` function
   asks the host to terminate cleanly or raise a runtime error by calling back
   through `CbHostApi`; the callback records the intent and RETURNS (it never
   unwinds the C frame). The host delivers its API once, at startup, via the
   `cb_runtime_init` handshake (modelled on SQLite's loadable-extension pApi);
   each module — the main runtime and every plugin DLL — keeps its own
   `g_host`. `CbCatalog`/`CbStringApi` stay const and are unaffected. */

typedef struct {
    uint32_t size;             /* sizeof(CbHostApi) — caller-set ABI guard */
    uint32_t abi_version;      /* == CB_HOST_ABI_VERSION */
    void (*request_exit)(int32_t code);        /* clean exit; host → Ok(code) */
    void (*raise_error)(const CbString* msg);  /* fatal runtime error → exit 1 */
    /* grow by appending; readers gate on `size` */
} CbHostApi;

typedef struct {
    uint32_t size;             /* sizeof(CbRuntimeHooks) — callee-set */
    void (*about_to_exit)(void);   /* host calls before shutdown; nullable.
                                      RESERVED — returned null for now. */
    /* grow by appending */
} CbRuntimeHooks;

/* can-trap flag bit for CbFuncDesc.flags — RESERVED for a future LLVM backend
   to gate its post-call pending-check so pure math/trig pays nothing. The
   interpreter drains the channel unconditionally after every call and ignores
   this bit. */
#define CB_FUNC_CAN_TRAP 0x1u

/* ABI layout pins (FD-024). These sizes are mirrored by `const`-assertions in
   crates/cb-runtime-sys/src/lib.rs; any drift fails the build on both sides
   before a mismatched struct can reach the FFI boundary. Sizes are the LP64 /
   Win64 repr(C) layout (8-byte pointers, natural alignment). */
static_assert(sizeof(CbHostApi)      == 24, "CbHostApi ABI drift");
static_assert(sizeof(CbRuntimeHooks) == 16, "CbRuntimeHooks ABI drift");
static_assert(sizeof(CbFuncDesc)     == 48, "CbFuncDesc ABI drift");
static_assert(sizeof(CbCatalog)      == 56, "CbCatalog ABI drift");

#ifdef __cplusplus
extern "C" {
#endif

const CbCatalog* cb_runtime_get_catalog(void);

/* String primitive implementation. Catalog v4 routes these through
   `CbStringApi`; the bare extern declarations are kept so the C++ side
   can reference them by symbol (e.g. when populating cb_runtime_string_api).
   They are NOT CB-visible (no `CB_FN` entry). */
CbString* cb_rt_string_retain(CbString* s);
void cb_rt_string_release(CbString* s);
CbString* cb_rt_string_from_literal(const uint8_t* data, size_t len);
size_t cb_rt_string_len(const CbString* s);
/* Unicode codepoint count (CB `Len(s$)`); distinct from the byte length above.
   Bare symbol; the native backend lowers `StrLen` onto it to match the
   interpreter's codepoint count. */
size_t cb_rt_string_char_len(const CbString* s);
const uint8_t* cb_rt_string_data(const CbString* s);
CbString* cb_rt_string_concat(const CbString* a, const CbString* b);

/* Instrumentation hook for tests — returns the current refcount, or a
   negative value for the static sentinel. Not in CbStringApi; called
   directly by Rust-side tests via an extern declaration. */
int32_t cb_rt_string_test_refcount(const CbString* s);

/* Lexicographic byte comparison shared by the interpreter and the native
   backend (FD-049 decision C). Returns <0 / 0 / >0 (normalized to -1/0/1);
   null operands are treated as empty. NOT a CB_FN — a bare symbol like the
   string primitives. */
int32_t cb_rt_string_compare(const CbString* a, const CbString* b);

extern const CbStringApi cb_runtime_string_api;

/* ─── Standalone (AOT) program lifecycle (FD-049 decision A) ──────────────
   The native backend emits a tiny `int main()` that calls cb_rt_standalone_run
   with the lowered top-level body (`cb_user_main`). These build the default
   host, run the FD-015 handshake, run the body, and exit cleanly. They carry no
   `main` of their own, so they are dormant/harmless in the interpreter binary
   (which statically links the runtime and drives cb_runtime_init itself). */

/* No-return clean process exit. Fires the about_to_exit teardown exactly once
   (latched), then libc exit() — which flushes piped stdio (the test harness
   captures stdout). Also the target of the default host's request_exit. */
void cb_rt_exit(int32_t code);

/* Build the default CbHostApi, run cb_runtime_init (null handshake → stderr +
   exit 1), stash the returned hooks for cb_rt_exit, invoke user_main, then
   cb_rt_exit(0). Returns 0 only formally (it never returns). */
int32_t cb_rt_standalone_run(void (*user_main)(void));

/* No-return trap for a null function-pointer call (FD-049): raise the FD-015
   error "null function pointer call" (matching the interpreter's NullFnPtr
   trap message) through the host channel, then exit 1. The LLVM backend's
   CallIndirect null-check branches here instead of a bare cb_rt_exit(1), so the
   native exe writes the same stderr trap message as the interpreter. */
void cb_rt_trap_null_fnptr(void);

/* Runtime Trap Channel handshake (FD-015). The host passes its API by const
   pointer; the runtime stashes it in a file-static and returns the hook table
   it wants connected (null hooks = not connected). Kept separate from
   cb_runtime_get_catalog, which must stay retrievable as pure data before
   init runs. Each plugin DLL exports both entry points. */
const CbRuntimeHooks* cb_runtime_init(const CbHostApi* host);

/* Runtime-side accessor for `g_host` — `cb_rt_*` functions call
   `cb_host()->request_exit(...)` / `->raise_error(...)`. Returns null before
   cb_runtime_init has run. */
const CbHostApi* cb_host(void);

/* Teardown-registration seam (FD-043). `about_to_exit` (the hook table slot)
   dispatches to every callback registered here, so a functionality module can
   register an at-exit teardown without core referencing its Allegro symbols —
   preserving the cb_runtime_core / functionality split (FD-016). Registration
   is de-duped by pointer, so multiple init sites registering the same callback
   is safe. The SDK-free build registers nothing, so about_to_exit is a no-op. */
void cb_runtime_register_teardown(void (*fn)(void));

/* Instrumentation hook for tests — number of times the about_to_exit dispatch
   has run. Bare symbol like cb_rt_string_test_refcount; not a CB_FN, not in any
   catalog. Lets a Rust-side test assert the teardown channel fired. */
int32_t cb_rt_test_teardown_count(void);

#ifdef __cplusplus
}
#endif

#endif /* CB_RUNTIME_CORE_H */
