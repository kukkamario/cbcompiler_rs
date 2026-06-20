// FD-036: masking / alpha-blend pipeline tests. These pin the fix for masking
// having no visible effect: (1) the runtime now uses a source-over alpha blender
// + alpha-capable bitmap format, so a keyed (alpha=0) pixel is transparent at
// draw time instead of overwriting opaque; (2) MaskImage keeps a pristine copy
// and re-keys from it, so masking a second colour works (the old single-bitmap
// code made the second MaskImage a silent no-op); (3) DrawImage's useMask=0 draws
// the un-keyed original.
//
// Runs fully headless: cb_rt_make_image falls back to memory bitmaps and bootstraps
// Allegro via the runtime's ensure_init (which also installs the alpha blender), so
// no display is needed. Drawing onto an image target uses the identity transform
// (gfx_begin_world only engages for the backbuffer), so pixel positions are exact.

// cb_gfx.h provides the cb::gfx::image_bitmap test hook and (via
// cb_runtime_func.h) the cb_rt_* image entry points + the opaque CbImage handle.
#include "cb_gfx.h"

#include <allegro5/allegro.h>
#include <gtest/gtest.h>

#include <cstdint>

namespace {

struct Rgba {
    unsigned char r, g, b, a;
};

Rgba get_rgba(ALLEGRO_BITMAP* bmp, int x, int y) {
    Rgba out{};
    al_unmap_rgba(al_get_pixel(bmp, x, y), &out.r, &out.g, &out.b, &out.a);
    return out;
}

// Paints a 2x1 image: pixel (0,0) = colour A, pixel (1,0) = colour B, both opaque.
// Returns the handle; caller deletes it.
CbImage* make_two_pixel(unsigned char ar, unsigned char ag, unsigned char ab,
                        unsigned char br, unsigned char bg, unsigned char bb) {
    CbImage* img = cb_rt_make_image(2, 1);
    ALLEGRO_BITMAP* bmp = cb::gfx::image_bitmap(img);
    ALLEGRO_BITMAP* prev = al_get_target_bitmap();
    al_set_target_bitmap(bmp);
    al_put_pixel(0, 0, al_map_rgb(ar, ag, ab));
    al_put_pixel(1, 0, al_map_rgb(br, bg, bb));
    if (prev) al_set_target_bitmap(prev);
    return img;
}

}  // namespace

// The core symptom: a keyed pixel must be transparent at draw time so the
// background shows through, while the kept pixel paints its own colour. Before the
// fix the ONE/ZERO blender copied source verbatim and the keyed pixel painted
// opaque black instead of the background.
TEST(Masking, AlphaRespectedAtDraw) {
    // Sprite: (0,0) blue (kept), (1,0) red (to be keyed transparent).
    CbImage* sprite = make_two_pixel(0, 0, 255, 255, 0, 0);
    cb_rt_mask_image(sprite, 255, 0, 0);

    // Background: 2x1 cleared to green.
    CbImage* bg = cb_rt_make_image(2, 1);
    ALLEGRO_BITMAP* bb = cb::gfx::image_bitmap(bg);
    al_set_target_bitmap(bb);
    al_clear_to_color(al_map_rgb(0, 255, 0));

    // Draw the masked sprite onto the background through the runtime.
    cb_rt_draw_to_image(bg);
    cb_rt_draw_image_frame(sprite, 0, 0, 0);

    Rgba kept = get_rgba(bb, 0, 0);    // sprite's blue
    Rgba keyed = get_rgba(bb, 1, 0);   // keyed -> background green shows through
    EXPECT_EQ(kept.r, 0);   EXPECT_EQ(kept.g, 0);   EXPECT_EQ(kept.b, 255);
    EXPECT_EQ(keyed.r, 0);  EXPECT_EQ(keyed.g, 255); EXPECT_EQ(keyed.b, 0);

    cb_rt_delete_image(sprite);
    cb_rt_delete_image(bg);
}

// MaskImage re-keys from a pristine copy: masking a second colour restores the
// first colour to opaque and keys the new one. The old single-bitmap code could
// not restore previously-keyed pixels, so this is the regression guard for the
// "MaskObject/MaskImage does nothing" bug.
TEST(Masking, MaskImageRekeysFromPristine) {
    CbImage* img = make_two_pixel(0, 0, 0, 255, 255, 255);  // (0,0) black, (1,0) white
    ALLEGRO_BITMAP* bmp = cb::gfx::image_bitmap(img);

    cb_rt_mask_image(img, 0, 0, 0);  // key black
    EXPECT_EQ(get_rgba(bmp, 0, 0).a, 0);    // black -> transparent
    EXPECT_EQ(get_rgba(bmp, 1, 0).a, 255);  // white stays opaque

    // cb::gfx::image_bitmap returns the live (re-cloned) bitmap; re-read it.
    cb_rt_mask_image(img, 255, 255, 255);  // re-key white from pristine
    bmp = cb::gfx::image_bitmap(img);
    EXPECT_EQ(get_rgba(bmp, 0, 0).a, 255);  // black restored to opaque
    EXPECT_EQ(get_rgba(bmp, 1, 0).a, 0);    // white now transparent

    cb_rt_delete_image(img);
}

// DrawImage useMask=0 draws the un-keyed original (pristine), so a keyed pixel
// shows its original colour rather than the background.
TEST(Masking, UseMaskZeroDrawsUnmasked) {
    CbImage* sprite = make_two_pixel(0, 0, 255, 255, 0, 0);  // blue, red
    cb_rt_mask_image(sprite, 255, 0, 0);                     // key red

    CbImage* bg = cb_rt_make_image(2, 1);
    ALLEGRO_BITMAP* bb = cb::gfx::image_bitmap(bg);
    al_set_target_bitmap(bb);
    al_clear_to_color(al_map_rgb(0, 255, 0));

    cb_rt_draw_to_image(bg);
    cb_rt_draw_image_frame_mask(sprite, 0, 0, 0, 0);  // useMask=0 -> draw original

    Rgba p1 = get_rgba(bb, 1, 0);  // would-be-keyed pixel shows its red original
    EXPECT_EQ(p1.r, 255); EXPECT_EQ(p1.g, 0); EXPECT_EQ(p1.b, 0);

    cb_rt_delete_image(sprite);
    cb_rt_delete_image(bg);
}
