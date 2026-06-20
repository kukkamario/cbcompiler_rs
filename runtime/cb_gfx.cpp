// CoolBasic graphics & image runtime (FD-013 Batch 4).
//
// Ported from the legacy ../CBCompiler/Runtime/cb_gfx.cpp + cb_image.cpp +
// image.cpp, flattening their RenderTarget/Window/Image class hierarchy down
// to a small set of file-static state + a C-ABI `CbImage` opaque handle. Kept
// deliberately simple and observable (CLAUDE.md): one translation unit owns the
// display, the active render target, the draw/clear colors, and FPS counting,
// so the screen and image functions share state without a class graph.
//
// ABI conventions (see cb_runtime.h / the catalog DSL): CB `Float` parameters
// arrive as `double` and CB `Int` as `int32_t`, regardless of what Allegro's
// own signatures want — the interpreter's libffi dispatch always pushes f64 for
// floats and i32 for ints. `Image` is the runtime-defined opaque handle type;
// it crosses the FFI boundary as `CbImage*` (a bit pattern the runtime owns).

#include "cb_runtime.h"
#include "cb_input.h"
#include "cb_camera.h"
#include "cb_map.h"
#include "cb_font.h"
#include "cb_geom.h"

#include <allegro5/allegro.h>
#include <allegro5/allegro_primitives.h>
#include <allegro5/allegro_image.h>
#include <allegro5/allegro_font.h>
#include <allegro5/allegro_ttf.h>

#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

// ─── Opaque Image handle ──────────────────────────────────────────────
//
// The CB-visible `Image` type. Declared (never defined) in cb_runtime.h as
// `struct CbImage`; defined here. Always passed/returned by pointer.
//
// FD-017 adds a hotspot — the draw/scale/rotate origin. It defaults to (0,0)
// (top-left), so functions that predate it (DrawImage) are unaffected; HotSpot,
// CloneImage, and RotateImage set it.
//
// FD-036 adds multi-frame sprite-sheet metadata. `anim_length == 0` (the
// default) means a single-frame image — every draw uses the whole bitmap and the
// frame parameter is ignored. LoadAnimImage sets frame_w/frame_h/anim_length so
// the bitmap is sliced into frame_w×frame_h cells. `anim_begin` (the start frame)
// is stored for parity but never read in any draw path (matches cbEnchanted).
struct CbImage {
    ALLEGRO_BITMAP* bmp;
    int32_t hotspot_x = 0;
    int32_t hotspot_y = 0;
    int32_t frame_w = 0;
    int32_t frame_h = 0;
    int32_t anim_begin = 0;
    int32_t anim_length = 0;
};

// ─── Opaque Font handle ───────────────────────────────────────────────
//
// The CB-visible `Font` type (FD-018). Wraps an Allegro font; always passed and
// returned by pointer. Created by LoadFont, freed by DeleteFont. The built-in
// default font is owned separately (see `default_font` below) and is never
// wrapped in a heap CbFont, so the program cannot DeleteFont it.
struct CbFont {
    ALLEGRO_FONT* font;
};

// ─── Shared graphics state ─────────────────────────────────────────────

static ALLEGRO_DISPLAY*     display       = nullptr;
static ALLEGRO_EVENT_QUEUE* event_queue   = nullptr;
static ALLEGRO_COLOR        draw_color;
static ALLEGRO_COLOR        clear_color;
static int32_t              screen_w      = 0;
static int32_t              screen_h      = 0;

// Logical design resolution (cbEnchanted's defaultWidth/Height). The camera
// world transform centers on (design_w/2, design_h/2). Defaults to 400x300 (the
// cbEnchanted default) and is updated to the requested size by `Screen` — kept
// separate from screen_w/h so the default survives when no window is opened.
static int32_t              design_w      = 400;
static int32_t              design_h      = 300;

// The active render target — the display backbuffer or an image's bitmap.
// Drawing primitives and PutPixel/Cls/Lock act on this. Mirrors the legacy
// RenderTarget::sCurrentTarget without the class machinery.
static ALLEGRO_BITMAP*      current_target = nullptr;

// FPS bookkeeping: frames counted in DrawScreen, sampled once per second.
static double               fps_last_sample = 0.0;
static int32_t              fps_frame_count = 0;
static int32_t              fps_value       = 0;

// FD-017 best-effort state. Smooth2D toggles linear filtering on new bitmaps;
// ScreenGamma is stored but not applied (Allegro 5 has no simple display gamma
// ramp) — kept so reads/round-trips behave and to document the divergence.
static bool                 smooth_2d       = false;
static double               gamma_r         = 1.0;
static double               gamma_g         = 1.0;
static double               gamma_b         = 1.0;

// DefaultMask: when enabled, the mask color is applied (mask→alpha 0) to every
// image created by MakeImage/LoadImage afterward.
static bool                 default_mask_on = false;
static ALLEGRO_COLOR        default_mask_color;

// ─── Text & font state (FD-018) ────────────────────────────────────────
//
// `default_font` is loaded once (Courier New 12pt, or Allegro's built-in 8x8
// font as a never-fail fallback) and owned for the process lifetime.
// `current_font` is what Text/AddText/TextWidth use; it points at default_font
// or at a LoadFont'd font's ALLEGRO_FONT*. The queued-text list holds AddText
// entries that re-render every DrawScreen until ClearText (mirrors cbEnchanted's
// TextInterface::texts). `text_loc_x/y` is the AddText cursor (Locate).
static ALLEGRO_FONT*        default_font  = nullptr;
static ALLEGRO_FONT*        current_font  = nullptr;
static int32_t              text_loc_x    = 0;
static int32_t              text_loc_y    = 0;

struct QueuedText {
    ALLEGRO_FONT* font;
    std::string   utf8;
    int32_t       x;
    int32_t       y;
    ALLEGRO_COLOR col;
};
static std::vector<QueuedText> queued_texts;

// Lazily initialize the Allegro subsystems the graphics runtime needs. Safe to
// call repeatedly. Image functions call this too, so images work (on memory
// bitmaps) before any window is opened.
static void ensure_init(void) {
    if (!al_is_system_installed()) {
        al_init();
    }
    if (!al_is_primitives_addon_initialized()) {
        al_init_primitives_addon();
    }
    if (!al_is_image_addon_initialized()) {
        al_init_image_addon();
    }
    if (!al_is_mouse_installed()) {
        al_install_mouse();
    }
    if (!al_is_keyboard_installed()) {
        al_install_keyboard();
    }
    if (!al_is_font_addon_initialized()) {
        al_init_font_addon();
    }
    if (!al_is_ttf_addon_initialized()) {
        al_init_ttf_addon();
    }
    if (!default_font) {
        // Default font: Courier New 12pt monochrome (cbEnchanted's default). If
        // the system font is unavailable, fall back to Allegro's built-in 8x8
        // bitmap font so a current font always exists — Text and the metric
        // queries never crash and work headless (improving on cbEnchanted,
        // which warned and risked a crash).
        std::string path = cb_findfont("Courier New");
        if (!path.empty()) {
            default_font = al_load_font(path.c_str(), 12, ALLEGRO_TTF_MONOCHROME);
        }
        if (!default_font) {
            default_font = al_create_builtin_font();
        }
        if (!current_font) {
            current_font = default_font;
        }
    }
}

// ─── Screen management ─────────────────────────────────────────────────

extern "C" void cb_rt_screen(int32_t w, int32_t h) {
    ensure_init();

    // The requested size is the logical design resolution (cbEnchanted sets
    // defaultWidth/Height here). Record it before the display logic so the
    // camera centers correctly even if the display fails to open (headless).
    design_w = w;
    design_h = h;

    if (display) {
        al_destroy_display(display);
    }
    // Request vsync so the present is throttled to the monitor refresh; without
    // it a `Repeat ... DrawScreen ... Forever` loop spins the render thread at
    // max FPS (100% CPU). Best-effort — a driver may ignore the suggestion.
    al_set_new_display_option(ALLEGRO_VSYNC, 1, ALLEGRO_SUGGEST);
    display = al_create_display(w, h);
    if (!display) return;
    screen_w = w;
    screen_h = h;

    if (event_queue) {
        al_destroy_event_queue(event_queue);
    }
    event_queue = al_create_event_queue();
    al_register_event_source(event_queue, al_get_display_event_source(display));
    al_register_event_source(event_queue, al_get_mouse_event_source());
    al_register_event_source(event_queue, al_get_keyboard_event_source());

    al_set_target_backbuffer(display);
    current_target = al_get_backbuffer(display);
    al_set_blender(ALLEGRO_ADD, ALLEGRO_ONE, ALLEGRO_ZERO);

    draw_color  = al_map_rgb(255, 255, 255);
    clear_color = al_map_rgb(0, 0, 0);
    al_clear_to_color(clear_color);

    fps_last_sample = al_get_time();
    fps_frame_count = 0;
    fps_value       = 0;
}

// Screen with an explicit window mode (0=fullscreen, 1=windowed, 2=resizable).
// We honor the mode via display flags but otherwise reuse cb_rt_screen's setup.
extern "C" void cb_rt_screen_mode(int32_t w, int32_t h, int32_t mode) {
    int flags = 0;
    switch (mode) {
        case 0:  flags = ALLEGRO_FULLSCREEN; break;
        case 2:  flags = ALLEGRO_RESIZABLE;  break;
        case 1:
        default: flags = ALLEGRO_WINDOWED;   break;
    }
    al_set_new_display_flags(flags);
    cb_rt_screen(w, h);
    al_set_new_display_flags(0);
}

// Screen(w, h, depth, mode): `depth` is accepted for source compatibility but
// ignored — the runtime always uses a 32-bit ARGB backbuffer. `mode` matches
// the 3-arg form (0=fullscreen, 1=windowed, 2/3=resizable).
extern "C" void cb_rt_screen_depth_mode(int32_t w, int32_t h, int32_t depth,
                                        int32_t mode) {
    (void)depth;
    cb_rt_screen_mode(w, h, mode);
}

// Screen() function form: the screen render-target buffer id. We model only a
// single implicit screen buffer, whose id is 0.
extern "C" int32_t cb_rt_screen_buffer_id(void) {
    return 0;
}

// Whole-screen gamma. Stored as ratios (cbEnchanted divides by 255); not
// applied (Allegro 5 exposes no portable display-gamma ramp).
extern "C" void cb_rt_screen_gamma(int32_t r, int32_t g, int32_t b) {
    gamma_r = r / 255.0;
    gamma_g = g / 255.0;
    gamma_b = b / 255.0;
}

// Saves the screen backbuffer to an image file. No-op without a display.
extern "C" void cb_rt_screenshot(const CbString* path) {
    if (!display || !path) return;
    std::size_t len = cb_rt_string_len(path);
    if (len == 0) return;
    std::string p(reinterpret_cast<const char*>(cb_rt_string_data(path)), len);
    al_save_bitmap(p.c_str(), al_get_backbuffer(display));
}

// Renders the persistent AddText queue onto the backbuffer (defined in the Text
// & fonts section). Called each frame just before the flip.
static void render_queued_texts(void);

// Shared DrawScreen body. `clear_after` controls whether the backbuffer is
// cleared once events are drained (the `cls` flag of the 2-arg form).
static void do_draw_screen(bool clear_after) {
    if (!display) return;

    // FD-036 Phase 3: composite the active tilemap (background then foreground)
    // through the camera onto the backbuffer — on top of this frame's user draws
    // and beneath the AddText overlay (mirrors cbEnchanted's DrawScreen ->
    // drawObjects ordering). A no-op when no map is loaded. Ensure the backbuffer
    // is the target first (a stray DrawToImage must not redirect the map).
    al_set_target_backbuffer(display);
    cb_map_render_active();

    // Composite queued (Locate/AddText) text onto this frame before presenting.
    render_queued_texts();

    al_flip_display();

    // FPS sampling once per second.
    fps_frame_count++;
    double now = al_get_time();
    if (now - fps_last_sample >= 1.0) {
        fps_value       = fps_frame_count;
        fps_frame_count = 0;
        fps_last_sample = now;
    }

    // Advance the input state machine for this frame (FD-013 Batch 5): clear
    // the per-key/button "changed" bits and zero movement deltas, then route
    // every queued event into the input module before processing window events.
    cb_input_frame_begin();

    ALLEGRO_EVENT ev;
    while (al_get_next_event(event_queue, &ev)) {
        cb_input_handle_event(&ev);
        if (ev.type == ALLEGRO_EVENT_DISPLAY_CLOSE) {
            // FD-015: route window-close through the trap channel for a clean
            // Halt/Ok(0) termination instead of exit(0). Tear down our own
            // display here (about_to_exit is reserved/null), ask the host to
            // exit, and return — the interpreter drains the pending Exit(0)
            // right after this runtime call returns. The `return` is essential:
            // `display` is now null and the code below would deref it. Fall
            // back to exit(0) only if no host is connected.
            al_destroy_display(display);
            display = nullptr;
            const CbHostApi* h = cb_host();
            if (h) {
                h->request_exit(0);
                return;
            }
            exit(0);
        }
    }

    al_set_target_backbuffer(display);
    current_target = al_get_backbuffer(display);
    if (clear_after) {
        al_clear_to_color(clear_color);
    }
}

// 0-arg DrawScreen always clears the backbuffer (legacy default).
extern "C" void cb_rt_drawscreen(void) {
    do_draw_screen(true);
}

// DrawScreen(cls, vsync): `cls`=0 keeps the backbuffer contents, non-zero
// clears. `vsync` is honored at display creation; per-frame it is accepted but
// has no extra effect beyond the flip (documented divergence).
extern "C" void cb_rt_drawscreen_args(int32_t cls, int32_t vsync) {
    (void)vsync;
    do_draw_screen(cls != 0);
}

extern "C" void cb_rt_cls(void) {
    if (!current_target) return;
    al_set_target_bitmap(current_target);
    al_clear_to_color(clear_color);
}

extern "C" void cb_rt_cls_color(int32_t r, int32_t g, int32_t b) {
    clear_color = al_map_rgb((unsigned char)r, (unsigned char)g, (unsigned char)b);
}

extern "C" void cb_rt_cls_color_a(int32_t r, int32_t g, int32_t b, int32_t a) {
    clear_color = al_map_rgba((unsigned char)r, (unsigned char)g,
                              (unsigned char)b, (unsigned char)a);
}

extern "C" void cb_rt_draw_to_screen(void) {
    if (!display) return;
    al_set_target_backbuffer(display);
    current_target = al_get_backbuffer(display);
}

extern "C" int32_t cb_rt_fps(void) {
    return fps_value;
}

// ─── Lock / Unlock ─────────────────────────────────────────────────────
//
// state: 0=read/write, 1=read-only, 2=write-only (legacy mapping).

static int lock_flags_for(int32_t state) {
    switch (state) {
        case 1:  return ALLEGRO_LOCK_READONLY;
        case 2:  return ALLEGRO_LOCK_WRITEONLY;
        default: return ALLEGRO_LOCK_READWRITE;
    }
}

extern "C" void cb_rt_lock(void) {
    if (current_target) al_lock_bitmap(current_target, ALLEGRO_PIXEL_FORMAT_ANY,
                                       ALLEGRO_LOCK_READWRITE);
}

extern "C" void cb_rt_lock_state(int32_t state) {
    if (current_target) al_lock_bitmap(current_target, ALLEGRO_PIXEL_FORMAT_ANY,
                                       lock_flags_for(state));
}

extern "C" void cb_rt_lock_image(CbImage* img) {
    if (img && img->bmp) al_lock_bitmap(img->bmp, ALLEGRO_PIXEL_FORMAT_ANY,
                                        ALLEGRO_LOCK_READWRITE);
}

extern "C" void cb_rt_lock_image_state(CbImage* img, int32_t state) {
    if (img && img->bmp) al_lock_bitmap(img->bmp, ALLEGRO_PIXEL_FORMAT_ANY,
                                        lock_flags_for(state));
}

extern "C" void cb_rt_unlock(void) {
    if (current_target) al_unlock_bitmap(current_target);
}

extern "C" void cb_rt_unlock_image(CbImage* img) {
    if (img && img->bmp) al_unlock_bitmap(img->bmp);
}

// ─── Drawing primitives ────────────────────────────────────────────────

extern "C" void cb_rt_color(int32_t r, int32_t g, int32_t b) {
    draw_color = al_map_rgb((unsigned char)r, (unsigned char)g, (unsigned char)b);
}

extern "C" void cb_rt_color_a(int32_t r, int32_t g, int32_t b, int32_t a) {
    draw_color = al_map_rgba((unsigned char)r, (unsigned char)g,
                             (unsigned char)b, (unsigned char)a);
}

// Component of the current draw color: 1=R, 2=G, 3=B, 4=A; anything else 0.
extern "C" int32_t cb_rt_get_rgb(int32_t channel) {
    unsigned char r, g, b, a;
    al_unmap_rgba(draw_color, &r, &g, &b, &a);
    switch (channel) {
        case 1:  return r;
        case 2:  return g;
        case 3:  return b;
        case 4:  return a;
        default: return 0;
    }
}

// Reads a pixel from the current render target and makes it the draw color.
// (cbEnchanted reads the window target; we read the current target so the
// behaviour is well-defined when drawing onto an image too.)
extern "C" void cb_rt_pick_color(int32_t x, int32_t y) {
    if (!current_target) return;
    draw_color = al_get_pixel(current_target, x, y);
}

// Toggles 2D antialiasing / smoothing. We model it as linear filtering applied
// to bitmaps created afterward (full primitive AA needs a multisampled display).
extern "C" void cb_rt_smooth_2d(int32_t enabled) {
    smooth_2d = enabled != 0;
}

// ─── DrawToWorld transform (FD-036 Phase 2) ─────────────────────────────
//
// When a DrawToWorld category flag is set AND we are drawing to the screen
// (not an image — cbEnchanted's `!drawingOnImage()`), a user draw command runs
// under the camera's world transform; otherwise under identity. We set it at the
// top of each participating command and restore identity after, so a world draw
// never leaks into a later screen draw or an image-processing copy (which all
// assume identity). `category` is one of the cb_camera flag getters.
static bool gfx_begin_world(int category_flag) {
    bool world = category_flag &&
                 display && current_target == al_get_backbuffer(display);
    if (world) {
        al_use_transform(cb_camera_render_transform());
    }
    return world;
}

static void gfx_end_world(bool active) {
    if (active) {
        ALLEGRO_TRANSFORM id;
        al_identity_transform(&id);
        al_use_transform(&id);
    }
}

extern "C" void cb_rt_line(double x1, double y1, double x2, double y2) {
    if (!current_target) return;
    bool w = gfx_begin_world(cb_camera_draw_cmd_to_world());
    al_draw_line((float)x1, (float)y1, (float)x2, (float)y2, draw_color, 1.0f);
    gfx_end_world(w);
}

// `d` is a diameter (CoolBasic convention); Allegro draws by radius.
extern "C" void cb_rt_circle(double x, double y, double d) {
    if (!current_target) return;
    bool world = gfx_begin_world(cb_camera_draw_cmd_to_world());
    float r = (float)d / 2.0f;
    al_draw_circle((float)x + r, (float)y + r, r, draw_color, 1.0f);
    gfx_end_world(world);
}

extern "C" void cb_rt_circle_fill(double x, double y, double d, int32_t fill) {
    if (!current_target) return;
    bool world = gfx_begin_world(cb_camera_draw_cmd_to_world());
    float r = (float)d / 2.0f;
    if (fill) {
        al_draw_filled_circle((float)x + r, (float)y + r, r, draw_color);
    } else {
        al_draw_circle((float)x + r, (float)y + r, r, draw_color, 1.0f);
    }
    gfx_end_world(world);
}

extern "C" void cb_rt_box(double x, double y, double w, double h) {
    if (!current_target) return;
    bool world = gfx_begin_world(cb_camera_draw_cmd_to_world());
    al_draw_rectangle((float)x, (float)y, (float)(x + w), (float)(y + h),
                      draw_color, 1.0f);
    gfx_end_world(world);
}

extern "C" void cb_rt_box_fill(double x, double y, double w, double h, int32_t fill) {
    if (!current_target) return;
    bool world = gfx_begin_world(cb_camera_draw_cmd_to_world());
    if (fill) {
        al_draw_filled_rectangle((float)x, (float)y, (float)(x + w), (float)(y + h),
                                 draw_color);
    } else {
        al_draw_rectangle((float)x, (float)y, (float)(x + w), (float)(y + h),
                          draw_color, 1.0f);
    }
    gfx_end_world(world);
}

extern "C" void cb_rt_dot(double x, double y) {
    if (!current_target) return;
    bool world = gfx_begin_world(cb_camera_draw_cmd_to_world());
    al_draw_pixel((float)x, (float)y, draw_color);
    gfx_end_world(world);
}

// Ellipse with top-left (x,y) and full diameters (w,h); matches our Circle's
// top-left convention. Allegro draws from the center with radii.
extern "C" void cb_rt_ellipse(double x, double y, double w, double h, int32_t fill) {
    if (!current_target) return;
    bool world = gfx_begin_world(cb_camera_draw_cmd_to_world());
    float rx = (float)w / 2.0f;
    float ry = (float)h / 2.0f;
    float cx = (float)x + rx;
    float cy = (float)y + ry;
    if (fill) {
        al_draw_filled_ellipse(cx, cy, rx, ry, draw_color);
    } else {
        al_draw_ellipse(cx, cy, rx, ry, draw_color, 1.0f);
    }
    gfx_end_world(world);
}

// ─── Pixel operations ──────────────────────────────────────────────────
//
// al_put_pixel writes to the current target bitmap; the caller is expected to
// have Lock()ed it first. Pixel params are CB Ints (i32) per the ABI.

extern "C" void cb_rt_put_pixel(int32_t x, int32_t y, int32_t r, int32_t g, int32_t b) {
    al_put_pixel(x, y, al_map_rgb((unsigned char)r, (unsigned char)g, (unsigned char)b));
}

extern "C" void cb_rt_put_pixel_a(int32_t x, int32_t y, int32_t r, int32_t g,
                                  int32_t b, int32_t a) {
    al_put_pixel(x, y, al_map_rgba((unsigned char)r, (unsigned char)g,
                                   (unsigned char)b, (unsigned char)a));
}

// Packed 32-bit ARGB.
extern "C" void cb_rt_put_pixel_argb(int32_t x, int32_t y, int32_t argb) {
    uint32_t p = (uint32_t)argb;
    al_put_pixel(x, y, al_map_rgba((p >> 16) & 0xFF, (p >> 8) & 0xFF,
                                   p & 0xFF, (p >> 24) & 0xFF));
}

// Packs an ALLEGRO_COLOR to 32-bit ARGB (the runtime's retained format; see
// FD-017 Q2 — diverges from the spec's nominal 0xRRGGBB but matches what
// cbEnchanted's GetPixel actually returns).
static int32_t pack_argb(ALLEGRO_COLOR color) {
    unsigned char r, g, b, a;
    al_unmap_rgba(color, &r, &g, &b, &a);
    return ((int32_t)a << 24) | ((int32_t)r << 16) | ((int32_t)g << 8) | (int32_t)b;
}

// Reads a pixel from `img` as packed 32-bit ARGB. (Image must be Lock()ed for
// reliable reads on a video bitmap; memory bitmaps read directly.)
extern "C" int32_t cb_rt_get_pixel(const CbImage* img, int32_t x, int32_t y) {
    if (!img || !img->bmp) return 0;
    return pack_argb(al_get_pixel(img->bmp, x, y));
}

// GetPixel2(x, y) / GetPixel(x, y): reads the CURRENT render target as packed
// ARGB (the buffer=0 form; we model only the current target).
extern "C" int32_t cb_rt_get_pixel2(int32_t x, int32_t y) {
    if (!current_target) return 0;
    return pack_argb(al_get_pixel(current_target, x, y));
}

// CopyBox: blits a w×h region from (srcX,srcY) to (destX,destY). Buffer ids are
// accepted for source compatibility but only the current render target (id 0)
// is modelled, so both source and destination are the current target; the copy
// goes through a temporary so a self-overlapping move is well-defined.
extern "C" void cb_rt_copy_box(double srcX, double srcY, double w, double h,
                               double destX, double destY,
                               int32_t srcBuf, int32_t destBuf) {
    (void)srcBuf;
    (void)destBuf;
    if (!current_target || w <= 0 || h <= 0) return;
    int prev_flags = al_get_new_bitmap_flags();
    if (!al_get_current_display()) {
        al_set_new_bitmap_flags(ALLEGRO_MEMORY_BITMAP);
    }
    ALLEGRO_BITMAP* tmp = al_create_bitmap((int)w, (int)h);
    al_set_new_bitmap_flags(prev_flags);
    if (!tmp) return;

    al_set_target_bitmap(tmp);
    al_clear_to_color(al_map_rgba(0, 0, 0, 0));
    al_draw_bitmap_region(current_target, (float)srcX, (float)srcY,
                          (float)w, (float)h, 0.0f, 0.0f, 0);
    al_set_target_bitmap(current_target);
    al_draw_bitmap(tmp, (float)destX, (float)destY, 0);
    al_destroy_bitmap(tmp);
}

// ─── Image handles ─────────────────────────────────────────────────────

extern "C" CbImage* cb_rt_make_image(int32_t w, int32_t h) {
    ensure_init();
    // Without a display, video bitmaps cannot be created — fall back to a
    // memory bitmap so images (and the headless pixel round-trip test) work
    // before any Screen() call. With a display, keep the default (video).
    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) {
        flags |= ALLEGRO_MEMORY_BITMAP;
    }
    // Smooth2D: linear min/mag filtering on bitmaps created while enabled.
    if (smooth_2d) {
        flags |= ALLEGRO_MIN_LINEAR | ALLEGRO_MAG_LINEAR;
    }
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* bmp = al_create_bitmap(w, h);
    al_set_new_bitmap_flags(prev_flags);
    if (!bmp) return nullptr;

    // Clear to opaque black (matches cbEnchanted's MakeImage), so the contents
    // are defined — fresh bitmaps are otherwise undefined, which would make
    // pixel reads / ImagesCollide nondeterministic.
    ALLEGRO_BITMAP* prev_target = al_get_target_bitmap();
    al_set_target_bitmap(bmp);
    al_clear_to_color(al_map_rgb(0, 0, 0));
    if (prev_target) al_set_target_bitmap(prev_target);

    if (default_mask_on) al_convert_mask_to_alpha(bmp, default_mask_color);

    CbImage* img = new CbImage{bmp};
    return img;
}

extern "C" CbImage* cb_rt_load_image(const CbString* path) {
    ensure_init();
    std::string p;
    if (path) {
        std::size_t len = cb_rt_string_len(path);
        if (len > 0) {
            p.assign(reinterpret_cast<const char*>(cb_rt_string_data(path)), len);
        }
    }
    ALLEGRO_BITMAP* bmp = al_load_bitmap(p.c_str());
    if (!bmp) return nullptr;

    if (default_mask_on) al_convert_mask_to_alpha(bmp, default_mask_color);

    CbImage* img = new CbImage{bmp};
    return img;
}

extern "C" void cb_rt_draw_image(const CbImage* img, double x, double y) {
    if (!img || !img->bmp || !current_target) return;
    bool world = gfx_begin_world(cb_camera_image_to_world());
    al_draw_bitmap(img->bmp, (float)x - img->hotspot_x, (float)y - img->hotspot_y, 0);
    gfx_end_world(world);
}

extern "C" void cb_rt_mask_image(CbImage* img, int32_t r, int32_t g, int32_t b) {
    if (!img || !img->bmp) return;
    al_convert_mask_to_alpha(img->bmp, al_map_rgb((unsigned char)r,
                                                  (unsigned char)g, (unsigned char)b));
}

extern "C" void cb_rt_mask_image_a(CbImage* img, int32_t r, int32_t g, int32_t b, int32_t a) {
    if (!img || !img->bmp) return;
    al_convert_mask_to_alpha(img->bmp, al_map_rgba((unsigned char)r, (unsigned char)g,
                                                   (unsigned char)b, (unsigned char)a));
}

extern "C" void cb_rt_draw_to_image(CbImage* img) {
    if (!img || !img->bmp) return;
    al_set_target_bitmap(img->bmp);
    current_target = img->bmp;
}

extern "C" int32_t cb_rt_image_width(const CbImage* img) {
    if (!img || !img->bmp) return 0;
    return al_get_bitmap_width(img->bmp);
}

extern "C" int32_t cb_rt_image_height(const CbImage* img) {
    if (!img || !img->bmp) return 0;
    return al_get_bitmap_height(img->bmp);
}

// Frees the image's bitmap and handle. If this image is the active render
// target, drop the dangling target pointer first.
extern "C" void cb_rt_delete_image(CbImage* img) {
    if (!img) return;
    if (img->bmp) {
        if (current_target == img->bmp) {
            current_target = display ? al_get_backbuffer(display) : nullptr;
            if (current_target) al_set_target_bitmap(current_target);
        }
        al_destroy_bitmap(img->bmp);
    }
    delete img;
}

// ─── FD-017 image additions (single-frame) ─────────────────────────────
//
// Multi-frame sprite sheets are deferred (FD-017 Q3), so the `frame` parameters
// of the cbEnchanted signatures are dropped here; SaveImage keeps a `frame` arg
// for source compatibility but ignores it.

// DefaultMask(enabled, r, g, b): mask color applied to future MakeImage/
// LoadImage results. enabled=0 turns it off.
extern "C" void cb_rt_default_mask(int32_t enabled, int32_t r, int32_t g, int32_t b) {
    default_mask_on = enabled != 0;
    if (default_mask_on) {
        default_mask_color = al_map_rgb((unsigned char)r, (unsigned char)g,
                                        (unsigned char)b);
    }
}

// Copies an image, its hotspot, and its frame metadata (FD-036).
extern "C" CbImage* cb_rt_clone_image(const CbImage* img) {
    if (!img || !img->bmp) return nullptr;
    ALLEGRO_BITMAP* b = al_clone_bitmap(img->bmp);
    if (!b) return nullptr;
    CbImage* out = new CbImage{b, img->hotspot_x, img->hotspot_y};
    out->frame_w = img->frame_w;
    out->frame_h = img->frame_h;
    out->anim_begin = img->anim_begin;
    out->anim_length = img->anim_length;
    return out;
}

// Resizes (scales) an image to w×h. Resets the hotspot to (0,0).
extern "C" void cb_rt_resize_image(CbImage* img, int32_t w, int32_t h) {
    if (!img || !img->bmp || w <= 0 || h <= 0) return;
    int ow = al_get_bitmap_width(img->bmp);
    int oh = al_get_bitmap_height(img->bmp);

    // Restore the caller's render target on exit, the way MakeImage does, so
    // building `dest` is not an observable global-state side effect (FD-022).
    ALLEGRO_BITMAP* prev = al_get_target_bitmap();

    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* dest = al_create_bitmap(w, h);
    al_set_new_bitmap_flags(prev_flags);
    if (!dest) return;

    al_set_target_bitmap(dest);
    al_clear_to_color(al_map_rgba(0, 0, 0, 0));
    al_draw_scaled_bitmap(img->bmp, 0, 0, (float)ow, (float)oh,
                          0, 0, (float)w, (float)h, 0);

    ALLEGRO_BITMAP* old = img->bmp;
    img->bmp = dest;
    if (current_target == old) current_target = dest;
    // Restore the previous target, but if it was the bitmap we just destroyed
    // point it at the replacement instead of a dangling pointer.
    al_set_target_bitmap(prev == old ? dest : prev);
    al_destroy_bitmap(old);
    img->hotspot_x = 0;
    img->hotspot_y = 0;
}

// Rotates an image `angle` degrees clockwise into a new bitmap sized to the
// rotated bounding box, and centers the hotspot (mirrors cbEnchanted).
extern "C" void cb_rt_rotate_image(CbImage* img, double angle) {
    if (!img || !img->bmp) return;
    double rad = angle / 180.0 * 3.14159265358979323846;
    double ow = al_get_bitmap_width(img->bmp);
    double oh = al_get_bitmap_height(img->bmp);
    double s = std::abs(std::sin(rad));
    double c = std::abs(std::cos(rad));
    double nw = ow * c + oh * s;
    double nh = ow * s + oh * c;
    int niw = (int)(nw + 0.999999);
    int nih = (int)(nh + 0.999999);
    if (niw < 1) niw = 1;
    if (nih < 1) nih = 1;

    // Restore the caller's render target on exit (see cb_rt_resize_image, FD-022).
    ALLEGRO_BITMAP* prev = al_get_target_bitmap();

    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* dest = al_create_bitmap(niw, nih);
    al_set_new_bitmap_flags(prev_flags);
    if (!dest) return;

    al_set_target_bitmap(dest);
    al_clear_to_color(al_map_rgba(0, 0, 0, 0));
    al_draw_rotated_bitmap(img->bmp, (float)(ow / 2.0), (float)(oh / 2.0),
                           (float)(niw / 2.0), (float)(nih / 2.0), (float)rad, 0);

    ALLEGRO_BITMAP* old = img->bmp;
    img->bmp = dest;
    if (current_target == old) current_target = dest;
    al_set_target_bitmap(prev == old ? dest : prev);
    al_destroy_bitmap(old);
    img->hotspot_x = niw / 2;
    img->hotspot_y = nih / 2;
}

// Reads a pixel from an image and makes it the current draw color.
// (PickImageColor2 is registered as an alias.)
extern "C" void cb_rt_pick_image_color(const CbImage* img, int32_t x, int32_t y) {
    if (!img || !img->bmp) return;
    draw_color = al_get_pixel(img->bmp, x, y);
}

// Writes an image to disk. `frame` is accepted but ignored (single-frame).
extern "C" void cb_rt_save_image(const CbImage* img, const CbString* path, int32_t frame) {
    (void)frame;
    if (!img || !img->bmp || !path) return;
    std::size_t len = cb_rt_string_len(path);
    if (len == 0) return;
    std::string p(reinterpret_cast<const char*>(cb_rt_string_data(path)), len);
    al_save_bitmap(p.c_str(), img->bmp);
}

// Draws an image with alpha blending; `alpha` is 0–100 (0=transparent,
// 100=opaque). Honors the hotspot.
extern "C" void cb_rt_draw_ghost_image(const CbImage* img, double x, double y, double alpha) {
    if (!img || !img->bmp || !current_target) return;
    bool world = gfx_begin_world(cb_camera_image_to_world());
    float a = (float)(alpha / 100.0);
    if (a < 0.0f) a = 0.0f;
    if (a > 1.0f) a = 1.0f;
    al_draw_tinted_bitmap(img->bmp, al_map_rgba_f(1.0f, 1.0f, 1.0f, a),
                          (float)x - img->hotspot_x, (float)y - img->hotspot_y, 0);
    gfx_end_world(world);
}

// Draws the source sub-rectangle (sx,sy,sw,sh) of an image at (tx,ty).
extern "C" void cb_rt_draw_image_box(const CbImage* img, double sx, double sy,
                                     double sw, double sh, double tx, double ty) {
    if (!img || !img->bmp || !current_target) return;
    bool world = gfx_begin_world(cb_camera_image_to_world());
    al_draw_bitmap_region(img->bmp, (float)sx, (float)sy, (float)sw, (float)sh,
                          (float)tx, (float)ty, 0);
    gfx_end_world(world);
}

// Per-image hotspot (scale/rotate/draw origin). x<0 || y<0 auto-centers.
// NOTE: cbEnchanted's HotSpot takes an integer id where 0/1 toggle a global
// default-hotspot; that overloading has no equivalent here because `Image` is
// an opaque handle, not an int id — so this is the per-image form only.
extern "C" void cb_rt_hotspot(CbImage* img, int32_t x, int32_t y) {
    if (!img || !img->bmp) return;
    if (x < 0 || y < 0) {
        // FD-036: center on a single frame when frame size is set, else on the
        // whole image (matches cbEnchanted CBImage::setHotspot).
        if (img->frame_w > 0 && img->frame_h > 0) {
            img->hotspot_x = img->frame_w / 2;
            img->hotspot_y = img->frame_h / 2;
        } else {
            img->hotspot_x = al_get_bitmap_width(img->bmp) / 2;
            img->hotspot_y = al_get_bitmap_height(img->bmp) / 2;
        }
    } else {
        img->hotspot_x = x;
        img->hotspot_y = y;
    }
}

// rect_overlap lives in cb_geom.h (FD-022) so the unit tests can exercise it
// without pulling in Allegro.

// Bounding-box overlap between two placed images (Y negated for world space,
// matching cbEnchanted/BoxOverlap).
extern "C" int32_t cb_rt_images_overlap(const CbImage* a, double x1, double y1,
                                        const CbImage* b, double x2, double y2) {
    if (!a || !a->bmp || !b || !b->bmp) return 0;
    double w1 = al_get_bitmap_width(a->bmp), h1 = al_get_bitmap_height(a->bmp);
    double w2 = al_get_bitmap_width(b->bmp), h2 = al_get_bitmap_height(b->bmp);
    return rect_overlap(x1, -y1, w1, h1, x2, -y2, w2, h2) ? 1 : 0;
}

// Pixel-precise collision between two placed image frames. `frame1`/`frame2`
// are accepted but ignored (single-frame). A pixel is "solid" when its alpha is
// non-zero, so MaskImage/DefaultMask transparency participates correctly.
extern "C" int32_t cb_rt_images_collide(const CbImage* a, double x1, double y1, int32_t frame1,
                                        const CbImage* b, double x2, double y2, int32_t frame2) {
    (void)frame1;
    (void)frame2;
    if (!a || !a->bmp || !b || !b->bmp) return 0;
    double w1 = al_get_bitmap_width(a->bmp), h1 = al_get_bitmap_height(a->bmp);
    double w2 = al_get_bitmap_width(b->bmp), h2 = al_get_bitmap_height(b->bmp);
    // Broad phase uses rect_overlap's world-space form (Y negated), matching
    // cb_rt_images_overlap; the narrow phase below works in screen-space
    // top-left (where al_get_pixel row 0 is the top). These two conventions
    // look mixed but are equivalent: negating both rectangles' Y is symmetric,
    // so the overlap boolean is identical to a direct screen-space test, and
    // the scan box is exactly the (non-empty) screen-space intersection the
    // AABB just gated. Verified true/false by the collide_images golden fixture.
    if (!rect_overlap(x1, -y1, w1, h1, x2, -y2, w2, h2)) return 0;

    int xmin = (int)std::max(x1, x2);
    int ymin = (int)std::max(y1, y2);
    int xmax = (int)std::min(x1 + w1, x2 + w2);
    int ymax = (int)std::min(y1 + h1, y2 + h2);
    for (int y = ymin; y < ymax; ++y) {
        for (int x = xmin; x < xmax; ++x) {
            unsigned char r, g, bl, a1, a2;
            al_unmap_rgba(al_get_pixel(a->bmp, x - (int)x1, y - (int)y1), &r, &g, &bl, &a1);
            al_unmap_rgba(al_get_pixel(b->bmp, x - (int)x2, y - (int)y2), &r, &g, &bl, &a2);
            if (a1 != 0 && a2 != 0) return 1;
        }
    }
    return 0;
}

// ─── FD-036 multi-frame sprite sheets ──────────────────────────────────
//
// A multi-frame image stores one bitmap sliced on the fly into frame_w×frame_h
// cells (LoadAnimImage sets the frame size). The frame draw overloads below sit
// alongside the single-frame ones above; the no-frame catalog rows keep working.

// Source sub-rect for `frame` of a multi-frame image. Returns false for a
// single-frame image (anim_length==0 or no frame size) — the caller draws the
// whole bitmap. `frame` is 0-based and taken modulo framesX (NOT clamped to
// anim_length), matching cbEnchanted. The row/offset math deliberately fixes
// cbEnchanted bugs #1/#2 (cbimage.cpp:64,67 used `/framesY` and `*frameWidth`,
// correct only for square single-row sheets); see FD-036.
static bool image_frame_src_rect(const CbImage* img, int32_t frame,
                                 float& left, float& top, float& w, float& h) {
    if (!img || !img->bmp || img->anim_length == 0 ||
        img->frame_w <= 0 || img->frame_h <= 0) {
        return false;
    }
    int frames_x = al_get_bitmap_width(img->bmp) / img->frame_w;
    if (frames_x <= 0) return false;
    int col = frame % frames_x;
    int row = frame / frames_x;                 // FIX #1: /framesX, not /framesY
    left = (float)(col * img->frame_w);
    top  = (float)(row * img->frame_h);         // FIX #2: *frame_h, not *frame_w
    w = (float)img->frame_w;
    h = (float)img->frame_h;
    return true;
}

// LoadAnimImage(path, frameW, frameH, startFrame, animLength): loads a sprite
// sheet and records its frame geometry. Mirrors MakeImage's memory-bitmap
// fallback so sheets load without a display (al_load_bitmap would otherwise try
// to create a video bitmap and fail headless). Returns Null on load failure.
extern "C" CbImage* cb_rt_load_anim_image(const CbString* path, int32_t frame_w,
                                          int32_t frame_h, int32_t start_frame,
                                          int32_t anim_length) {
    ensure_init();
    std::string p;
    if (path) {
        std::size_t len = cb_rt_string_len(path);
        if (len > 0) {
            p.assign(reinterpret_cast<const char*>(cb_rt_string_data(path)), len);
        }
    }
    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    if (smooth_2d) flags |= ALLEGRO_MIN_LINEAR | ALLEGRO_MAG_LINEAR;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* bmp = al_load_bitmap(p.c_str());
    al_set_new_bitmap_flags(prev_flags);
    if (!bmp) return nullptr;

    if (default_mask_on) al_convert_mask_to_alpha(bmp, default_mask_color);

    CbImage* img = new CbImage{bmp};
    img->frame_w = frame_w;
    img->frame_h = frame_h;
    img->anim_begin = start_frame;
    img->anim_length = anim_length;
    return img;
}

// MakeImage(w, h, frameCount): the 3-arg overload. `frameCount` is popped and
// ignored — cbEnchanted has no frame size to slice by, so a made image is always
// single-frame. Identical to the 2-arg MakeImage otherwise.
extern "C" CbImage* cb_rt_make_image_frames(int32_t w, int32_t h, int32_t frame_count) {
    (void)frame_count;
    return cb_rt_make_image(w, h);
}

// DrawImage(img, x, y, frame): draws one frame, honoring the hotspot. Falls back
// to the whole bitmap for a single-frame image.
extern "C" void cb_rt_draw_image_frame(const CbImage* img, double x, double y, int32_t frame) {
    if (!img || !img->bmp || !current_target) return;
    bool world = gfx_begin_world(cb_camera_image_to_world());
    float l, t, w, h;
    if (image_frame_src_rect(img, frame, l, t, w, h)) {
        al_draw_bitmap_region(img->bmp, l, t, w, h,
                              (float)x - img->hotspot_x, (float)y - img->hotspot_y, 0);
    } else {
        al_draw_bitmap(img->bmp, (float)x - img->hotspot_x, (float)y - img->hotspot_y, 0);
    }
    gfx_end_world(world);
}

// DrawImage(img, x, y, frame, useMask): the documented 5-arg form.
// TODO(FD-036): `useMask` is accepted but ignored — this port's masking is
// destructive (MaskImage bakes alpha into the single bitmap; there is no unmasked
// copy to select between). Revisit by storing masked+unmasked bitmaps if a
// program needs per-draw mask selection.
extern "C" void cb_rt_draw_image_frame_mask(const CbImage* img, double x, double y,
                                            int32_t frame, int32_t use_mask) {
    (void)use_mask;
    cb_rt_draw_image_frame(img, x, y, frame);
}

// DrawGhostImage(img, x, y, frame, alpha): alpha-blended single frame.
extern "C" void cb_rt_draw_ghost_image_frame(const CbImage* img, double x, double y,
                                             int32_t frame, double alpha) {
    if (!img || !img->bmp || !current_target) return;
    bool world = gfx_begin_world(cb_camera_image_to_world());
    float a = (float)(alpha / 100.0);
    if (a < 0.0f) a = 0.0f;
    if (a > 1.0f) a = 1.0f;
    ALLEGRO_COLOR tint = al_map_rgba_f(1.0f, 1.0f, 1.0f, a);
    float l, t, w, h;
    if (image_frame_src_rect(img, frame, l, t, w, h)) {
        al_draw_tinted_bitmap_region(img->bmp, tint, l, t, w, h,
                                     (float)x - img->hotspot_x, (float)y - img->hotspot_y, 0);
    } else {
        al_draw_tinted_bitmap(img->bmp, tint,
                              (float)x - img->hotspot_x, (float)y - img->hotspot_y, 0);
    }
    gfx_end_world(world);
}

// DrawImageBox(img, sx, sy, sw, sh, tx, ty, frame): same source/dest convention
// as the 7-arg cb_rt_draw_image_box (source rect (sx,sy,sw,sh) → dest (tx,ty));
// for a multi-frame image the source origin is shifted to the frame's top-left.
// Does not apply the hotspot (matches the non-frame box draw).
extern "C" void cb_rt_draw_image_box_frame(const CbImage* img, double sx, double sy,
                                           double sw, double sh, double tx, double ty,
                                           int32_t frame) {
    if (!img || !img->bmp || !current_target) return;
    bool world = gfx_begin_world(cb_camera_image_to_world());
    float l, t, w, h;
    double ox = 0.0, oy = 0.0;
    if (image_frame_src_rect(img, frame, l, t, w, h)) {
        ox = l;
        oy = t;
    }
    al_draw_bitmap_region(img->bmp, (float)(ox + sx), (float)(oy + sy),
                          (float)sw, (float)sh, (float)tx, (float)ty, 0);
    gfx_end_world(world);
}

// DrawImageBox(..., frame, useMask): the documented 9-arg form. useMask ignored
// (see the TODO on cb_rt_draw_image_frame_mask).
extern "C" void cb_rt_draw_image_box_frame_mask(const CbImage* img, double sx, double sy,
                                                double sw, double sh, double tx, double ty,
                                                int32_t frame, int32_t use_mask) {
    (void)use_mask;
    cb_rt_draw_image_box_frame(img, sx, sy, sw, sh, tx, ty, frame);
}

// ─── Screen queries ────────────────────────────────────────────────────

extern "C" int32_t cb_rt_screen_width(void) {
    return screen_w;
}

extern "C" int32_t cb_rt_screen_height(void) {
    return screen_h;
}

// Color depth in bits. The runtime always uses a 32-bit ARGB backbuffer.
extern "C" int32_t cb_rt_screen_depth(void) {
    return 32;
}

// ─── Internal glue for cb_input.cpp (FD-017) ───────────────────────────
//
// cb_gfx.cpp owns the display and event queue; the blocking/cursor input
// functions (WaitKey/WaitMouse/PositionMouse/ShowMouse) need them. Declared in
// cb_input.h, not the catalog ABI. Both return null when no window is open, so
// those input functions degrade to a safe no-op/0 headlessly.
extern "C" ALLEGRO_DISPLAY* cb_gfx_display(void) {
    return display;
}

extern "C" ALLEGRO_EVENT_QUEUE* cb_gfx_event_queue(void) {
    return event_queue;
}

// Internal glue for cb_camera.cpp (FD-036): the logical design resolution the
// camera centers its world transform on. Declared in cb_camera.h.
extern "C" void cb_gfx_design_size(int32_t* w, int32_t* h) {
    if (w) *w = design_w;
    if (h) *h = design_h;
}

// Whether a graphics mode is available. Best-effort: any positive resolution is
// reported as available (we create windowed displays on demand; `depth` is
// ignored since the backbuffer is always 32-bit).
extern "C" int32_t cb_rt_gfx_mode_exists(int32_t w, int32_t h, int32_t depth) {
    (void)depth;
    return (w > 0 && h > 0) ? 1 : 0;
}

// ─── Text & fonts (FD-018) ─────────────────────────────────────────────
//
// Ported from cbEnchanted's TextInterface (../cbEnchanted/src/textinterface.cpp)
// onto this file's shared state: text draws in the current `draw_color` onto the
// active `current_target`, font lookup honours `smooth_2d`, and the persistent
// AddText queue re-renders every DrawScreen (render_queued_texts, above) until
// ClearText. `Font` crosses the FFI as the opaque CbFont* handle.

// Reads a CbString into a UTF-8 std::string (empty when null).
static std::string cb_text_to_utf8(const CbString* s) {
    std::string out;
    if (s) {
        std::size_t len = cb_rt_string_len(s);
        if (len > 0) {
            out.assign(reinterpret_cast<const char*>(cb_rt_string_data(s)), len);
        }
    }
    return out;
}

// Splits a UTF-8 string into its codepoint substrings (1–4 bytes each). Used by
// VerticalText so it advances one *character* per line, not one byte (cbEnchanted
// iterated raw bytes — wrong for multibyte text).
static std::vector<std::string> utf8_chars(const std::string& s) {
    std::vector<std::string> out;
    std::size_t i = 0;
    while (i < s.size()) {
        unsigned char c = (unsigned char)s[i];
        std::size_t n = 1;
        if      (c >= 0xF0) n = 4;
        else if (c >= 0xE0) n = 3;
        else if (c >= 0xC0) n = 2;
        if (i + n > s.size()) n = 1;  // truncated/invalid lead byte: emit one byte
        out.push_back(s.substr(i, n));
        i += n;
    }
    return out;
}

// Composites the persistent AddText queue onto the display backbuffer. Forward-
// declared above do_draw_screen, which calls it once per frame before the flip.
static void render_queued_texts(void) {
    if (queued_texts.empty() || !display) return;
    al_set_target_backbuffer(display);
    for (const QueuedText& t : queued_texts) {
        if (t.font) {
            al_draw_text(t.font, t.col, (float)t.x, (float)t.y, 0, t.utf8.c_str());
        }
    }
}

// Text(x, y, s): draws immediately at (x, y) in the current font/color onto the
// active render target. Honors DrawToWorld's text flag (FD-036): when set and
// drawing to the screen, (x, y) is interpreted in world coordinates.
extern "C" void cb_rt_text(double x, double y, const CbString* s) {
    ensure_init();
    if (!current_target || !current_font) return;
    bool world = gfx_begin_world(cb_camera_text_to_world());
    std::string txt = cb_text_to_utf8(s);
    al_draw_text(current_font, draw_color, (float)x, (float)y, 0, txt.c_str());
    gfx_end_world(world);
}

// CenterText(x, y, s, style): style 0=horizontal centering, 1=vertical, 2=both
// (mirrors cbEnchanted's HCenter/VCenter/Center alignment).
extern "C" void cb_rt_center_text(int32_t x, int32_t y, const CbString* s,
                                  int32_t style) {
    ensure_init();
    if (!current_target || !current_font) return;
    std::string txt = cb_text_to_utf8(s);
    float fx = (float)x;
    float fy = (float)y;
    float half_ascent = al_get_font_ascent(current_font) * 0.5f;
    switch (style) {
        case 1:  // vertical only: shift up half an ascent, left-aligned
            al_draw_text(current_font, draw_color, fx, fy - half_ascent, 0,
                         txt.c_str());
            break;
        case 2:  // both axes
            al_draw_text(current_font, draw_color, fx, fy - half_ascent,
                         ALLEGRO_ALIGN_CENTRE, txt.c_str());
            break;
        case 0:  // horizontal only
        default:
            al_draw_text(current_font, draw_color, fx, fy, ALLEGRO_ALIGN_CENTRE,
                         txt.c_str());
            break;
    }
}

// VerticalText(x, y, s): one character per line, top-to-bottom.
//
// NOTE: our docs (docs/cb_runtime.md) specify VerticalText(x, y, s); cbEnchanted
// pops its arguments as (y, x, s), a likely long-standing label swap there. We
// follow the documented (x, y, s) order.
extern "C" void cb_rt_vertical_text(int32_t x, int32_t y, const CbString* s) {
    ensure_init();
    if (!current_target || !current_font) return;
    std::string txt = cb_text_to_utf8(s);
    int32_t line_h = al_get_font_line_height(current_font);
    float fy = (float)y;
    for (const std::string& ch : utf8_chars(txt)) {
        al_draw_text(current_font, draw_color, (float)x, fy, 0, ch.c_str());
        fy += (float)line_h;
    }
}

// Locate(x, y): sets the AddText cursor.
extern "C" void cb_rt_locate(int32_t x, int32_t y) {
    text_loc_x = x;
    text_loc_y = y;
}

// AddText(s): queues persistent on-screen text at the cursor (snapshotting the
// current font/color), then advances the cursor one line.
extern "C" void cb_rt_add_text(const CbString* s) {
    ensure_init();
    QueuedText t;
    t.font = current_font;
    t.utf8 = cb_text_to_utf8(s);
    t.x    = text_loc_x;
    t.y    = text_loc_y;
    t.col  = draw_color;
    queued_texts.push_back(std::move(t));
    if (current_font) {
        text_loc_y += al_get_font_line_height(current_font);
    }
}

// ClearText(): drops the queued text and resets the cursor.
extern "C" void cb_rt_clear_text(void) {
    queued_texts.clear();
    text_loc_x = 0;
    text_loc_y = 0;
}

// LoadFont(name, size, bold, italic, underline) -> Font. `name` with a '.' is a
// file path; otherwise it is a system font family name resolved via cb_findfont.
// Smooth2D selects antialiased vs monochrome rendering. `underline` is accepted
// but not rendered (cbEnchanted TODO). Returns null (CB `0`) on failure.
extern "C" CbFont* cb_rt_load_font(const CbString* name, int32_t size,
                                   int32_t bold, int32_t italic,
                                   int32_t underline) {
    ensure_init();
    (void)underline;
    std::string fontname = cb_text_to_utf8(name);
    std::string path;
    if (fontname.find('.') != std::string::npos) {
        path = fontname;  // looks like a file path → load directly
    } else {
        path = cb_findfont(fontname.c_str(), bold != 0, italic != 0);
    }
    if (path.empty()) return nullptr;

    int flags = smooth_2d ? 0 : ALLEGRO_TTF_MONOCHROME;
    ALLEGRO_FONT* f = al_load_font(path.c_str(), size, flags);
    if (!f) return nullptr;
    return new CbFont{f};
}

// SetFont(f): makes `f` the current font; a null handle resets to the default.
extern "C" void cb_rt_set_font(CbFont* f) {
    ensure_init();
    current_font = (f && f->font) ? f->font : default_font;
}

// DeleteFont(f): frees a font. If it was the current font, falls back to the
// default. Queued AddText entries snapshot a borrowed ALLEGRO_FONT* (see
// cb_rt_add_text), so any that referenced this font are rebound to default_font
// too — otherwise render_queued_texts would dereference the freed font on the
// next DrawScreen (FD-022). default_font is process-owned and never freed here,
// so the rebind is always safe.
extern "C" void cb_rt_delete_font(CbFont* f) {
    if (!f) return;
    if (current_font == f->font) {
        current_font = default_font;
    }
    for (QueuedText& t : queued_texts) {
        if (t.font == f->font) {
            t.font = default_font;
        }
    }
    if (f->font) {
        al_destroy_font(f->font);
    }
    delete f;
}

// TextWidth(s): pixel width of `s` in the current font (0 with no font).
extern "C" int32_t cb_rt_text_width(const CbString* s) {
    ensure_init();
    if (!current_font) return 0;
    std::string txt = cb_text_to_utf8(s);
    return al_get_text_width(current_font, txt.c_str());
}

// TextHeight(s): pixel height of `s` in the current font (0 with no font).
extern "C" int32_t cb_rt_text_height(const CbString* s) {
    ensure_init();
    if (!current_font) return 0;
    std::string txt = cb_text_to_utf8(s);
    int bx, by, bw, bh;
    al_get_text_dimensions(current_font, txt.c_str(), &bx, &by, &bw, &bh);
    return bh;
}
