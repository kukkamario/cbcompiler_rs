// CoolBasic sprite-Object runtime (FD-036 Phase 4).
//
// The 2D sprite Object: a world position, an angle, an optional animated sprite
// sheet, custom data slots, lifetime, and draw-order membership. Ported from
// cbEnchanted's CBObject + ObjectInterface (src/cbobject.cpp, objectinterface.cpp).
// The pure math — heading, angle/distance, frame slice, rotated bounding box,
// turn wrap, animation/life ticks — lives in the Allegro-free cb_object_data.h so
// it unit-tests without a display; this TU adds the live registry, the bitmaps,
// the catalog entry points, and the camera-space render orchestrator.
//
// Registry — no numeric ids (FD-036, decided 2026-06-20): the CB-visible `Object`
// handle (tag 13) *is* a `CbObject*`, exactly like `Image`/`Font`. There is no
// integer-id map and no shared id space. Lookup is the pointer itself; draw order
// is one std::vector per chain (floor, regular); enumeration walks a creation-
// order std::vector. Use-after-DeleteObject is a dangling handle (matches
// Image/Font). The VM is single-threaded (FD-036), so process-global state is safe.
//
// Textures are shared & reference-counted: each object holds a
// shared_ptr<CbTexture> wrapping the ALLEGRO_BITMAP. CloneObject copies the
// shared_ptr (the bitmap is freed only when the last owner releases it);
// PaintObject/MaskObject mutate the shared bitmap in place (all clones see it);
// MirrorObject repoints the one object to a fresh private holder.
//
// ABI (see cb_runtime.h / the catalog DSL): CB Float args arrive as `double`, Int
// as `int32_t`; strings as `const CbString*`; the `Object` handle is a `CbObject*`.

#include "cb_object.h"
#include "cb_object_data.h"
#include "cb_camera.h"
#include "cb_map.h"
#include "cb_runtime_func.h"

#include <allegro5/allegro.h>
#include <allegro5/allegro_image.h>

#include <algorithm>
#include <memory>
#include <string>
#include <vector>

// Internal glue: the live bitmap behind an `Image` handle (defined in cb_gfx.cpp)
// — used by PaintObject(Object, Image). Mirrors cb_input.cpp's forward-declared
// cb_gfx glue rather than widening a public header.
extern "C" ALLEGRO_BITMAP* cb_gfx_image_bitmap(const CbImage* img);

// ─── Shared texture holder ──────────────────────────────────────────────
//
// One ALLEGRO_BITMAP, reference-counted across an object and its clones. The
// destructor frees the bitmap when the last owner drops it — safer than
// cbEnchanted's raw `copied` flag (which dangles if the original is deleted
// first). Objects render by dereferencing tex->bmp live; never cache the pointer.
struct CbTexture {
    ALLEGRO_BITMAP* bmp = nullptr;
    ~CbTexture() {
        if (bmp) al_destroy_bitmap(bmp);
    }
};

// Default visibility for newly created objects (cbEnchanted's static
// defaultVisible; DefaultVisible sets it, the ctor reads it).
static bool g_default_visible = true;

// ─── Opaque Object handle ───────────────────────────────────────────────
//
// The CB-visible `Object` type (tag 13). Declared (never defined) via the
// `typedef struct CbObject CbObject` in cb_runtime_func.h; defined here. Always
// passed/returned by pointer. Field defaults mirror CBObject's constructor; the
// alphaBlend `= 255` load write is intentionally NOT replicated (render only
// blends when < 1.0, so we keep the documented 0–1 scale honest).
struct CbObject {
    double posX = 0.0, posY = 0.0;
    double sizeX = 0.0, sizeY = 0.0;       // set on load
    double angle = 0.0;                    // degrees, 0° = right
    bool visible;                          // = g_default_visible (ctor)
    std::shared_ptr<CbTexture> tex;        // shared bitmap holder (may be null)
    ALLEGRO_COLOR maskColor;               // = black (ctor)
    double alphaBlend = 1.0;               // 0–1; render blends only when < 1.0
    int32_t frameWidth = 0, frameHeight = 0;
    int32_t startFrame = 0;
    int32_t maxFrames = 0;                 // isAnimated = maxFrames > 0
    double currentFrame = 0.0;             // fractional
    int32_t objectIntData = 0;
    double objectFloatData = 0.0;
    std::string objectStringData;
    bool usingLife = false;
    uint32_t life = 0;
    int32_t animStartFrame = 0, animEndingFrame = 0;
    double animSpeed = 0.0;
    bool animLooping = false;              // cbEnchanted leaves uninit; default false
    bool playing = false;
    bool isFloor;                          // ctor arg
    bool painted = false;

    explicit CbObject(bool floor)
        : visible(g_default_visible), maskColor(al_map_rgb(0, 0, 0)), isFloor(floor) {}
};

namespace {

// The live registry (FD-036 no-id design). `live_objects` is creation order (the
// InitObjectList/NextObject walk); `floor_objects`/`regular_objects` are the draw
// chains (floor drawn before regular). Every live object appears in `live_objects`
// and exactly one draw chain.
std::vector<CbObject*> live_objects;
std::vector<CbObject*> floor_objects;
std::vector<CbObject*> regular_objects;

// Shared stateful enumeration cursor (InitObjectList resets it; NextObject
// advances). Non-reentrant, exactly like cbEnchanted's single iterator.
std::size_t enum_index = 0;

std::string read_cb_string(const CbString* s) {
    std::string out;
    if (s) {
        std::size_t len = cb_rt_string_len(s);
        if (len > 0) {
            out.assign(reinterpret_cast<const char*>(cb_rt_string_data(s)), len);
        }
    }
    return out;
}

// Create a bitmap honoring the headless memory-bitmap fallback (mirrors
// cb_gfx.cpp's MakeImage / cb_map.cpp's load_tileset): without a display, video
// bitmaps can't be made, so fall back to a memory bitmap.
ALLEGRO_BITMAP* create_bitmap_headless(int w, int h) {
    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* b = al_create_bitmap(w, h);
    al_set_new_bitmap_flags(prev_flags);
    return b;
}

ALLEGRO_BITMAP* clone_bitmap_headless(ALLEGRO_BITMAP* src) {
    if (!src) return nullptr;
    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* b = al_clone_bitmap(src);
    al_set_new_bitmap_flags(prev_flags);
    return b;
}

// Loads + masks an object bitmap. Self-inits the subsystems al_load_bitmap needs
// (idempotent) and mirrors the memory-bitmap fallback so objects load headless.
ALLEGRO_BITMAP* load_object_bitmap(const std::string& path, ALLEGRO_COLOR mask) {
    if (!al_is_system_installed()) al_init();
    if (!al_is_image_addon_initialized()) al_init_image_addon();
    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* bmp = al_load_bitmap(path.c_str());
    al_set_new_bitmap_flags(prev_flags);
    if (!bmp) return nullptr;
    al_convert_mask_to_alpha(bmp, mask);
    return bmp;
}

void register_object(CbObject* o) {
    live_objects.push_back(o);
    if (o->isFloor) {
        floor_objects.push_back(o);
    } else {
        regular_objects.push_back(o);
    }
}

void erase_from(std::vector<CbObject*>& v, CbObject* o) {
    v.erase(std::remove(v.begin(), v.end(), o), v.end());
}

// CBObject::startPlaying (cbobject.cpp:219). Resets currentFrame to startf for a
// fresh one-shot, or when a continuous play falls outside the new range.
void start_playing(CbObject* o, int32_t startf, int32_t endf, double spd,
                   bool continuous) {
    if ((!continuous && !o->playing) ||
        (continuous && (o->currentFrame < startf || o->currentFrame > endf))) {
        o->currentFrame = startf;
    }
    o->animStartFrame = startf;
    o->animEndingFrame = endf;
    o->animSpeed = spd;
    o->playing = true;
}

// Default end frame for the no-range Play/LoopObject overloads: the last frame
// (maxFrames-1), or 0 for a non-animated object.
int32_t default_end_frame(const CbObject* o) {
    return o->maxFrames > 0 ? o->maxFrames - 1 : 0;
}

// PlayObject body: endFrame == -1 stops and resets (stopPlaying(false)).
void play_impl(CbObject* o, int32_t startf, int32_t endf, double speed,
               bool continuous) {
    if (!o) return;
    if (endf == -1) {
        o->playing = false;
        o->currentFrame = 0.0;
        return;
    }
    start_playing(o, startf, endf, speed, continuous);
    o->animLooping = false;
}

// LoopObject body: always loops; no -1 stop sentinel (matches cbEnchanted).
void loop_impl(CbObject* o, int32_t startf, int32_t endf, double speed,
               bool continuous) {
    if (!o) return;
    start_playing(o, startf, endf, speed, continuous);
    o->animLooping = true;
}

// ─── Per-object render (camera world space already active) ──────────────
//
// Anchor is (posX, -posY): the convertCoords Y-flip, matching cb_map.cpp's -wy.
// Rotation is -(angle/180)*π about the bitmap/frame centre; ghost alpha folds in
// via a white tint with alpha = alphaBlend.
void render_floor(const CbObject* o, ALLEGRO_BITMAP* bmp) {
    // Tile the bitmap across the camera's visible draw area (cbobject.cpp:347).
    // Visual-only; cannot be golden-asserted. Imageless floor (size 0) would spin
    // the fill loops forever — guard it (cbEnchanted never reaches here unpainted).
    double sizeX = o->sizeX, sizeY = o->sizeY;
    if (sizeX <= 0.0 || sizeY <= 0.0) return;

    double camX = cb_rt_camera_x();
    double camY = cb_rt_camera_y();
    double scrW = 0.0, scrH = 0.0;
    cb_camera_draw_area(&scrW, &scrH);

    double areaTop = camY + 0.5 * scrH;
    double areaBottom = camY - 0.5 * scrH;
    double areaLeft = camX - 0.5 * scrW;
    double areaRight = camX + 0.5 * scrW;

    // cbEnchanted computes the fill in flipped-Y space, then drawBitmap's
    // convertCoords flips Y again — so the final blit Y is -iterY.
    double x = o->posX;
    double y = -o->posY;
    if (x > areaLeft) {
        while (x > areaLeft) x -= sizeX;
    } else {
        while (x + sizeX < areaLeft) x += sizeX;
    }
    if (y > areaBottom) {
        while (y > areaBottom) y -= sizeY;
    } else {
        while (y + sizeY < areaBottom) y += sizeY;
    }

    bool blend = o->alphaBlend < 1.0;
    ALLEGRO_COLOR tint = al_map_rgba_f(1.0f, 1.0f, 1.0f, (float)o->alphaBlend);
    for (; x - sizeX < areaRight; x += sizeX) {
        for (double iterY = y; iterY - sizeY < areaTop; iterY += sizeY) {
            float fx = (float)x;
            float fy = (float)(-iterY);
            if (blend) {
                al_draw_tinted_bitmap(bmp, tint, fx, fy, 0);
            } else {
                al_draw_bitmap(bmp, fx, fy, 0);
            }
        }
    }
}

void render_object(const CbObject* o) {
    if (!o->visible || !o->painted || !o->tex || !o->tex->bmp) return;
    ALLEGRO_BITMAP* bmp = o->tex->bmp;

    if (o->isFloor) {
        render_floor(o, bmp);
        return;
    }

    float dx = (float)o->posX;
    float dy = (float)(-o->posY);  // convertCoords Y-flip
    float rot = (float)(-(o->angle / 180.0) * cb_object_pi);
    ALLEGRO_COLOR tint = al_map_rgba_f(1.0f, 1.0f, 1.0f, (float)o->alphaBlend);

    if (o->maxFrames > 0) {
        int32_t tw = al_get_bitmap_width(bmp);
        int32_t col = 0, row = 0, left = 0, top = 0;
        if (!cb_object_frame_slice(tw, o->frameWidth, o->frameHeight,
                                   (int32_t)o->currentFrame, col, row, left, top)) {
            return;
        }
        al_draw_tinted_scaled_rotated_bitmap_region(
            bmp, (float)left, (float)top, (float)o->frameWidth, (float)o->frameHeight,
            tint, o->frameWidth * 0.5f, o->frameHeight * 0.5f, dx, dy, 1.0f, 1.0f,
            rot, 0);
        return;
    }

    float cx = al_get_bitmap_width(bmp) * 0.5f;
    float cy = al_get_bitmap_height(bmp) * 0.5f;
    if (o->alphaBlend < 1.0) {
        al_draw_tinted_rotated_bitmap(bmp, tint, cx, cy, dx, dy, rot, 0);
    } else {
        al_draw_rotated_bitmap(bmp, cx, cy, dx, dy, rot, 0);
    }
}

}  // namespace

// ─── Creation / destruction ─────────────────────────────────────────────

// LoadObject(path): a single-frame image as an object (Null on load failure).
extern "C" CbObject* cb_rt_load_object(const CbString* path) {
    CbObject* o = new CbObject(false);
    ALLEGRO_BITMAP* bmp = load_object_bitmap(read_cb_string(path), o->maskColor);
    if (!bmp) {
        delete o;
        return nullptr;
    }
    o->tex = std::make_shared<CbTexture>();
    o->tex->bmp = bmp;
    o->sizeX = al_get_bitmap_width(bmp);
    o->sizeY = al_get_bitmap_height(bmp);
    o->painted = true;
    register_object(o);
    return o;
}

// LoadObject(path, rotQuality): rotQuality is accepted but ignored (cbEnchanted).
extern "C" CbObject* cb_rt_load_object_rq(const CbString* path, int32_t rot_quality) {
    (void)rot_quality;
    return cb_rt_load_object(path);
}

// LoadAnimObject(path, frameW, frameH, startFrame, frameCount): a sprite-sheet
// object. Validates frame geometry like cbEnchanted (Null on bad dims / load).
extern "C" CbObject* cb_rt_load_anim_object(const CbString* path, int32_t frame_w,
                                            int32_t frame_h, int32_t start_frame,
                                            int32_t frame_count) {
    CbObject* o = new CbObject(false);
    ALLEGRO_BITMAP* bmp = load_object_bitmap(read_cb_string(path), o->maskColor);
    if (!bmp) {
        delete o;
        return nullptr;
    }
    int32_t tw = al_get_bitmap_width(bmp);
    int32_t th = al_get_bitmap_height(bmp);
    if (frame_w <= 0 || frame_h <= 0 || frame_w > tw || frame_h > th ||
        frame_count > (tw / frame_w) * (th / frame_h)) {
        al_destroy_bitmap(bmp);
        delete o;
        return nullptr;
    }
    o->tex = std::make_shared<CbTexture>();
    o->tex->bmp = bmp;
    o->frameWidth = frame_w;
    o->frameHeight = frame_h;
    o->sizeX = frame_w;
    o->sizeY = frame_h;
    o->startFrame = start_frame;
    o->maxFrames = frame_count;
    o->painted = true;
    register_object(o);
    return o;
}

// LoadAnimObject(..., rotQuality): rotQuality accepted but ignored.
extern "C" CbObject* cb_rt_load_anim_object_rq(const CbString* path, int32_t frame_w,
                                               int32_t frame_h, int32_t start_frame,
                                               int32_t frame_count, int32_t rot_quality) {
    (void)rot_quality;
    return cb_rt_load_anim_object(path, frame_w, frame_h, start_frame, frame_count);
}

// MakeObject(): an empty (imageless, unpainted) object.
extern "C" CbObject* cb_rt_make_object(void) {
    CbObject* o = new CbObject(false);
    register_object(o);
    return o;
}

// MakeObjectFloor(): a floor object (drawn before regular objects).
extern "C" CbObject* cb_rt_make_object_floor(void) {
    CbObject* o = new CbObject(true);
    register_object(o);
    return o;
}

// CloneObject(obj): shares the texture (refcount++); position and angle reset to
// 0; visibility forced true (faithful to cbEnchanted). Copies frame/anim metadata.
extern "C" CbObject* cb_rt_clone_object(const CbObject* src) {
    if (!src) return nullptr;
    CbObject* o = new CbObject(src->isFloor);
    o->tex = src->tex;  // share the holder
    o->maskColor = src->maskColor;
    o->frameWidth = src->frameWidth;
    o->frameHeight = src->frameHeight;
    o->startFrame = src->startFrame;
    o->maxFrames = src->maxFrames;
    o->animStartFrame = src->animStartFrame;
    o->animEndingFrame = src->animEndingFrame;
    o->animSpeed = src->animSpeed;
    o->animLooping = src->animLooping;
    o->painted = src->painted;
    o->sizeX = src->sizeX;
    o->sizeY = src->sizeY;
    o->visible = true;  // cbEnchanted forces visible=true on a clone
    // posX/posY/angle/currentFrame stay at constructor defaults (0) — NOT copied.
    register_object(o);
    return o;
}

// DeleteObject(obj): unregisters and frees. The shared texture's bitmap is freed
// only when the last owner (a clone) releases it. Dangling-handle if reused.
extern "C" void cb_rt_delete_object(CbObject* o) {
    if (!o) return;
    erase_from(live_objects, o);
    erase_from(floor_objects, o);
    erase_from(regular_objects, o);
    delete o;
}

// ClearObjects(): deletes all objects. Objects-only — the active tilemap is left
// alone (FD-036 decoupling decision; LoadMap/MakeMap own the map's lifetime).
extern "C" void cb_rt_clear_objects(void) {
    for (CbObject* o : live_objects) delete o;
    live_objects.clear();
    floor_objects.clear();
    regular_objects.clear();
    enum_index = 0;
}

// ─── Position / movement ────────────────────────────────────────────────

extern "C" void cb_rt_position_object(CbObject* o, double x, double y) {
    if (!o) return;
    o->posX = x;
    o->posY = y;
}

// PositionObject(obj, x, y, z): z accepted but ignored (own arity overload).
extern "C" void cb_rt_position_object_z(CbObject* o, double x, double y, double z) {
    (void)z;
    cb_rt_position_object(o, x, y);
}

extern "C" void cb_rt_move_object(CbObject* o, double forward, double side) {
    if (!o) return;
    double dx = 0.0, dy = 0.0;
    cb_object_move_delta(o->angle, forward, side, dx, dy);
    o->posX += dx;
    o->posY += dy;
}

extern "C" void cb_rt_move_object_z(CbObject* o, double forward, double side, double z) {
    (void)z;
    cb_rt_move_object(o, forward, side);
}

extern "C" void cb_rt_translate_object(CbObject* o, double dx, double dy) {
    if (!o) return;
    o->posX += dx;
    o->posY += dy;
}

extern "C" void cb_rt_translate_object_z(CbObject* o, double dx, double dy, double dz) {
    (void)dz;
    cb_rt_translate_object(o, dx, dy);
}

extern "C" void cb_rt_clone_object_position(CbObject* dst, const CbObject* src) {
    if (!dst || !src) return;
    dst->posX = src->posX;
    dst->posY = src->posY;
}

extern "C" double cb_rt_object_x(const CbObject* o) { return o ? o->posX : 0.0; }
extern "C" double cb_rt_object_y(const CbObject* o) { return o ? o->posY : 0.0; }

// ─── Rotation / angle ───────────────────────────────────────────────────

extern "C" void cb_rt_rotate_object(CbObject* o, double angle) {
    if (o) o->angle = angle;
}

extern "C" void cb_rt_turn_object(CbObject* o, double speed) {
    if (o) o->angle = cb_object_turn(o->angle, speed);
}

extern "C" void cb_rt_point_object(CbObject* o, const CbObject* target) {
    if (!o || !target) return;
    o->angle = cb_object_angle2(o->posX, o->posY, target->posX, target->posY);
}

extern "C" void cb_rt_clone_object_orientation(CbObject* dst, const CbObject* src) {
    if (dst && src) dst->angle = src->angle;
}

extern "C" double cb_rt_object_angle(const CbObject* o) { return o ? o->angle : 0.0; }

extern "C" double cb_rt_get_angle2(const CbObject* a, const CbObject* b) {
    if (!a || !b) return 0.0;
    return cb_object_angle2(a->posX, a->posY, b->posX, b->posY);
}

extern "C" double cb_rt_distance2(const CbObject* a, const CbObject* b) {
    if (!a || !b) return 0.0;
    return cb_object_distance2(a->posX, a->posY, b->posX, b->posY);
}

// ─── Appearance ─────────────────────────────────────────────────────────

// PaintObject(obj, image): replaces the object's bitmap with a masked clone of
// the image. Mutates inside the shared holder so existing clones pick it up.
extern "C" void cb_rt_paint_object_image(CbObject* o, const CbImage* img) {
    if (!o) return;
    ALLEGRO_BITMAP* src = cb_gfx_image_bitmap(img);
    if (!src) return;
    ALLEGRO_BITMAP* clone = clone_bitmap_headless(src);
    if (!clone) return;
    al_convert_mask_to_alpha(clone, o->maskColor);
    if (!o->tex) o->tex = std::make_shared<CbTexture>();
    if (o->tex->bmp) al_destroy_bitmap(o->tex->bmp);
    o->tex->bmp = clone;
    o->sizeX = al_get_bitmap_width(clone);
    o->sizeY = al_get_bitmap_height(clone);
    o->painted = true;
}

// PaintObject(obj, source): copies another object's texture (a fresh clone, in
// place inside the shared holder). An unpainted source leaves obj unpainted.
extern "C" void cb_rt_paint_object_object(CbObject* o, const CbObject* src) {
    if (!o || !src) return;
    if (src->tex && src->tex->bmp) {
        ALLEGRO_BITMAP* clone = clone_bitmap_headless(src->tex->bmp);
        if (!clone) return;
        if (!o->tex) o->tex = std::make_shared<CbTexture>();
        if (o->tex->bmp) al_destroy_bitmap(o->tex->bmp);
        o->tex->bmp = clone;
        o->painted = true;
    } else {
        if (o->tex && o->tex->bmp) {
            al_destroy_bitmap(o->tex->bmp);
            o->tex->bmp = nullptr;
        }
        o->painted = false;
    }
    o->sizeX = src->sizeX;
    o->sizeY = src->sizeY;
}

// MaskObject(obj, r, g, b): sets the transparent colour key in place on the
// shared bitmap (all clones see it). Destructive/cumulative (no unmasked copy).
extern "C" void cb_rt_mask_object(CbObject* o, int32_t r, int32_t g, int32_t b) {
    if (!o || !o->tex || !o->tex->bmp) return;
    o->maskColor = al_map_rgb((unsigned char)r, (unsigned char)g, (unsigned char)b);
    al_convert_mask_to_alpha(o->tex->bmp, o->maskColor);
}

// GhostObject(obj, alpha): alpha 0–100, scaled to 0–1 and clamped.
extern "C" void cb_rt_ghost_object(CbObject* o, double alpha) {
    if (!o) return;
    double a = alpha / 100.0;
    if (a < 0.0) a = 0.0;
    if (a > 1.0) a = 1.0;
    o->alphaBlend = a;
}

// MirrorObject(obj, dir): 0=horizontal, 1=vertical, 2=both. Regular objects only.
// Allocates a fresh PRIVATE holder and repoints only this object — clones keep
// the old shared bitmap (faithful to cbEnchanted's new render target).
extern "C" void cb_rt_mirror_object(CbObject* o, int32_t dir) {
    if (!o || dir < 0 || dir > 2 || o->isFloor) return;
    if (!o->tex || !o->tex->bmp) return;
    ALLEGRO_BITMAP* srcb = o->tex->bmp;
    int w = al_get_bitmap_width(srcb);
    int h = al_get_bitmap_height(srcb);
    int flip = ALLEGRO_FLIP_HORIZONTAL;
    if (dir == 1) flip = ALLEGRO_FLIP_VERTICAL;
    else if (dir == 2) flip = ALLEGRO_FLIP_HORIZONTAL | ALLEGRO_FLIP_VERTICAL;

    ALLEGRO_BITMAP* dst = create_bitmap_headless(w, h);
    if (!dst) return;
    ALLEGRO_BITMAP* prev = al_get_target_bitmap();
    al_set_target_bitmap(dst);
    al_clear_to_color(al_map_rgba(0, 0, 0, 0));
    al_draw_bitmap(srcb, 0, 0, flip);
    if (prev) al_set_target_bitmap(prev);

    auto holder = std::make_shared<CbTexture>();
    holder->bmp = dst;
    o->tex = holder;  // private; clones retain the old shared holder
    o->painted = true;
}

// ShowObject(obj, visible): show/hide (hidden objects still update & collide).
extern "C" void cb_rt_show_object(CbObject* o, int32_t visible) {
    if (o) o->visible = visible != 0;
}

// DefaultVisible(visible): default visibility for objects created afterwards.
extern "C" void cb_rt_default_visible(int32_t visible) {
    g_default_visible = visible != 0;
}

// ObjectOrder(obj, direction): 1 = to front (drawn last/on top), -1 = to back.
extern "C" void cb_rt_object_order(CbObject* o, int32_t direction) {
    if (!o || (direction != 1 && direction != -1)) return;
    std::vector<CbObject*>& chain = o->isFloor ? floor_objects : regular_objects;
    auto it = std::find(chain.begin(), chain.end(), o);
    if (it == chain.end()) return;
    chain.erase(it);
    if (direction == 1) {
        chain.push_back(o);  // drawn last → on top
    } else {
        chain.insert(chain.begin(), o);  // drawn first → at back
    }
}

extern "C" int32_t cb_rt_object_size_x(const CbObject* o) {
    return o ? cb_object_size_x(o->sizeX, o->sizeY, o->angle) : 0;
}

extern "C" int32_t cb_rt_object_size_y(const CbObject* o) {
    return o ? cb_object_size_y(o->sizeX, o->sizeY, o->angle) : 0;
}

// ─── Animation ──────────────────────────────────────────────────────────

// PlayObject overloads (arities 1/3/4/5). Defaults: startFrame=0, endFrame=last,
// speed=0.1, continuous=0. endFrame=-1 stops and resets.
extern "C" void cb_rt_play_object(CbObject* o) {
    if (!o) return;
    play_impl(o, 0, default_end_frame(o), 0.1, false);
}
extern "C" void cb_rt_play_object3(CbObject* o, int32_t start_f, int32_t end_f) {
    play_impl(o, start_f, end_f, 0.1, false);
}
extern "C" void cb_rt_play_object4(CbObject* o, int32_t start_f, int32_t end_f, double speed) {
    play_impl(o, start_f, end_f, speed, false);
}
extern "C" void cb_rt_play_object5(CbObject* o, int32_t start_f, int32_t end_f, double speed, int32_t continuous) {
    play_impl(o, start_f, end_f, speed, continuous != 0);
}

// LoopObject overloads (arities 1/3/4/5); same defaults, always loops.
extern "C" void cb_rt_loop_object(CbObject* o) {
    if (!o) return;
    loop_impl(o, 0, default_end_frame(o), 0.1, false);
}
extern "C" void cb_rt_loop_object3(CbObject* o, int32_t start_f, int32_t end_f) {
    loop_impl(o, start_f, end_f, 0.1, false);
}
extern "C" void cb_rt_loop_object4(CbObject* o, int32_t start_f, int32_t end_f, double speed) {
    loop_impl(o, start_f, end_f, speed, false);
}
extern "C" void cb_rt_loop_object5(CbObject* o, int32_t start_f, int32_t end_f, double speed, int32_t continuous) {
    loop_impl(o, start_f, end_f, speed, continuous != 0);
}

// StopObject(obj): stops animation, keeping the current frame.
extern "C" void cb_rt_stop_object(CbObject* o) {
    if (o) o->playing = false;
}

extern "C" int32_t cb_rt_object_playing(const CbObject* o) {
    return o && o->playing ? 1 : 0;
}

extern "C" double cb_rt_object_frame(const CbObject* o) {
    return o ? o->currentFrame : 0.0;
}

// ─── Custom data slots / life ───────────────────────────────────────────

extern "C" int32_t cb_rt_object_integer_get(const CbObject* o) {
    return o ? o->objectIntData : 0;
}
extern "C" void cb_rt_object_integer_set(CbObject* o, int32_t value) {
    if (o) o->objectIntData = value;
}

extern "C" double cb_rt_object_float_get(const CbObject* o) {
    return o ? o->objectFloatData : 0.0;
}
extern "C" void cb_rt_object_float_set(CbObject* o, double value) {
    if (o) o->objectFloatData = value;
}

extern "C" CbString* cb_rt_object_string_get(const CbObject* o) {
    if (!o || o->objectStringData.empty()) {
        return cb_rt_string_retain(const_cast<CbString*>(cb_runtime_string_api.empty));
    }
    return cb_rt_string_from_literal(
        reinterpret_cast<const uint8_t*>(o->objectStringData.data()),
        o->objectStringData.size());
}
extern "C" void cb_rt_object_string_set(CbObject* o, const CbString* value) {
    if (o) o->objectStringData = read_cb_string(value);
}

// ObjectLife get/set; set marks the object as using life (decremented per update
// tick by the Phase-5 game loop, auto-deleting at 0).
extern "C" int32_t cb_rt_object_life_get(const CbObject* o) {
    return o ? (int32_t)o->life : 0;
}
extern "C" void cb_rt_object_life_set(CbObject* o, int32_t frames) {
    if (!o) return;
    o->usingLife = true;
    o->life = (uint32_t)frames;
}

// ─── Enumeration ────────────────────────────────────────────────────────

// InitObjectList(): reset the shared enumeration cursor.
extern "C" void cb_rt_init_object_list(void) { enum_index = 0; }

// NextObject(): the next object in creation order, or Null at the end. Map ids
// are intentionally NOT surfaced (Map is a separate opaque type).
extern "C" CbObject* cb_rt_next_object(void) {
    if (enum_index >= live_objects.size()) return nullptr;
    return live_objects[enum_index++];
}

// ─── Render orchestrator (glue for cb_gfx.cpp; see cb_object.h) ──────────
//
// The cbEnchanted drawObjects analogue: one world-transform bracket over map
// background (layer 0) → floor objects → regular objects → map foreground (layer
// 1). A no-op when there is nothing to draw. The caller (do_draw_screen) has
// already set the backbuffer as the target.
extern "C" void cb_objects_render_all(void) {
    if (!cb_map_active() && floor_objects.empty() && regular_objects.empty()) return;
    if (!al_get_target_bitmap()) return;

    al_use_transform(cb_camera_world_transform());
    cb_map_render_layer(0);
    for (CbObject* o : floor_objects) render_object(o);
    for (CbObject* o : regular_objects) render_object(o);
    cb_map_render_layer(1);

    ALLEGRO_TRANSFORM id;
    al_identity_transform(&id);
    al_use_transform(&id);
}
