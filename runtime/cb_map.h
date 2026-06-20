#ifndef CB_MAP_H
#define CB_MAP_H

// Internal tilemap glue (FD-036 Phase 3/4). NOT catalog ABI and NOT a CB-visible
// function. cb_map.cpp owns the single active tilemap; the object render
// orchestrator in cb_object.cpp brackets the world transform and calls
// cb::map::render_layer() for the two drawn layers, so the map composites in the
// object draw order (background layer 0 before objects, foreground layer 1 after).

// CbMapData is the Allegro-free grid defined in cb_map_data.h; only a pointer to
// it crosses this boundary, so a forward declaration suffices here.
struct CbMapData;

namespace cb::map {

// Draws one map layer (slot 0 = background, 1 = foreground) under the world
// transform the caller has already set. Honors layerShowing/painted/visible and
// is a no-op when no map is active. The orchestrator owns the transform bracket.
void render_layer(int slot);

// Whether a tilemap is currently active (1) or none is loaded (0). The render
// orchestrator's early-out checks this alongside the object draw chains.
int active();

// The active map's parsed grid (or null when none), so the object subsystem can
// run map collision (type 4) and ObjectSight against the collision layer (layer
// 2). The pointer is owned by cb_map.cpp and invalidated by the next
// LoadMap/MakeMap.
const CbMapData* active_data();

// Advance every animated tile by one tile-animation tick. A no-op when no map is
// active or no tile is animated. Called from the object update pass.
void tick_animation();

}  // namespace cb::map

#endif  // CB_MAP_H
