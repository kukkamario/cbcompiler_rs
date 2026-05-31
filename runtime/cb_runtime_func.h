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

/* System / Time (cb_system.cpp). `End` is not a runtime function — it is a
   language statement lowered to an IR `Halt` terminator. `MakeError` only
   writes its message to stderr here; the lowerer appends `Halt(1)` to
   terminate. */
int32_t cb_rt_timer(void);
void cb_rt_wait(int32_t ms);
void cb_rt_make_error(const CbString* msg);

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
double cb_rt_rnd_range(double min, double max);
int32_t cb_rt_rand_max(int32_t max);
int32_t cb_rt_rand_range(int32_t min, int32_t max);
void cb_rt_randomize(int32_t seed);

/* Graphics & images (cb_gfx.cpp, FD-013 Batch 4). CB `Float` args arrive as
   `double`, `Int` as `int32_t`. `Image` is the opaque CbImage* handle. Many
   functions are overloaded by arity/type — sema resolves them; each maps to a
   distinct C symbol below sharing one CB name in catalog.cpp. */
void    cb_rt_screen(int32_t w, int32_t h);
void    cb_rt_screen_mode(int32_t w, int32_t h, int32_t mode);
void    cb_rt_drawscreen(void);
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
void    cb_rt_line(double x1, double y1, double x2, double y2);
void    cb_rt_circle(double x, double y, double d);
void    cb_rt_circle_fill(double x, double y, double d, int32_t fill);
void    cb_rt_box(double x, double y, double w, double h);
void    cb_rt_box_fill(double x, double y, double w, double h, int32_t fill);
void    cb_rt_dot(double x, double y);
void    cb_rt_put_pixel(int32_t x, int32_t y, int32_t r, int32_t g, int32_t b);
void    cb_rt_put_pixel_a(int32_t x, int32_t y, int32_t r, int32_t g, int32_t b, int32_t a);
void    cb_rt_put_pixel_argb(int32_t x, int32_t y, int32_t argb);
int32_t cb_rt_get_pixel(const CbImage* img, int32_t x, int32_t y);
CbImage* cb_rt_make_image(int32_t w, int32_t h);
CbImage* cb_rt_load_image(const CbString* path);
void    cb_rt_draw_image(const CbImage* img, double x, double y);
void    cb_rt_mask_image(CbImage* img, int32_t r, int32_t g, int32_t b);
void    cb_rt_mask_image_a(CbImage* img, int32_t r, int32_t g, int32_t b, int32_t a);
void    cb_rt_draw_to_image(CbImage* img);
int32_t cb_rt_image_width(const CbImage* img);
int32_t cb_rt_image_height(const CbImage* img);
void    cb_rt_delete_image(CbImage* img);
int32_t cb_rt_screen_width(void);
int32_t cb_rt_screen_height(void);

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
