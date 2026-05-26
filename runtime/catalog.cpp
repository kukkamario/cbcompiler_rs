// CoolBasic runtime catalog — built via C++ template DSL.
//
// Adding a runtime function is a one-line `CB_FN(...)` entry in
// `catalog_funcs[]` below. `FuncTraits<Fn>` deduces parameter and
// return type tags from the function signature; the `#fn` stringification
// in CB_FN ties the linker symbol to the function pointer, so they can't
// drift.
//
// Parameter names are intentionally anonymous in this first cut. Sema's
// diagnostics fall back to positional names ("argument 1", etc.). A
// future enhancement could pass names via a C++20 `StrLit` non-type
// template parameter pack if richer diagnostics become important.

#include "cb_runtime.h"

#include <array>
#include <cmath>
#include <cstdint>
#include <type_traits>

extern "C" {
    // Forward-declare implementations defined in the .c TUs (gfx.c, input.c)
    // and below in this TU.
    void    cb_rt_print(const char* text);
    int32_t cb_rt_abs_int(int32_t x);
    double  cb_rt_abs_float(double x);
    CbTestHandle* cb_rt_create_test_handle(void);
    int32_t cb_rt_use_test_handle(const CbTestHandle* handle);
}

// ─── Test-handle implementations ──────────────────────────────────────
//
// These were previously in catalog.c. Kept here because they're the only
// runtime functions that use a runtime-defined opaque type, so they
// double as compile-time tests of the `type_tag<CbTestHandle>` path.

extern "C" CbTestHandle* cb_rt_create_test_handle(void) {
    return reinterpret_cast<CbTestHandle*>(static_cast<uintptr_t>(42));
}

extern "C" int32_t cb_rt_use_test_handle(const CbTestHandle* handle) {
    return static_cast<int32_t>(reinterpret_cast<uintptr_t>(handle));
}

extern "C" void cb_rt_print(const char* text) {
    if (text) {
        std::puts(text);
    } else {
        std::puts("");
    }
}

extern "C" int32_t cb_rt_abs_int(int32_t x) {
    return x < 0 ? -x : x;
}

extern "C" double cb_rt_abs_float(double x) {
    return x < 0.0 ? -x : x;
}

extern "C" double cb_rt_sqrt(double x) {
    return std::sqrt(x);
}

// ─── Template DSL ─────────────────────────────────────────────────────

namespace cb_catalog {

// type_tag<T> — undefined primary template; specialize per supported C type.
//
// Built-in C types are specialized on the value type below.
//
// Custom (runtime-defined) types are specialized ONLY on pointer forms
// (`T*` and `const T*`). The primary template's missing definition turns
// any attempt to register a function taking a custom type by value into
// a compile error from FuncTraits — enforces the
// "custom types always via pointer" convention at compile time.
template<typename T> struct type_tag;

template<> struct type_tag<void>          { static constexpr CbTypeTag value = CB_TYPE_VOID; };
template<> struct type_tag<int8_t>        { static constexpr CbTypeTag value = CB_TYPE_BYTE; };
template<> struct type_tag<int16_t>       { static constexpr CbTypeTag value = CB_TYPE_SHORT; };
template<> struct type_tag<int32_t>       { static constexpr CbTypeTag value = CB_TYPE_INT; };
template<> struct type_tag<uint32_t>      { static constexpr CbTypeTag value = CB_TYPE_UINT; };
template<> struct type_tag<int64_t>       { static constexpr CbTypeTag value = CB_TYPE_LONG; };
template<> struct type_tag<uint64_t>      { static constexpr CbTypeTag value = CB_TYPE_ULONG; };
template<> struct type_tag<float>         { static constexpr CbTypeTag value = CB_TYPE_FLOAT; };
template<> struct type_tag<double>        { static constexpr CbTypeTag value = CB_TYPE_FLOAT; };
template<> struct type_tag<bool>          { static constexpr CbTypeTag value = CB_TYPE_BOOL; };
template<> struct type_tag<const char*>   { static constexpr CbTypeTag value = CB_TYPE_STRING; };

// Opaque handle type tags — two specializations per runtime-defined type,
// one for `T*` (mutable / owning return) and one for `const T*` (borrowing
// read-only). Both map to the same tag; const distinction is a C-side
// documentation convention and is not tracked in the catalog.
constexpr CbTypeTag CB_TYPE_TEST_HANDLE = 10;
template<> struct type_tag<      CbTestHandle*> { static constexpr CbTypeTag value = CB_TYPE_TEST_HANDLE; };
template<> struct type_tag<const CbTestHandle*> { static constexpr CbTypeTag value = CB_TYPE_TEST_HANDLE; };

template<typename T> inline constexpr CbTypeTag type_tag_v = type_tag<T>::value;

// FuncTraits<Fn> — deduces param/return tags from a function pointer's type.
template<auto Fn> struct FuncTraits;

template<typename R, typename... Args, R(*Fn)(Args...)>
struct FuncTraits<Fn> {
    static constexpr uint32_t  param_count = sizeof...(Args);
    static constexpr CbTypeTag return_tag  = type_tag_v<R>;

    using params_array = std::array<CbParamDesc, param_count>;

    static constexpr params_array params() {
        return params_array{ CbParamDesc{ nullptr, type_tag_v<Args> }... };
    }
};

// Per-function inline storage for the params array. `cb_anon_params<fn>`
// has stable static storage (one instance per Fn template argument), so
// its `.data()` pointer is a valid constant expression usable inside a
// `constexpr CbFuncDesc[]` initializer.
template<auto Fn>
inline constexpr auto cb_anon_params = FuncTraits<Fn>::params();

} // namespace cb_catalog

// CB_FN — register a runtime function as a single CbFuncDesc entry.
//
// `cb_name` is the CoolBasic-visible name (case-insensitive lookup).
// `fn` is the runtime function. `#fn` stringifies it as the linker symbol,
// and `reinterpret_cast<void(*)(void)>(fn)` produces the stored pointer.
// Both reference the same identifier — they cannot drift.
#define CB_FN(cb_name, fn)                                                      \
    CbFuncDesc{                                                                 \
        cb_name, #fn,                                                           \
        reinterpret_cast<void(*)(void)>(fn),                                    \
        (::cb_catalog::FuncTraits<fn>::param_count == 0)                        \
            ? nullptr                                                           \
            : ::cb_catalog::cb_anon_params<fn>.data(),                          \
        ::cb_catalog::FuncTraits<fn>::param_count,                              \
        ::cb_catalog::FuncTraits<fn>::return_tag,                               \
        0u                                                                      \
    }

// ─── Type catalog ─────────────────────────────────────────────────────

static constexpr CbTypeDesc catalog_types[] = {
    { "TestHandle", ::cb_catalog::CB_TYPE_TEST_HANDLE },
};

// ─── Function catalog ─────────────────────────────────────────────────
//
// Adding a new runtime function: declare its prototype in cb_runtime.h
// (with extern "C" if not already in the extern "C" block), implement
// it in one of the .c TUs (gfx.c / input.c) or here, and add one line
// to this array. No other edits required.

// `static const` rather than `constexpr` because `reinterpret_cast` of a
// function pointer is not a constant expression. Static initialization runs
// once before `cb_runtime_get_catalog` is called.
static const CbFuncDesc catalog_funcs[] = {
    // System
    CB_FN("print",            cb_rt_print),
    CB_FN("abs",              cb_rt_abs_int),
    CB_FN("abs",              cb_rt_abs_float),

    // Math
    CB_FN("sqrt",             cb_rt_sqrt),

    // Graphics
    CB_FN("screen",           cb_rt_screen),
    CB_FN("drawscreen",       cb_rt_drawscreen),
    CB_FN("color",            cb_rt_color),
    CB_FN("line",             cb_rt_line),
    CB_FN("screenwidth",      cb_rt_screen_width),
    CB_FN("screenheight",     cb_rt_screen_height),

    // Input
    CB_FN("mousex",           cb_rt_mouse_x),
    CB_FN("mousey",           cb_rt_mouse_y),

    // Test handles
    CB_FN("createtesthandle", cb_rt_create_test_handle),
    CB_FN("usetesthandle",    cb_rt_use_test_handle),
};

// ─── Catalog struct ────────────────────────────────────────────────────

static const CbCatalog catalog = {
    CB_CATALOG_VERSION,
    static_cast<uint32_t>(sizeof(catalog_types) / sizeof(catalog_types[0])),
    catalog_types,
    static_cast<uint32_t>(sizeof(catalog_funcs) / sizeof(catalog_funcs[0])),
    catalog_funcs,
};

extern "C" const CbCatalog* cb_runtime_get_catalog(void) {
    return &catalog;
}
