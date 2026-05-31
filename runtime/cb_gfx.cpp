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

#include <allegro5/allegro.h>
#include <allegro5/allegro_primitives.h>
#include <allegro5/allegro_image.h>

#include <algorithm>
#include <cmath>
#include <string>

// ─── Opaque Image handle ──────────────────────────────────────────────
//
// The CB-visible `Image` type. Declared (never defined) in cb_runtime.h as
// `struct CbImage`; defined here. Always passed/returned by pointer.
//
// FD-017 adds a hotspot — the draw/scale/rotate origin. It defaults to (0,0)
// (top-left), so functions that predate it (DrawImage) are unaffected; HotSpot,
// CloneImage, and RotateImage set it. Single-frame only (multi-frame deferred).
struct CbImage {
    ALLEGRO_BITMAP* bmp;
    int32_t hotspot_x = 0;
    int32_t hotspot_y = 0;
};

// ─── Shared graphics state ─────────────────────────────────────────────

static ALLEGRO_DISPLAY*     display       = nullptr;
static ALLEGRO_EVENT_QUEUE* event_queue   = nullptr;
static ALLEGRO_COLOR        draw_color;
static ALLEGRO_COLOR        clear_color;
static int32_t              screen_w      = 0;
static int32_t              screen_h      = 0;

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
}

// ─── Screen management ─────────────────────────────────────────────────

extern "C" void cb_rt_screen(int32_t w, int32_t h) {
    ensure_init();

    if (display) {
        al_destroy_display(display);
    }
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

// Shared DrawScreen body. `clear_after` controls whether the backbuffer is
// cleared once events are drained (the `cls` flag of the 2-arg form).
static void do_draw_screen(bool clear_after) {
    if (!display) return;

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

extern "C" void cb_rt_line(double x1, double y1, double x2, double y2) {
    if (!current_target) return;
    al_draw_line((float)x1, (float)y1, (float)x2, (float)y2, draw_color, 1.0f);
}

// `d` is a diameter (CoolBasic convention); Allegro draws by radius.
extern "C" void cb_rt_circle(double x, double y, double d) {
    if (!current_target) return;
    float r = (float)d / 2.0f;
    al_draw_circle((float)x + r, (float)y + r, r, draw_color, 1.0f);
}

extern "C" void cb_rt_circle_fill(double x, double y, double d, int32_t fill) {
    if (!current_target) return;
    float r = (float)d / 2.0f;
    if (fill) {
        al_draw_filled_circle((float)x + r, (float)y + r, r, draw_color);
    } else {
        al_draw_circle((float)x + r, (float)y + r, r, draw_color, 1.0f);
    }
}

extern "C" void cb_rt_box(double x, double y, double w, double h) {
    if (!current_target) return;
    al_draw_rectangle((float)x, (float)y, (float)(x + w), (float)(y + h),
                      draw_color, 1.0f);
}

extern "C" void cb_rt_box_fill(double x, double y, double w, double h, int32_t fill) {
    if (!current_target) return;
    if (fill) {
        al_draw_filled_rectangle((float)x, (float)y, (float)(x + w), (float)(y + h),
                                 draw_color);
    } else {
        al_draw_rectangle((float)x, (float)y, (float)(x + w), (float)(y + h),
                          draw_color, 1.0f);
    }
}

extern "C" void cb_rt_dot(double x, double y) {
    if (!current_target) return;
    al_draw_pixel((float)x, (float)y, draw_color);
}

// Ellipse with top-left (x,y) and full diameters (w,h); matches our Circle's
// top-left convention. Allegro draws from the center with radii.
extern "C" void cb_rt_ellipse(double x, double y, double w, double h, int32_t fill) {
    if (!current_target) return;
    float rx = (float)w / 2.0f;
    float ry = (float)h / 2.0f;
    float cx = (float)x + rx;
    float cy = (float)y + ry;
    if (fill) {
        al_draw_filled_ellipse(cx, cy, rx, ry, draw_color);
    } else {
        al_draw_ellipse(cx, cy, rx, ry, draw_color, 1.0f);
    }
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
    al_draw_bitmap(img->bmp, (float)x - img->hotspot_x, (float)y - img->hotspot_y, 0);
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

// Copies an image and its hotspot.
extern "C" CbImage* cb_rt_clone_image(const CbImage* img) {
    if (!img || !img->bmp) return nullptr;
    ALLEGRO_BITMAP* b = al_clone_bitmap(img->bmp);
    if (!b) return nullptr;
    return new CbImage{b, img->hotspot_x, img->hotspot_y};
}

// Resizes (scales) an image to w×h. Resets the hotspot to (0,0).
extern "C" void cb_rt_resize_image(CbImage* img, int32_t w, int32_t h) {
    if (!img || !img->bmp || w <= 0 || h <= 0) return;
    int ow = al_get_bitmap_width(img->bmp);
    int oh = al_get_bitmap_height(img->bmp);

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
    al_set_target_bitmap(current_target ? current_target : dest);
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
    al_set_target_bitmap(current_target ? current_target : dest);
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
    float a = (float)(alpha / 100.0);
    if (a < 0.0f) a = 0.0f;
    if (a > 1.0f) a = 1.0f;
    al_draw_tinted_bitmap(img->bmp, al_map_rgba_f(1.0f, 1.0f, 1.0f, a),
                          (float)x - img->hotspot_x, (float)y - img->hotspot_y, 0);
}

// Draws the source sub-rectangle (sx,sy,sw,sh) of an image at (tx,ty).
extern "C" void cb_rt_draw_image_box(const CbImage* img, double sx, double sy,
                                     double sw, double sh, double tx, double ty) {
    if (!img || !img->bmp || !current_target) return;
    al_draw_bitmap_region(img->bmp, (float)sx, (float)sy, (float)sw, (float)sh,
                          (float)tx, (float)ty, 0);
}

// Per-image hotspot (scale/rotate/draw origin). x<0 || y<0 auto-centers.
// NOTE: cbEnchanted's HotSpot takes an integer id where 0/1 toggle a global
// default-hotspot; that overloading has no equivalent here because `Image` is
// an opaque handle, not an int id — so this is the per-image form only.
extern "C" void cb_rt_hotspot(CbImage* img, int32_t x, int32_t y) {
    if (!img || !img->bmp) return;
    if (x < 0 || y < 0) {
        img->hotspot_x = al_get_bitmap_width(img->bmp) / 2;
        img->hotspot_y = al_get_bitmap_height(img->bmp) / 2;
    } else {
        img->hotspot_x = x;
        img->hotspot_y = y;
    }
}

// AABB overlap helper (cbEnchanted's RectRectTest: box = left=x, right=x+w,
// top=y-h, bottom=y; epsilon keeps shared edges from counting).
static bool rect_overlap(double x1, double y1, double w1, double h1,
                         double x2, double y2, double w2, double h2) {
    constexpr double eps = 1e-5;
    double l1 = x1, r1 = x1 + w1, t1 = y1 - h1, b1 = y1;
    double l2 = x2, r2 = x2 + w2, t2 = y2 - h2, b2 = y2;
    if (b1 < t2 + eps) return false;
    if (t1 > b2 - eps) return false;
    if (r1 < l2 + eps) return false;
    if (l1 > r2 - eps) return false;
    return true;
}

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

// Whether a graphics mode is available. Best-effort: any positive resolution is
// reported as available (we create windowed displays on demand; `depth` is
// ignored since the backbuffer is always 32-bit).
extern "C" int32_t cb_rt_gfx_mode_exists(int32_t w, int32_t h, int32_t depth) {
    (void)depth;
    return (w > 0 && h > 0) ? 1 : 0;
}
