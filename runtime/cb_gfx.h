#ifndef CB_GFX_H
#define CB_GFX_H

// Internal graphics glue (FD-037). NOT part of the catalog ABI and NOT
// registered as CB-visible runtime functions: these bridge cb_gfx.cpp — which
// owns the display, event queue, render target, and `Image` bitmaps — to the
// other Allegro-linked subsystems (input, camera, map, object) and the native
// tests. They traffic in Allegro types, so this header pulls in Allegro and is
// included only by the Allegro-linked TUs, never by the Allegro-free core.
// Previously each consumer hand-declared these; this is the single shared
// declaration.

#include "cb_runtime_func.h"  // CbImage

#include <allegro5/allegro.h>
#include <cstdint>

namespace cb::gfx {

// New-bitmap state masking depends on: an alpha channel (ANY_32_WITH_ALPHA)
// carrying straight (non-premultiplied) alpha, so a colour key written as
// alpha=0 shows through. Process-global, so it must be set before any bitmap is
// created — the object/map loaders that self-init Allegro call it too.
void apply_bitmap_defaults();

// The source-over alpha blender (color = src·srcA + dst·(1-srcA)). Per-target
// state, re-applied wherever a render target is established, so masked (alpha=0)
// pixels are skipped instead of overwriting as opaque. cb_object.cpp restores it
// after a temporary copy blender (MirrorObject).
void apply_alpha_blender();

// The display and its event queue. Both null when no window is open, so the
// blocking/cursor input functions degrade to a safe no-op headlessly.
ALLEGRO_DISPLAY* display();
ALLEGRO_EVENT_QUEUE* event_queue();

// The logical design resolution the camera centers its world transform on
// (400×300 until a Screen command sets it).
void design_size(int32_t* w, int32_t* h);

// The physical display size (the window), used by CameraFollow's deadzone. 0×0
// when no window is open.
void window_size(int32_t* w, int32_t* h);

// The live bitmap behind an `Image` handle, used by PaintObject(Object/Map,
// Image). Null when the image or its bitmap is null.
ALLEGRO_BITMAP* image_bitmap(const CbImage* img);

// The image's pristine (pre-mask) bitmap, so PaintObject keys from the unmasked
// original rather than an already-keyed copy. Falls back to the live bitmap for
// a never-masked image; null when the image or its bitmap is null.
ALLEGRO_BITMAP* image_pristine(const CbImage* img);

}  // namespace cb::gfx

#endif  // CB_GFX_H
