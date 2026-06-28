#ifndef CB_INPUT_H
#define CB_INPUT_H

// Internal input glue. NOT part of the catalog ABI and NOT
// registered as CB-visible runtime functions. cb_gfx.cpp owns the Allegro event
// queue and calls these from DrawScreen to advance the input state machine each
// frame; cb_input.cpp implements them. They reference ALLEGRO_EVENT, so they
// live here rather than in the Allegro-free cb_runtime.h.

#include <allegro5/allegro.h>

namespace cb::input {

// Begin a new input frame: clear the per-key/button "changed" bits and zero the
// mouse movement deltas. Call once per frame before draining the event queue.
void frame_begin();

// Feed one queued Allegro event into the input state machine (keyboard
// down/up, mouse buttons, mouse axes). Ignores event types it doesn't track.
void handle_event(const ALLEGRO_EVENT* ev);

}  // namespace cb::input

// (The cb_gfx.cpp display/event-queue accessors the blocking/cursor input
// functions use live in cb_gfx.h as cb::gfx::display / cb::gfx::event_queue.)

#endif /* CB_INPUT_H */
