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

// FD-036 Phase 5: the game-loop update half (cbEnchanted updateObjects). Advances
// each object's animation, decrements ObjectLife (auto-deleting at 0), wipes each
// object's per-frame collision list, advances map tile animation, runs every
// registered collision check, then re-arms collision checking on all survivors.
// Driven by UpdateGame/DrawGame and the implicit DrawScreen pass (cb_gfx.cpp).
void cb_objects_update_all(void);

// FD-036 Phase 5: re-test every registered SetupCollision check (one update
// tick). Called by the game-loop update half (cb_objects_update_all, Phase 5c);
// declared here so that future caller lives in this TU but the symbol has
// external linkage (no unused-function warning while only collision lands).
void cb_run_collision_checks(void);

// FD-036 Phase 5: pick the first pickable object containing a world point. Used
// by CameraPick (cb_camera.cpp) after converting screen → world; sets the
// PickedObject slot (queryable via cb_rt_picked_object).
void cb_object_pick_at(double wx, double wy);

#ifdef __cplusplus
}
#endif

#endif  // CB_OBJECT_H
