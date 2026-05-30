#ifndef CB_INPUT_H
#define CB_INPUT_H

// Internal input <-> graphics glue (FD-013 Batch 5). NOT part of the catalog
// ABI and NOT registered as CB-visible runtime functions. cb_gfx.cpp owns the
// Allegro event queue and calls these from DrawScreen to advance the input
// state machine each frame; cb_input.cpp implements them. They reference
// ALLEGRO_EVENT, so they live here rather than in the Allegro-free cb_runtime.h.

#include <allegro5/allegro.h>

#ifdef __cplusplus
extern "C" {
#endif

// Begin a new input frame: clear the per-key/button "changed" bits and zero the
// mouse movement deltas. Call once per frame before draining the event queue.
void cb_input_frame_begin(void);

// Feed one queued Allegro event into the input state machine (keyboard
// down/up, mouse buttons, mouse axes). Ignores event types it doesn't track.
void cb_input_handle_event(const ALLEGRO_EVENT* ev);

#ifdef __cplusplus
}
#endif

#endif /* CB_INPUT_H */
