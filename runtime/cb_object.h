#ifndef CB_OBJECT_H
#define CB_OBJECT_H

// Internal object <-> graphics glue (FD-036 Phase 4). NOT catalog ABI and NOT a
// CB-visible function. cb_object.cpp owns the live object registry; cb_gfx.cpp
// calls cb_objects_render_all() from DrawScreen to composite the whole object
// pass through the camera, on top of this frame's user draws and beneath the
// AddText overlay. Mirrors cb_camera.h / the retired cb_map_render_active glue.
//
// This is the cbEnchanted drawObjects analogue: one world-transform bracket over
// map background (layer 0) -> floor objects -> regular objects -> map foreground
// (layer 1). It calls into cb_map (cb_map_render_layer / cb_map_active) for the
// two tilemap layers, replacing Phase 3's standalone cb_map_render_active pass.

#ifdef __cplusplus
extern "C" {
#endif

void cb_objects_render_all(void);

#ifdef __cplusplus
}
#endif

#endif  // CB_OBJECT_H
