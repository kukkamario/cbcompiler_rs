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

#include <string>

// ─── Opaque Image handle ──────────────────────────────────────────────
//
// The CB-visible `Image` type. Declared (never defined) in cb_runtime.h as
// `struct CbImage`; defined here. Always passed/returned by pointer.
struct CbImage {
    ALLEGRO_BITMAP* bmp;
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

extern "C" void cb_rt_drawscreen(void) {
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
            // NOTE (FD-013): this exit() is inconsistent with Batch 3's clean
            // IR `Halt` termination, but routing a window-close back to the
            // interpreter needs a runtime→interpreter trap channel that does
            // not exist yet. Left as-is; tracked in the FD.
            al_destroy_display(display);
            display = nullptr;
            exit(0);
        }
    }

    al_set_target_backbuffer(display);
    current_target = al_get_backbuffer(display);
    al_clear_to_color(clear_color);
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

// Reads a pixel from `img` as packed 32-bit ARGB. (Image must be Lock()ed for
// reliable reads on a video bitmap; memory bitmaps read directly.)
extern "C" int32_t cb_rt_get_pixel(const CbImage* img, int32_t x, int32_t y) {
    if (!img || !img->bmp) return 0;
    ALLEGRO_COLOR color = al_get_pixel(img->bmp, x, y);
    unsigned char r, g, b, a;
    al_unmap_rgba(color, &r, &g, &b, &a);
    return ((int32_t)a << 24) | ((int32_t)r << 16) | ((int32_t)g << 8) | (int32_t)b;
}

// ─── Image handles ─────────────────────────────────────────────────────

extern "C" CbImage* cb_rt_make_image(int32_t w, int32_t h) {
    ensure_init();
    // Without a display, video bitmaps cannot be created — fall back to a
    // memory bitmap so images (and the headless pixel round-trip test) work
    // before any Screen() call. With a display, keep the default (video).
    int prev_flags = al_get_new_bitmap_flags();
    if (!al_get_current_display()) {
        al_set_new_bitmap_flags(ALLEGRO_MEMORY_BITMAP);
    }
    ALLEGRO_BITMAP* bmp = al_create_bitmap(w, h);
    al_set_new_bitmap_flags(prev_flags);
    if (!bmp) return nullptr;

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

    CbImage* img = new CbImage{bmp};
    return img;
}

extern "C" void cb_rt_draw_image(const CbImage* img, double x, double y) {
    if (!img || !img->bmp || !current_target) return;
    al_draw_bitmap(img->bmp, (float)x, (float)y, 0);
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

// ─── Screen queries ────────────────────────────────────────────────────

extern "C" int32_t cb_rt_screen_width(void) {
    return screen_w;
}

extern "C" int32_t cb_rt_screen_height(void) {
    return screen_h;
}
