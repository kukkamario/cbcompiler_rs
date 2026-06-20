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
    // Forward-declare the implementations defined below in this TU.
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
// These are the only runtime functions that use a runtime-defined opaque
// type, so they double as compile-time tests of the
// `type_tag<CbTestHandle>` path.

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
// Strings flow as opaque `CbString*` (catalog v4+). A `const char*` form
// is intentionally NOT specialized — any runtime function declaring a
// string parameter as `const char*` fails to compile, enforcing the v4
// ABI at template-deduction time.
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

// Font — the text opaque handle (FD-018).
constexpr CbTypeTag CB_TYPE_FONT = 12;
template<> struct type_tag<      CbFont*>       { static constexpr CbTypeTag value = CB_TYPE_FONT; };
template<> struct type_tag<const CbFont*>       { static constexpr CbTypeTag value = CB_TYPE_FONT; };

// Object — the sprite opaque handle (FD-036 Phase 4).
constexpr CbTypeTag CB_TYPE_OBJECT = 13;
template<> struct type_tag<      CbObject*>     { static constexpr CbTypeTag value = CB_TYPE_OBJECT; };
template<> struct type_tag<const CbObject*>     { static constexpr CbTypeTag value = CB_TYPE_OBJECT; };

// Map — the tilemap opaque handle (FD-036 Phase 3). Object is tag 13; the map
// is tag 14.
constexpr CbTypeTag CB_TYPE_MAP = 14;
template<> struct type_tag<      CbMap*>        { static constexpr CbTypeTag value = CB_TYPE_MAP; };
template<> struct type_tag<const CbMap*>        { static constexpr CbTypeTag value = CB_TYPE_MAP; };

// Memblock — the raw byte-buffer opaque handle (FD-039, tag 15). Allegro-free,
// unlike Image/Font/Object/Map, so it (and its functions) stay outside the
// CB_NO_ALLEGRO guard and ship in the SDK-free catalog.
constexpr CbTypeTag CB_TYPE_MEMBLOCK = 15;
template<> struct type_tag<      CbMemblock*>   { static constexpr CbTypeTag value = CB_TYPE_MEMBLOCK; };
template<> struct type_tag<const CbMemblock*>   { static constexpr CbTypeTag value = CB_TYPE_MEMBLOCK; };

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
    // Memblock (FD-039) is Allegro-free — its Make/Peek/Poke functions are
    // present in every build, so the type is advertised unconditionally (unlike
    // the graphics handles below, which would have no functions in the SDK-free
    // catalog).
    { "Memblock",   ::cb_catalog::CB_TYPE_MEMBLOCK },
#ifndef CB_NO_ALLEGRO
    // Image/Font are graphics handles; their operations live behind the
    // Allegro guard below, so they're absent from the SDK-free catalog
    // (FD-033). Advertising the type with no functions to make one would
    // be inconsistent.
    { "Image",      ::cb_catalog::CB_TYPE_IMAGE },
    { "Font",       ::cb_catalog::CB_TYPE_FONT },
    { "Object",     ::cb_catalog::CB_TYPE_OBJECT },
    { "Map",        ::cb_catalog::CB_TYPE_MAP },
#endif
};

// ─── Constant catalog (FD-029) ────────────────────────────────────────
//
// Runtime-defined global constants. The compiler seeds these into its
// global scope and folds them like a user `Const`, so they never reach the
// backend at runtime. Only CB_TYPE_INT and CB_TYPE_FLOAT are supported.

#define CB_CONST_INT(n, val)   CbConstDesc{ n, CB_TYPE_INT,   { .i = static_cast<int64_t>(val) } }
#define CB_CONST_FLOAT(n, val) CbConstDesc{ n, CB_TYPE_FLOAT, { .f = static_cast<double>(val) } }

static const CbConstDesc catalog_consts[] = {
    CB_CONST_INT("On", 1),
    CB_CONST_INT("Off", 0),
    CB_CONST_FLOAT("PI", 3.14159265358979323846),
    // cbKey* scancode constants — generated from the shared cb_keys.def table
    // (the same source of truth that builds sCBKeyMap in cb_input.cpp).
    // CB_KEY_RAW rows are scancode-only and emit no constant here.
#define CB_KEY(name, scan, al) CB_CONST_INT(#name, scan),
#define CB_KEY_RAW(scan, al)
#include "cb_keys.def"
#undef CB_KEY
#undef CB_KEY_RAW
};

#undef CB_CONST_INT
#undef CB_CONST_FLOAT

// ─── Function catalog ─────────────────────────────────────────────────
//
// Adding a new runtime function: declare its prototype in cb_runtime.h
// (with extern "C" if not already in the extern "C" block), implement
// it in one of the subsystem TUs (cb_gfx.cpp / cb_input.cpp / …) or here,
// and add one line to this array. No other edits required.

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

    // Memory blocks (cb_memblock.cpp, FD-039). Allegro-free, so — unlike the
    // graphics/input block below — these stay outside the CB_NO_ALLEGRO guard
    // and ship in the SDK-free catalog. `Memblock` is the opaque tag-15 handle.
    CB_FN("makememblock",     cb_rt_make_memblock),
    CB_FN("deletememblock",   cb_rt_delete_memblock),
    CB_FN("resizememblock",   cb_rt_resize_memblock),
    CB_FN("memblocksize",     cb_rt_memblock_size),
    CB_FN("memcopy",          cb_rt_mem_copy),
    CB_FN("peekbyte",         cb_rt_peek_byte),
    CB_FN("peekshort",        cb_rt_peek_short),
    CB_FN("peekint",          cb_rt_peek_int),
    CB_FN("peekfloat",        cb_rt_peek_float),
    CB_FN("pokebyte",         cb_rt_poke_byte),
    CB_FN("pokeshort",        cb_rt_poke_short),
    CB_FN("pokeint",          cb_rt_poke_int),
    CB_FN("pokefloat",        cb_rt_poke_float),

    // Graphics, text, and input (cb_gfx.cpp / cb_font.cpp / cb_input.cpp) pull
    // in Allegro. The SDK-free build (FD-033, -DCB_NO_ALLEGRO) compiles only the
    // Allegro-free TUs, so these entries — and only these — are guarded out.
    // Their symbols are the sole reason catalog.cpp would otherwise force a link
    // against the Allegro closure (CB_FN takes each function's address).
#ifndef CB_NO_ALLEGRO
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
    // Game loop (FD-036 Phase 5): built-in object update/draw, deduped per frame.
    CB_FN("updategame",       cb_rt_update_game),
    CB_FN("drawgame",         cb_rt_draw_game),
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
    // FD-036 multi-frame sprite sheets — `frame`/`useMask` overloads (one CB_FN
    // per arity; useMask is accepted but ignored). LoadAnimImage returns the
    // existing `Image` opaque type, so no new type tag is registered.
    CB_FN("loadanimimage",    cb_rt_load_anim_image),
    CB_FN("makeimage",        cb_rt_make_image_frames),
    CB_FN("drawimage",        cb_rt_draw_image_frame),
    CB_FN("drawimage",        cb_rt_draw_image_frame_mask),
    CB_FN("drawghostimage",   cb_rt_draw_ghost_image_frame),
    CB_FN("drawimagebox",     cb_rt_draw_image_box_frame),
    CB_FN("drawimagebox",     cb_rt_draw_image_box_frame_mask),
    CB_FN("imagesoverlap",    cb_rt_images_overlap),
    CB_FN("imagescollide",    cb_rt_images_collide),
    CB_FN("screenwidth",      cb_rt_screen_width),
    CB_FN("screenheight",     cb_rt_screen_height),

    // Camera (cb_camera.cpp, FD-036 Phase 2). The world<->screen transform core.
    // No new opaque type — camera state is process-global. RotateCamera/
    // TurnCamera take two angle args (logical, render) feeding two independent
    // fields (CoolBasic's desyncable logical/render angles).
    CB_FN("positioncamera",   cb_rt_position_camera),
    CB_FN("movecamera",       cb_rt_move_camera),
    CB_FN("translatecamera",  cb_rt_translate_camera),
    CB_FN("rotatecamera",     cb_rt_rotate_camera),
    CB_FN("turncamera",       cb_rt_turn_camera),
    CB_FN("camerax",          cb_rt_camera_x),
    CB_FN("cameray",          cb_rt_camera_y),
    CB_FN("cameraangle",      cb_rt_camera_angle),
    CB_FN("drawtoworld",      cb_rt_draw_to_world),
    CB_FN("mousewx",          cb_rt_mouse_wx),
    CB_FN("mousewy",          cb_rt_mouse_wy),
    // Object-aware camera (FD-036 Phase 5, deferred from Phase 2). These take an
    // `Object`; CameraPick converts a screen point to world then picks.
    CB_FN("pointcamera",            cb_rt_point_camera),
    CB_FN("camerafollow",           cb_rt_camera_follow),
    CB_FN("clonecameraposition",    cb_rt_clone_camera_position),
    CB_FN("clonecameraorientation", cb_rt_clone_camera_orientation),
    CB_FN("camerapick",             cb_rt_camera_pick),

    // Tile maps (cb_map.cpp, FD-036 Phase 3). `Map` is the opaque handle
    // registered above (tag 14). One active map; EditMap's `map` arg is popped
    // but ignored. SetTile has a 2- and 3-arg form (animSlowness defaults to 1).
    CB_FN("loadmap",          cb_rt_load_map),
    CB_FN("makemap",          cb_rt_make_map),
    CB_FN("mapwidth",         cb_rt_map_width),
    CB_FN("mapheight",        cb_rt_map_height),
    CB_FN("getmap",           cb_rt_get_map),
    CB_FN("getmap2",          cb_rt_get_map2),
    CB_FN("editmap",          cb_rt_edit_map),
    CB_FN("setmap",           cb_rt_set_map),
    CB_FN("settile",          cb_rt_set_tile),
    CB_FN("settile",          cb_rt_set_tile_slow),

    // Objects / sprites (cb_object.cpp, FD-036 Phase 4). `Object` is the opaque
    // handle registered above (tag 13). Per-arity overloads share the CB name
    // with distinct C symbols (the settile/drawimage pattern); the lower-arity C
    // function bakes the defaults. Documented-but-ignored z/dz/rotQuality args are
    // exposed as separate higher-arity overloads. PaintObject (Object×Image,
    // Object×Object, Map×Image) and PlayObject (Object… vs Map…, to start tile
    // animation) are type-distinct overloads — sema's resolve_overload scores
    // exact type matches; the Map forms of both live in cb_map.cpp.
    CB_FN("loadobject",            cb_rt_load_object),
    CB_FN("loadobject",            cb_rt_load_object_rq),
    CB_FN("loadanimobject",        cb_rt_load_anim_object),
    CB_FN("loadanimobject",        cb_rt_load_anim_object_rq),
    CB_FN("makeobject",            cb_rt_make_object),
    CB_FN("makeobjectfloor",       cb_rt_make_object_floor),
    CB_FN("cloneobject",           cb_rt_clone_object),
    CB_FN("deleteobject",          cb_rt_delete_object),
    CB_FN("clearobjects",          cb_rt_clear_objects),
    CB_FN("positionobject",        cb_rt_position_object),
    CB_FN("positionobject",        cb_rt_position_object_z),
    CB_FN("moveobject",            cb_rt_move_object_fwd),
    CB_FN("moveobject",            cb_rt_move_object),
    CB_FN("moveobject",            cb_rt_move_object_z),
    CB_FN("translateobject",       cb_rt_translate_object),
    CB_FN("translateobject",       cb_rt_translate_object_z),
    CB_FN("cloneobjectposition",   cb_rt_clone_object_position),
    CB_FN("objectx",               cb_rt_object_x),
    CB_FN("objecty",               cb_rt_object_y),
    CB_FN("rotateobject",          cb_rt_rotate_object),
    CB_FN("turnobject",            cb_rt_turn_object),
    CB_FN("pointobject",           cb_rt_point_object),
    CB_FN("cloneobjectorientation", cb_rt_clone_object_orientation),
    CB_FN("objectangle",           cb_rt_object_angle),
    CB_FN("getangle2",             cb_rt_get_angle2),
    CB_FN("distance2",             cb_rt_distance2),
    CB_FN("paintobject",           cb_rt_paint_object_image),
    CB_FN("paintobject",           cb_rt_paint_object_object),
    CB_FN("paintobject",           cb_rt_paint_object_map),
    CB_FN("maskobject",            cb_rt_mask_object),
    CB_FN("ghostobject",           cb_rt_ghost_object),
    CB_FN("mirrorobject",          cb_rt_mirror_object),
    CB_FN("showobject",            cb_rt_show_object),
    CB_FN("defaultvisible",        cb_rt_default_visible),
    CB_FN("objectorder",           cb_rt_object_order),
    CB_FN("objectsizex",           cb_rt_object_size_x),
    CB_FN("objectsizey",           cb_rt_object_size_y),
    CB_FN("playobject",            cb_rt_play_object),
    CB_FN("playobject",            cb_rt_play_object3),
    CB_FN("playobject",            cb_rt_play_object4),
    CB_FN("playobject",            cb_rt_play_object5),
    CB_FN("playobject",            cb_rt_play_map),
    CB_FN("playobject",            cb_rt_play_map3),
    CB_FN("playobject",            cb_rt_play_map4),
    CB_FN("playobject",            cb_rt_play_map5),
    CB_FN("loopobject",            cb_rt_loop_object),
    CB_FN("loopobject",            cb_rt_loop_object3),
    CB_FN("loopobject",            cb_rt_loop_object4),
    CB_FN("loopobject",            cb_rt_loop_object5),
    CB_FN("stopobject",            cb_rt_stop_object),
    CB_FN("objectplaying",         cb_rt_object_playing),
    CB_FN("objectframe",           cb_rt_object_frame),
    CB_FN("objectinteger",         cb_rt_object_integer_get),
    CB_FN("objectinteger",         cb_rt_object_integer_set),
    CB_FN("objectfloat",           cb_rt_object_float_get),
    CB_FN("objectfloat",           cb_rt_object_float_set),
    CB_FN("objectstring",          cb_rt_object_string_get),
    CB_FN("objectstring",          cb_rt_object_string_set),
    CB_FN("objectlife",            cb_rt_object_life_get),
    CB_FN("objectlife",            cb_rt_object_life_set),
    CB_FN("initobjectlist",        cb_rt_init_object_list),
    CB_FN("nextobject",            cb_rt_next_object),

    // Collision (FD-036 Phase 5). SetupCollision is two type-distinct overloads
    // (object-object vs the type-4 Map form); ObjectRange/ObjectsOverlap have an
    // optional-arg arity overload. GetCollision returns an `Object` handle.
    CB_FN("setupcollision",        cb_rt_setup_collision),
    CB_FN("setupcollision",        cb_rt_setup_collision_map),
    CB_FN("objectrange",           cb_rt_object_range),
    CB_FN("objectrange",           cb_rt_object_range3),
    CB_FN("resetobjectcollision",  cb_rt_reset_object_collision),
    CB_FN("clearcollisions",       cb_rt_clear_collisions),
    CB_FN("countcollisions",       cb_rt_count_collisions),
    CB_FN("getcollision",          cb_rt_get_collision),
    CB_FN("collisionx",            cb_rt_collision_x),
    CB_FN("collisiony",            cb_rt_collision_y),
    CB_FN("collisionangle",        cb_rt_collision_angle),
    CB_FN("objectsoverlap",        cb_rt_objects_overlap),
    CB_FN("objectsoverlap",        cb_rt_objects_overlap3),

    // Picking & line of sight (FD-036 Phase 5). PickedObject returns an `Object`
    // handle; PixelPick is a registered no-op stub (1- and 2-arg forms).
    CB_FN("objectpickable",        cb_rt_object_pickable),
    CB_FN("objectpick",            cb_rt_object_pick),
    CB_FN("pixelpick",             cb_rt_pixel_pick),
    CB_FN("pixelpick",             cb_rt_pixel_pick_acc),
    CB_FN("pickedobject",          cb_rt_picked_object),
    CB_FN("pickedx",               cb_rt_picked_x),
    CB_FN("pickedy",               cb_rt_picked_y),
    CB_FN("pickedangle",           cb_rt_picked_angle),
    CB_FN("objectsight",           cb_rt_object_sight),
    CB_FN("screenpositionobject",  cb_rt_screen_position_object),

    // Particle emitters (FD-038). An emitter IS an `Object` (MakeEmitter returns
    // tag 13), so the object commands above drive it; the Particle* commands take
    // that `Object` and trap on a non-emitter. ParticleMovement's acceleration is
    // an optional arg (own arity overload).
    CB_FN("makeemitter",           cb_rt_make_emitter),
    CB_FN("particlemovement",      cb_rt_particle_movement),
    CB_FN("particlemovement",      cb_rt_particle_movement_acc),
    CB_FN("particleemission",      cb_rt_particle_emission),
    CB_FN("particleanimation",     cb_rt_particle_animation),

    // Text & fonts (FD-018)
    CB_FN("text",             cb_rt_text),
    CB_FN("centertext",       cb_rt_center_text),
    CB_FN("verticaltext",     cb_rt_vertical_text),
    CB_FN("locate",           cb_rt_locate),
    CB_FN("addtext",          cb_rt_add_text),
    CB_FN("cleartext",        cb_rt_clear_text),
    CB_FN("loadfont",         cb_rt_load_font),
    CB_FN("setfont",          cb_rt_set_font),
    CB_FN("deletefont",       cb_rt_delete_font),
    CB_FN("textwidth",        cb_rt_text_width),
    CB_FN("textheight",       cb_rt_text_height),

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
#endif // CB_NO_ALLEGRO

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
    static_cast<uint32_t>(sizeof(catalog_consts) / sizeof(catalog_consts[0])),
    catalog_consts,
    &cb_runtime_string_api,
};

extern "C" const CbCatalog* cb_runtime_get_catalog(void) {
    return &catalog;
}
