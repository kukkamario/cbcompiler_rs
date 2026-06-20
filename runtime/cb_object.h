#ifndef CB_OBJECT_H
#define CB_OBJECT_H

// Internal object glue (FD-036 Phase 4). NOT catalog ABI and NOT a CB-visible
// function. cb_object.cpp owns the live object registry; cb_gfx.cpp calls
// cb::object::render_all() from DrawScreen to composite the whole object pass
// through the camera, on top of this frame's user draws and beneath the AddText
// overlay.
//
// render_all is one world-transform bracket over map background (layer 0) ->
// floor objects -> regular objects -> map foreground (layer 1). It calls into
// cb::map (render_layer / active) for the two tilemap layers.

namespace cb::object {

// Composite the whole object pass (see above). Called from DrawScreen.
void render_all();

// The game-loop update half (FD-036 Phase 5). Advances each object's animation,
// decrements ObjectLife (auto-deleting at 0), wipes each object's per-frame
// collision list, advances map tile animation, runs every registered collision
// check, then re-arms collision checking on all survivors. Driven by
// UpdateGame/DrawGame and the implicit DrawScreen pass (cb_gfx.cpp).
void update_all();

// Re-test every registered SetupCollision check (one update tick). Called by
// update_all.
void run_collision_checks();

// Pick the first pickable object containing a world point. Used by CameraPick
// (cb_camera.cpp) after converting screen -> world; sets the PickedObject slot
// (queryable via cb_rt_picked_object).
void pick_at(double wx, double wy);

}  // namespace cb::object

#endif  // CB_OBJECT_H
