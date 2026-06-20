#ifndef CB_CAMERA_H
#define CB_CAMERA_H

// Internal camera glue (FD-036 Phase 2). NOT catalog ABI and NOT registered as
// CB-visible functions: these bridge cb_camera.cpp (owns the camera transform +
// DrawToWorld flags) to the other Allegro-linked subsystems (gfx, object, map).
// They traffic in ALLEGRO_TRANSFORM, so this header pulls in Allegro and is
// never included by the Allegro-free core. (The reverse-direction gfx accessors
// the camera reads live in cb_gfx.h under cb::gfx.)

#include <allegro5/allegro.h>
#include <cstdint>

namespace cb::camera {

// The transform for DrawToWorld *user* draws, built from the live camera state.
// Feed raw world coordinates; the result lands in screen space (the Y-flip is
// folded in). The returned pointer aliases a static rebuilt on each call — valid
// until the next call (safe on the single-threaded VM), so use it immediately.
const ALLEGRO_TRANSFORM* render_transform();

// The *plain* world transform (no folded Y-flip). Callers that draw world
// bitmaps (the tilemap) apply this and flip each anchor's Y themselves so the
// bitmaps stay upright. Same static-alias lifetime rule as render_transform —
// use it immediately.
const ALLEGRO_TRANSFORM* world_transform();

// The three DrawToWorld flags (nonzero = draw that category in world space).
// cb_gfx.cpp consults these per draw command.
int draw_cmd_to_world();
int image_to_world();
int text_to_world();

// The current zoom, and the world-space size of the visible draw area (the
// rotated window extent / zoom). Floor objects tile to fill this area.
double zoom();
void draw_area(double* w, double* h);

// Convert a screen coordinate to world space through the live camera (the
// inverse world transform + Y-flip). Used by ScreenPositionObject and the
// picking funnel.
void screen_to_world(double* x, double* y);

// If CameraFollow is active, step the camera toward its target once (called per
// frame from DrawScreen). A no-op when not following.
void update_follow();

}  // namespace cb::camera

#endif  // CB_CAMERA_H
