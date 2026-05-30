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
#include <cstdio>
#include <type_traits>

extern "C" {
    // Forward-declare implementations defined in the .c TUs (gfx.c, input.c)
    // and below in this TU.
    void    cb_rt_print(const CbString* text);
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

extern "C" void cb_rt_print(const CbString* text) {
    if (text) {
        std::size_t len = cb_rt_string_len(text);
        if (len > 0) {
            std::fwrite(cb_rt_string_data(text), 1, len, stdout);
        }
    }
    std::putchar('\n');
}

extern "C" int32_t cb_rt_abs_int(int32_t x) {
    return x < 0 ? -x : x;
}

extern "C" double cb_rt_abs_float(double x) {
    return x < 0.0 ? -x : x;
}

// Math functions live in cb_math.cpp; their prototypes are in cb_runtime.h.

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
// Strings flow as opaque `CbString*` (catalog v4+). The legacy
// `const char*` form is intentionally NOT specialized — any runtime
// function declaring a string parameter as `const char*` now fails to
// compile, enforcing the v4 ABI at template-deduction time.
template<> struct type_tag<      CbString*>     { static constexpr CbTypeTag value = CB_TYPE_STRING; };
template<> struct type_tag<const CbString*>     { static constexpr CbTypeTag value = CB_TYPE_STRING; };

// Opaque handle type tags — two specializations per runtime-defined type,
// one for `T*` (mutable / owning return) and one for `const T*` (borrowing
// read-only). Both map to the same tag; const distinction is a C-side
// documentation convention and is not tracked in the catalog.
constexpr CbTypeTag CB_TYPE_TEST_HANDLE = 10;
template<> struct type_tag<      CbTestHandle*> { static constexpr CbTypeTag value = CB_TYPE_TEST_HANDLE; };
template<> struct type_tag<const CbTestHandle*> { static constexpr CbTypeTag value = CB_TYPE_TEST_HANDLE; };

// Image — the graphics opaque handle (FD-013 Batch 4).
constexpr CbTypeTag CB_TYPE_IMAGE = 11;
template<> struct type_tag<      CbImage*>      { static constexpr CbTypeTag value = CB_TYPE_IMAGE; };
template<> struct type_tag<const CbImage*>      { static constexpr CbTypeTag value = CB_TYPE_IMAGE; };

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
    { "Image",      ::cb_catalog::CB_TYPE_IMAGE },
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

    // System / Time (cb_system.cpp). `End` is a language statement (IR Halt),
    // not registered here. `makeerror` prints its message; the lowerer appends
    // the terminating Halt.
    CB_FN("timer",            cb_rt_timer),
    CB_FN("wait",             cb_rt_wait),
    CB_FN("makeerror",        cb_rt_make_error),

    // Math (cb_math.cpp). Trig is in degrees. Min/Max/Rnd/Rand are
    // overloaded; sema resolves by argument type and arity.
    CB_FN("sin",              cb_rt_sin),
    CB_FN("cos",              cb_rt_cos),
    CB_FN("tan",              cb_rt_tan),
    CB_FN("asin",             cb_rt_asin),
    CB_FN("acos",             cb_rt_acos),
    CB_FN("atan",             cb_rt_atan),
    CB_FN("sqrt",             cb_rt_sqrt),
    CB_FN("log",              cb_rt_log),
    CB_FN("log10",            cb_rt_log10),
    CB_FN("roundup",          cb_rt_round_up),
    CB_FN("rounddown",        cb_rt_round_down),
    CB_FN("max",              cb_rt_max_int),
    CB_FN("max",              cb_rt_max_float),
    CB_FN("min",              cb_rt_min_int),
    CB_FN("min",              cb_rt_min_float),
    CB_FN("distance",         cb_rt_distance),
    CB_FN("getangle",         cb_rt_get_angle),
    CB_FN("wrapangle",        cb_rt_wrap_angle),
    CB_FN("rnd",              cb_rt_rnd_max),
    CB_FN("rnd",              cb_rt_rnd_range),
    CB_FN("rand",             cb_rt_rand_max),
    CB_FN("rand",             cb_rt_rand_range),
    CB_FN("randomize",        cb_rt_randomize),

    // String (cb_string.cpp). Codepoint-based, 1-based, clamping. `Str`/`Len`
    // are sema intrinsics, not registered here. InStr is overloaded (2/3 args).
    CB_FN("upper",            cb_rt_str_upper),
    CB_FN("lower",            cb_rt_str_lower),
    CB_FN("trim",             cb_rt_str_trim),
    CB_FN("left",             cb_rt_str_left),
    CB_FN("right",            cb_rt_str_right),
    CB_FN("strremove",        cb_rt_str_remove),
    CB_FN("instr",            cb_rt_str_instr),
    CB_FN("instr",            cb_rt_str_instr_from),
    CB_FN("chr",              cb_rt_chr),
    CB_FN("hex",              cb_rt_hex),

    // Graphics & images (cb_gfx.cpp). Overloads (Screen, Color, ClsColor,
    // Circle, Box, Lock, Unlock, PutPixel, MaskImage) share a CB name across
    // multiple C symbols; sema resolves by arity/type. `Image` is the opaque
    // handle type registered above.
    CB_FN("screen",           cb_rt_screen),
    CB_FN("screen",           cb_rt_screen_mode),
    CB_FN("drawscreen",       cb_rt_drawscreen),
    CB_FN("cls",              cb_rt_cls),
    CB_FN("clscolor",         cb_rt_cls_color),
    CB_FN("clscolor",         cb_rt_cls_color_a),
    CB_FN("drawtoscreen",     cb_rt_draw_to_screen),
    CB_FN("fps",              cb_rt_fps),
    CB_FN("lock",             cb_rt_lock),
    CB_FN("lock",             cb_rt_lock_state),
    CB_FN("lock",             cb_rt_lock_image),
    CB_FN("lock",             cb_rt_lock_image_state),
    CB_FN("unlock",           cb_rt_unlock),
    CB_FN("unlock",           cb_rt_unlock_image),
    CB_FN("color",            cb_rt_color),
    CB_FN("color",            cb_rt_color_a),
    CB_FN("line",             cb_rt_line),
    CB_FN("circle",           cb_rt_circle),
    CB_FN("circle",           cb_rt_circle_fill),
    CB_FN("box",              cb_rt_box),
    CB_FN("box",              cb_rt_box_fill),
    CB_FN("dot",              cb_rt_dot),
    CB_FN("putpixel",         cb_rt_put_pixel),
    CB_FN("putpixel",         cb_rt_put_pixel_a),
    CB_FN("putpixel",         cb_rt_put_pixel_argb),
    CB_FN("getpixel",         cb_rt_get_pixel),
    CB_FN("makeimage",        cb_rt_make_image),
    CB_FN("loadimage",        cb_rt_load_image),
    CB_FN("drawimage",        cb_rt_draw_image),
    CB_FN("maskimage",        cb_rt_mask_image),
    CB_FN("maskimage",        cb_rt_mask_image_a),
    CB_FN("drawtoimage",      cb_rt_draw_to_image),
    CB_FN("imagewidth",       cb_rt_image_width),
    CB_FN("imageheight",      cb_rt_image_height),
    CB_FN("deleteimage",      cb_rt_delete_image),
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
    &cb_runtime_string_api,
};

extern "C" const CbCatalog* cb_runtime_get_catalog(void) {
    return &catalog;
}
