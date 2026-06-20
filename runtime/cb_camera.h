#ifndef CB_CAMERA_H
#define CB_CAMERA_H

// Internal camera <-> graphics glue (FD-036 Phase 2). NOT catalog ABI and NOT
// registered as CB-visible functions. These declarations bridge cb_camera.cpp
// (owns the camera transform + DrawToWorld flags) and cb_gfx.cpp (owns the
// display + render target). They reference ALLEGRO_TRANSFORM, so they live here
// rather than in the Allegro-free cb_runtime.h (mirrors cb_input.h's
// cb_gfx_display glue).

#include <allegro5/allegro.h>
#include <cstdint>

#ifdef __cplusplus
extern "C" {
#endif

// cb_camera.cpp -> cb_gfx.cpp: the transform for DrawToWorld *user* draws, built
// from the live camera state. Feed raw world coordinates; the result lands in
// screen space (the Y-flip is folded in). The returned pointer aliases a static
// rebuilt on each call — valid until the next call (safe on the single-threaded
// VM), so use it immediately.
const ALLEGRO_TRANSFORM* cb_camera_render_transform(void);

// cb_camera.cpp -> cb_map.cpp: the *plain* world transform (no folded Y-flip),
// matching cbEnchanted's RenderTarget::useWorldCoords. Callers that draw world
// bitmaps (the tilemap) apply this and flip each anchor's Y themselves (as
// cbEnchanted's convertCoords does), so bitmaps stay upright. Same static-alias
// lifetime rule as cb_camera_render_transform — use it immediately.
const ALLEGRO_TRANSFORM* cb_camera_world_transform(void);

// cb_camera.cpp -> cb_gfx.cpp: the three DrawToWorld flags (nonzero = draw that
// category in world space). cb_gfx.cpp consults these per draw command.
int cb_camera_draw_cmd_to_world(void);
int cb_camera_image_to_world(void);
int cb_camera_text_to_world(void);

// cb_camera.cpp -> cb_object.cpp (FD-036 Phase 4): the current zoom, and the
// world-space size of the visible draw area (cbEnchanted getDrawAreaWidth/Height
// — rotated screen extent / zoom). Floor objects tile to fill this area.
double cb_camera_zoom(void);
void cb_camera_draw_area(double* w, double* h);

// cb_gfx.cpp -> cb_camera.cpp: the logical design resolution (cbEnchanted's
// defaultWidth/Height; 400x300 until a Screen command sets it). The camera
// centers its world transform on (w/2, h/2).
void cb_gfx_design_size(int32_t* w, int32_t* h);

// cb_gfx.cpp -> cb_camera.cpp (FD-036 Phase 5): the PHYSICAL display size
// (cbEnchanted's screenWidth/Height — the window, distinct from the design size
// above). CameraFollow's style-2 deadzone is measured against this. 0×0 before a
// Screen command opens a display.
void cb_gfx_window_size(int32_t* w, int32_t* h);

// cb_camera.cpp -> cb_object.cpp / cb_gfx.cpp (FD-036 Phase 5): convert a screen
// coordinate to world space through the live camera (the inverse world transform
// + Y-flip, matching cbEnchanted's screenCoordToWorld). Used by ScreenPositionObject
// and the picking funnel.
void cb_camera_screen_to_world(double* x, double* y);

// cb_camera.cpp -> cb_gfx.cpp (FD-036 Phase 5): if CameraFollow is active, step
// the camera toward its target once (called per frame from DrawScreen, Phase 5c).
// A no-op when not following.
void cb_camera_update_follow(void);

#ifdef __cplusplus
}
#endif

#endif  // CB_CAMERA_H
