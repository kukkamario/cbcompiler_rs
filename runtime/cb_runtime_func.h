#ifndef CB_RUNTIME_FUNC_H
#define CB_RUNTIME_FUNC_H

/* Runtime FUNCTIONALITY prototypes (FD-016).
 *
 * The feature subsystems built ON TOP of the core ABI: a handful of System
 * builtins, the String library, System/Time, Math, Graphics, and Input. Only
 * Graphics and Input pull in Allegro; the rest depend on nothing but the core
 * string primitives.
 *
 * Plugins do NOT include this header — it is internal to the bundled runtime.
 * See cb_runtime_core.h for the plugin SDK surface. */

#include "cb_runtime_core.h"

/* ─── Concrete opaque handle types ───────────────────────────────────────
   These follow the custom-type convention documented in cb_runtime_core.h
   but are functionality, not core ABI. */

/* Test handle — the only runtime function pair that exercises a
   runtime-defined opaque type end to end. */
typedef struct CbTestHandle CbTestHandle;

/* Image handle — the CB-visible `Image` opaque type (FD-013 Batch 4). Wraps an
   Allegro bitmap; defined in cb_gfx.cpp. Created by MakeImage/LoadImage, freed
   by DeleteImage. */
typedef struct CbImage CbImage;

/* Font handle — the CB-visible `Font` opaque type (FD-018). Wraps an Allegro
   font; defined in cb_gfx.cpp. Created by LoadFont, freed by DeleteFont. */
typedef struct CbFont CbFont;

#ifdef __cplusplus
extern "C" {
#endif

/* System */
void cb_rt_print(const CbString* text);
int32_t cb_rt_abs_int(int32_t x);
double cb_rt_abs_float(double x);

/* String library (cb_string.cpp). Character indices/counts are 1-based and
   measured in Unicode codepoints; out-of-range arguments clamp (never abort).
   String-returning functions yield an owning CbString* (refcount 1, or the
   immortal empty sentinel). String Len is handled by a sema intrinsic, not
   here. */
CbString* cb_rt_str_upper(const CbString* s);
CbString* cb_rt_str_lower(const CbString* s);
CbString* cb_rt_str_trim(const CbString* s);
CbString* cb_rt_str_left(const CbString* s, int32_t n);
CbString* cb_rt_str_right(const CbString* s, int32_t n);
CbString* cb_rt_str_remove(const CbString* s, int32_t pos, int32_t count);
int32_t cb_rt_str_instr(const CbString* s, const CbString* find);
int32_t cb_rt_str_instr_from(const CbString* s, const CbString* find, int32_t start);
CbString* cb_rt_chr(int32_t code);
CbString* cb_rt_hex(int32_t value);
/* FD-017 completeness pass. Same codepoint/1-based/clamp conventions. */
CbString* cb_rt_str_mid(const CbString* s, int32_t pos, int32_t len);
CbString* cb_rt_str_replace(const CbString* s, const CbString* find, const CbString* repl);
CbString* cb_rt_str_lset(const CbString* s, int32_t len);
CbString* cb_rt_str_rset(const CbString* s, int32_t len);
int32_t   cb_rt_str_asc(const CbString* s);
CbString* cb_rt_bin(int32_t value);
CbString* cb_rt_str_repeat(const CbString* s, int32_t count);
CbString* cb_rt_str_flip(const CbString* s);
CbString* cb_rt_str_insert(const CbString* s, int32_t pos, const CbString* txt);
CbString* cb_rt_str_move(const CbString* s, int32_t pos, int32_t len, int32_t offset);
int32_t   cb_rt_count_words(const CbString* s, const CbString* sep);
CbString* cb_rt_get_word(const CbString* s, int32_t n, const CbString* sep);

/* System / Time (cb_system.cpp). `End` is not a runtime function — it is a
   language statement lowered to an IR `Halt` terminator. `MakeError` only
   writes its message to stderr here; the lowerer appends `Halt(1)` to
   terminate. */
int32_t cb_rt_timer(void);
void cb_rt_wait(int32_t ms);
void cb_rt_make_error(const CbString* msg);
/* FD-017. Date "D Mon YYYY"; Time "HH:MM:SS"; CommandLine/GetEXEName read the
   running process (for the interpreter, that is the `cb` executable). */
CbString* cb_rt_date(void);
CbString* cb_rt_time(void);
CbString* cb_rt_command_line(void);
CbString* cb_rt_get_exe_name(void);

/* Math (implemented in cb_math.cpp). Trig is in DEGREES. */
double cb_rt_sin(double deg);
double cb_rt_cos(double deg);
double cb_rt_tan(double deg);
double cb_rt_asin(double x);
double cb_rt_acos(double x);
double cb_rt_atan(double x);
double cb_rt_sqrt(double x);
double cb_rt_log(double x);
double cb_rt_log10(double x);
int32_t cb_rt_round_up(double x);
int32_t cb_rt_round_down(double x);
int32_t cb_rt_max_int(int32_t a, int32_t b);
int32_t cb_rt_min_int(int32_t a, int32_t b);
double cb_rt_max_float(double a, double b);
double cb_rt_min_float(double a, double b);
double cb_rt_distance(double x1, double y1, double x2, double y2);
double cb_rt_get_angle(double x1, double y1, double x2, double y2);
double cb_rt_wrap_angle(double a);
double cb_rt_rnd_max(double max);
double cb_rt_rnd_range(double low, double high);
int32_t cb_rt_rand_max(int32_t max);
int32_t cb_rt_rand_range(int32_t low, int32_t high);
void cb_rt_randomize(int32_t seed);
/* FD-017. CurveValue/CurveAngle ease current->target; BoxOverlap is AABB. */
double cb_rt_curve_value(double target, double current, double smoothness);
double cb_rt_curve_angle(double target, double current, double smoothness);
int32_t cb_rt_box_overlap(double x1, double y1, double w1, double h1,
                          double x2, double y2, double w2, double h2);

/* Graphics & images (cb_gfx.cpp, FD-013 Batch 4). CB `Float` args arrive as
   `double`, `Int` as `int32_t`. `Image` is the opaque CbImage* handle. Many
   functions are overloaded by arity/type — sema resolves them; each maps to a
   distinct C symbol below sharing one CB name in catalog.cpp. */
void    cb_rt_screen(int32_t w, int32_t h);
void    cb_rt_screen_mode(int32_t w, int32_t h, int32_t mode);
void    cb_rt_screen_depth_mode(int32_t w, int32_t h, int32_t depth, int32_t mode);
int32_t cb_rt_screen_buffer_id(void);
void    cb_rt_drawscreen(void);
void    cb_rt_drawscreen_args(int32_t cls, int32_t vsync);
void    cb_rt_cls(void);
void    cb_rt_cls_color(int32_t r, int32_t g, int32_t b);
void    cb_rt_cls_color_a(int32_t r, int32_t g, int32_t b, int32_t a);
void    cb_rt_draw_to_screen(void);
int32_t cb_rt_fps(void);
void    cb_rt_lock(void);
void    cb_rt_lock_state(int32_t state);
void    cb_rt_lock_image(CbImage* img);
void    cb_rt_lock_image_state(CbImage* img, int32_t state);
void    cb_rt_unlock(void);
void    cb_rt_unlock_image(CbImage* img);
void    cb_rt_color(int32_t r, int32_t g, int32_t b);
void    cb_rt_color_a(int32_t r, int32_t g, int32_t b, int32_t a);
int32_t cb_rt_get_rgb(int32_t channel);
void    cb_rt_pick_color(int32_t x, int32_t y);
void    cb_rt_smooth_2d(int32_t enabled);
void    cb_rt_screen_gamma(int32_t r, int32_t g, int32_t b);
void    cb_rt_screenshot(const CbString* path);
void    cb_rt_line(double x1, double y1, double x2, double y2);
void    cb_rt_ellipse(double x, double y, double w, double h, int32_t fill);
void    cb_rt_circle(double x, double y, double d);
void    cb_rt_circle_fill(double x, double y, double d, int32_t fill);
void    cb_rt_box(double x, double y, double w, double h);
void    cb_rt_box_fill(double x, double y, double w, double h, int32_t fill);
void    cb_rt_dot(double x, double y);
void    cb_rt_put_pixel(int32_t x, int32_t y, int32_t r, int32_t g, int32_t b);
void    cb_rt_put_pixel_a(int32_t x, int32_t y, int32_t r, int32_t g, int32_t b, int32_t a);
void    cb_rt_put_pixel_argb(int32_t x, int32_t y, int32_t argb);
int32_t cb_rt_get_pixel(const CbImage* img, int32_t x, int32_t y);
int32_t cb_rt_get_pixel2(int32_t x, int32_t y);
void    cb_rt_copy_box(double srcX, double srcY, double w, double h,
                       double destX, double destY, int32_t srcBuf, int32_t destBuf);
CbImage* cb_rt_make_image(int32_t w, int32_t h);
CbImage* cb_rt_load_image(const CbString* path);
void    cb_rt_draw_image(const CbImage* img, double x, double y);
void    cb_rt_mask_image(CbImage* img, int32_t r, int32_t g, int32_t b);
void    cb_rt_mask_image_a(CbImage* img, int32_t r, int32_t g, int32_t b, int32_t a);
void    cb_rt_draw_to_image(CbImage* img);
int32_t cb_rt_image_width(const CbImage* img);
int32_t cb_rt_image_height(const CbImage* img);
void    cb_rt_delete_image(CbImage* img);
/* FD-017 image additions (single-frame; multi-frame deferred). */
void     cb_rt_default_mask(int32_t enabled, int32_t r, int32_t g, int32_t b);
CbImage* cb_rt_clone_image(const CbImage* img);
void     cb_rt_resize_image(CbImage* img, int32_t w, int32_t h);
void     cb_rt_rotate_image(CbImage* img, double angle);
void     cb_rt_pick_image_color(const CbImage* img, int32_t x, int32_t y);
void     cb_rt_save_image(const CbImage* img, const CbString* path, int32_t frame);
void     cb_rt_draw_ghost_image(const CbImage* img, double x, double y, double alpha);
void     cb_rt_draw_image_box(const CbImage* img, double sx, double sy,
                              double sw, double sh, double tx, double ty);
void     cb_rt_hotspot(CbImage* img, int32_t x, int32_t y);
int32_t  cb_rt_images_overlap(const CbImage* a, double x1, double y1,
                              const CbImage* b, double x2, double y2);
int32_t  cb_rt_images_collide(const CbImage* a, double x1, double y1, int32_t frame1,
                              const CbImage* b, double x2, double y2, int32_t frame2);
/* FD-036 multi-frame sprite sheets. */
CbImage* cb_rt_load_anim_image(const CbString* path, int32_t frame_w, int32_t frame_h,
                               int32_t start_frame, int32_t anim_length);
CbImage* cb_rt_make_image_frames(int32_t w, int32_t h, int32_t frame_count);
void     cb_rt_draw_image_frame(const CbImage* img, double x, double y, int32_t frame);
void     cb_rt_draw_image_frame_mask(const CbImage* img, double x, double y,
                                     int32_t frame, int32_t use_mask);
void     cb_rt_draw_ghost_image_frame(const CbImage* img, double x, double y,
                                      int32_t frame, double alpha);
void     cb_rt_draw_image_box_frame(const CbImage* img, double sx, double sy,
                                    double sw, double sh, double tx, double ty,
                                    int32_t frame);
void     cb_rt_draw_image_box_frame_mask(const CbImage* img, double sx, double sy,
                                         double sw, double sh, double tx, double ty,
                                         int32_t frame, int32_t use_mask);
int32_t cb_rt_screen_width(void);
int32_t cb_rt_screen_height(void);
int32_t cb_rt_screen_depth(void);
int32_t cb_rt_gfx_mode_exists(int32_t w, int32_t h, int32_t depth);

/* Text & fonts (cb_gfx.cpp, FD-018). Text draws in the current draw color onto
   the active render target; `Font` is the opaque CbFont* handle. Locate/AddText/
   ClearText manage a persistent on-screen text queue re-rendered each DrawScreen.
   LoadFont resolves a system family name or a file path (name containing '.');
   Smooth2D toggles antialiased vs monochrome glyphs. With no font loaded the
   metric queries return 0 (a default font is loaded lazily, so this is rare). */
void     cb_rt_text(double x, double y, const CbString* s);
void     cb_rt_center_text(int32_t x, int32_t y, const CbString* s, int32_t style);
void     cb_rt_vertical_text(int32_t x, int32_t y, const CbString* s);
void     cb_rt_locate(int32_t x, int32_t y);
void     cb_rt_add_text(const CbString* s);
void     cb_rt_clear_text(void);
CbFont*  cb_rt_load_font(const CbString* name, int32_t size, int32_t bold,
                         int32_t italic, int32_t underline);
void     cb_rt_set_font(CbFont* font);
void     cb_rt_delete_font(CbFont* font);
int32_t  cb_rt_text_width(const CbString* s);
int32_t  cb_rt_text_height(const CbString* s);

/* Input (cb_input.cpp, FD-013 Batch 5). Keyboard scancodes use the legacy CB
   DirectInput-style numbering (1=Esc, 16=Q, 30=A, 200=Up, …). Edge queries
   (KeyHit/KeyUp, MouseHit/MouseUp) and movement deltas are relative to the
   frame boundary, which DrawScreen advances; with no display open every query
   returns 0. Mouse buttons: 1=left, 2=right, 3=middle. EscapeKey is a pure
   query (no auto-exit). The cb_input_frame_begin/cb_input_handle_event hooks
   that drive this state are internal (see cb_input.h), not catalog functions. */
int32_t cb_rt_key_down(int32_t scancode);
int32_t cb_rt_key_up(int32_t scancode);
int32_t cb_rt_key_hit(int32_t scancode);
int32_t cb_rt_escape_key(void);
int32_t cb_rt_mouse_x(void);
int32_t cb_rt_mouse_y(void);
int32_t cb_rt_mouse_down(int32_t button);
int32_t cb_rt_mouse_hit(int32_t button);
int32_t cb_rt_mouse_up(int32_t button);
int32_t cb_rt_mouse_z(void);
int32_t cb_rt_mouse_move_x(void);
int32_t cb_rt_mouse_move_y(void);
int32_t cb_rt_mouse_move_z(void);
/* FD-017 input additions. WaitKey/WaitMouse block on the window event queue;
   with no window open they return 0 immediately (headless-safe). */
int32_t cb_rt_get_key(void);
int32_t cb_rt_left_key(void);
int32_t cb_rt_right_key(void);
int32_t cb_rt_up_key(void);
int32_t cb_rt_down_key(void);
void    cb_rt_clear_keys(void);
int32_t cb_rt_wait_key(void);
int32_t cb_rt_get_mouse(void);
int32_t cb_rt_wait_mouse(void);
void    cb_rt_position_mouse(int32_t x, int32_t y);
void    cb_rt_show_mouse(int32_t mode);
void    cb_rt_clear_mouse(void);

/* Test handle functions for opaque type testing */
CbTestHandle* cb_rt_create_test_handle(void);
int32_t cb_rt_use_test_handle(const CbTestHandle* handle);

/* Trap-channel test functions (FD-015, test-only): ask the host to exit /
   raise an error via cb_host(), exercising the channel end to end. */
void cb_rt_test_request_exit(int32_t code);
void cb_rt_test_raise_error(const CbString* msg);

#ifdef __cplusplus
}
#endif

#endif /* CB_RUNTIME_FUNC_H */
