#ifndef CB_MAP_H
#define CB_MAP_H

// Internal tilemap <-> graphics glue (FD-036 Phase 3/4). NOT catalog ABI and NOT
// a CB-visible function. cb_map.cpp owns the single active tilemap; the Phase-4
// object render orchestrator (cb_object.cpp's cb_objects_render_all) brackets the
// world transform and calls cb_map_render_layer() for the two drawn layers, so
// the map composites in cbEnchanted's drawObjects order (background layer 0
// before objects, foreground layer 1 after). This replaced Phase 3's standalone
// cb_map_render_active pass (retired).

#ifdef __cplusplus
extern "C" {
#endif

// Draws one map layer (slot 0 = background, 1 = foreground) under the world
// transform the caller has already set. Honors layerShowing/painted/visible and
// is a no-op when no map is active. The orchestrator owns the transform bracket.
void cb_map_render_layer(int slot);

// Whether a tilemap is currently active (1) or none is loaded (0). The render
// orchestrator's early-out checks this alongside the object draw chains.
int cb_map_active(void);

#ifdef __cplusplus
}
#endif

#endif  // CB_MAP_H
