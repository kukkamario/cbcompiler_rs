#ifndef CB_MAP_H
#define CB_MAP_H

// Internal tilemap <-> graphics glue (FD-036 Phase 3). NOT catalog ABI and NOT
// a CB-visible function. cb_map.cpp owns the single active tilemap; cb_gfx.cpp
// calls cb_map_render_active() from DrawScreen to composite the map (background
// layer 0 then foreground layer 1) through the camera, on top of the user's
// frame and beneath the AddText overlay. Mirrors cb_camera.h's gfx glue.
//
// In cbEnchanted the map draws inside the object draw order (drawObjects); with
// no object subsystem yet (FD-036 Phase 4/5), this standalone pass stands in.
// Phase 5 relocates the call into the full drawObjects order.

#ifdef __cplusplus
extern "C" {
#endif

void cb_map_render_active(void);

#ifdef __cplusplus
}
#endif

#endif  // CB_MAP_H
