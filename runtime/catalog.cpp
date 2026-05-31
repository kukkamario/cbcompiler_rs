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
    void cb_rt_test_request_exit(int32_t code);
    void cb_rt_test_raise_error(const CbString* msg);
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

// ─── Trap-channel test functions (FD-015) ─────────────────────────────
//
// Test-only, like the test-handle pair above: they give the runtime trap
// channel automated end-to-end coverage (the only production caller of
// request_exit is the manual window-close path, and raise_error has no
// production caller yet since out-of-range string args still clamp). Each
// asks the host to act through cb_host(); the callback records intent and
// returns, so we return normally afterwards.

extern "C" void cb_rt_test_request_exit(int32_t code) {
    const CbHostApi* h = cb_host();
    if (h) h->request_exit(code);
}

extern "C" void cb_rt_test_raise_error(const CbString* msg) {
    const CbHostApi* h = cb_host();
    if (h) h->raise_error(msg);
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
    CB_FN("date",             cb_rt_date),
    CB_FN("time",             cb_rt_time),
    CB_FN("commandline",      cb_rt_command_line),
    CB_FN("getexename",       cb_rt_get_exe_name),

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
    CB_FN("curvevalue",       cb_rt_curve_value),
    CB_FN("curveangle",       cb_rt_curve_angle),
    CB_FN("boxoverlap",       cb_rt_box_overlap),

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
    // FD-017 completeness pass.
    CB_FN("mid",              cb_rt_str_mid),
    CB_FN("replace",          cb_rt_str_replace),
    CB_FN("lset",             cb_rt_str_lset),
    CB_FN("rset",             cb_rt_str_rset),
    CB_FN("asc",              cb_rt_str_asc),
    CB_FN("bin",              cb_rt_bin),
    CB_FN("string",           cb_rt_str_repeat),
    CB_FN("flip",             cb_rt_str_flip),
    CB_FN("strinsert",        cb_rt_str_insert),
    CB_FN("strmove",          cb_rt_str_move),
    CB_FN("countwords",       cb_rt_count_words),
    CB_FN("getword",          cb_rt_get_word),

    // Graphics & images (cb_gfx.cpp). Overloads (Screen, Color, ClsColor,
    // Circle, Box, Lock, Unlock, PutPixel, MaskImage) share a CB name across
    // multiple C symbols; sema resolves by arity/type. `Image` is the opaque
    // handle type registered above.
    CB_FN("screen",           cb_rt_screen),
    CB_FN("screen",           cb_rt_screen_mode),
    CB_FN("screen",           cb_rt_screen_depth_mode),
    CB_FN("screen",           cb_rt_screen_buffer_id),
    CB_FN("screendepth",      cb_rt_screen_depth),
    CB_FN("gfxmodeexists",    cb_rt_gfx_mode_exists),
    CB_FN("drawscreen",       cb_rt_drawscreen),
    CB_FN("drawscreen",       cb_rt_drawscreen_args),
    CB_FN("screengamma",      cb_rt_screen_gamma),
    CB_FN("screenshot",       cb_rt_screenshot),
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
    CB_FN("getrgb",           cb_rt_get_rgb),
    CB_FN("pickcolor",        cb_rt_pick_color),
    CB_FN("smooth2d",         cb_rt_smooth_2d),
    CB_FN("line",             cb_rt_line),
    CB_FN("ellipse",          cb_rt_ellipse),
    CB_FN("circle",           cb_rt_circle),
    CB_FN("circle",           cb_rt_circle_fill),
    CB_FN("box",              cb_rt_box),
    CB_FN("box",              cb_rt_box_fill),
    CB_FN("dot",              cb_rt_dot),
    CB_FN("putpixel",         cb_rt_put_pixel),
    CB_FN("putpixel",         cb_rt_put_pixel_a),
    CB_FN("putpixel",         cb_rt_put_pixel_argb),
    CB_FN("putpixel2",        cb_rt_put_pixel_argb),
    CB_FN("getpixel",         cb_rt_get_pixel),
    CB_FN("getpixel",         cb_rt_get_pixel2),
    CB_FN("getpixel2",        cb_rt_get_pixel2),
    CB_FN("copybox",          cb_rt_copy_box),
    CB_FN("makeimage",        cb_rt_make_image),
    CB_FN("loadimage",        cb_rt_load_image),
    CB_FN("drawimage",        cb_rt_draw_image),
    CB_FN("maskimage",        cb_rt_mask_image),
    CB_FN("maskimage",        cb_rt_mask_image_a),
    CB_FN("drawtoimage",      cb_rt_draw_to_image),
    CB_FN("imagewidth",       cb_rt_image_width),
    CB_FN("imageheight",      cb_rt_image_height),
    CB_FN("deleteimage",      cb_rt_delete_image),
    CB_FN("defaultmask",      cb_rt_default_mask),
    CB_FN("cloneimage",       cb_rt_clone_image),
    CB_FN("resizeimage",      cb_rt_resize_image),
    CB_FN("rotateimage",      cb_rt_rotate_image),
    CB_FN("pickimagecolor",   cb_rt_pick_image_color),
    CB_FN("pickimagecolor2",  cb_rt_pick_image_color),
    CB_FN("saveimage",        cb_rt_save_image),
    CB_FN("drawghostimage",   cb_rt_draw_ghost_image),
    CB_FN("drawimagebox",     cb_rt_draw_image_box),
    CB_FN("hotspot",          cb_rt_hotspot),
    CB_FN("imagesoverlap",    cb_rt_images_overlap),
    CB_FN("imagescollide",    cb_rt_images_collide),
    CB_FN("screenwidth",      cb_rt_screen_width),
    CB_FN("screenheight",     cb_rt_screen_height),

    // Input
    CB_FN("keydown",          cb_rt_key_down),
    CB_FN("keyup",            cb_rt_key_up),
    CB_FN("keyhit",           cb_rt_key_hit),
    CB_FN("escapekey",        cb_rt_escape_key),
    CB_FN("getkey",           cb_rt_get_key),
    CB_FN("waitkey",          cb_rt_wait_key),
    CB_FN("clearkeys",        cb_rt_clear_keys),
    CB_FN("leftkey",          cb_rt_left_key),
    CB_FN("rightkey",         cb_rt_right_key),
    CB_FN("upkey",            cb_rt_up_key),
    CB_FN("downkey",          cb_rt_down_key),
    CB_FN("mousex",           cb_rt_mouse_x),
    CB_FN("mousey",           cb_rt_mouse_y),
    CB_FN("mousedown",        cb_rt_mouse_down),
    CB_FN("mousehit",         cb_rt_mouse_hit),
    CB_FN("mouseup",          cb_rt_mouse_up),
    CB_FN("mousez",           cb_rt_mouse_z),
    CB_FN("mousemovex",       cb_rt_mouse_move_x),
    CB_FN("mousemovey",       cb_rt_mouse_move_y),
    CB_FN("mousemovez",       cb_rt_mouse_move_z),
    CB_FN("getmouse",         cb_rt_get_mouse),
    CB_FN("waitmouse",        cb_rt_wait_mouse),
    CB_FN("positionmouse",    cb_rt_position_mouse),
    CB_FN("showmouse",        cb_rt_show_mouse),
    CB_FN("clearmouse",       cb_rt_clear_mouse),

    // Test handles
    CB_FN("createtesthandle", cb_rt_create_test_handle),
    CB_FN("usetesthandle",    cb_rt_use_test_handle),

    // Trap-channel test functions (FD-015; test-only, see implementations)
    CB_FN("testrequestexit",  cb_rt_test_request_exit),
    CB_FN("testraiseerror",   cb_rt_test_raise_error),
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
