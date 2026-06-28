// CoolBasic sprite-Object runtime.
//
// The 2D sprite Object: a world position, an angle, an optional animated sprite
// sheet, custom data slots, lifetime, and draw-order membership.
// The pure math — heading, angle/distance, frame slice, rotated bounding box,
// turn wrap, animation/life ticks — lives in the Allegro-free cb_object_data.h so
// it unit-tests without a display; this TU adds the live registry, the bitmaps,
// the catalog entry points, and the camera-space render orchestrator.
//
// Registry — no numeric ids: the CB-visible `Object`
// handle (tag 13) *is* a `CbObject*`, exactly like `Image`/`Font`. There is no
// integer-id map and no shared id space. Lookup is the pointer itself; draw order
// is one std::vector per chain (floor, regular); enumeration walks a creation-
// order std::vector. Use-after-DeleteObject is a dangling handle (matches
// Image/Font). The VM is single-threaded, so process-global state is safe.
//
// Textures are shared & reference-counted: each object holds a
// shared_ptr<CbTexture>, which keeps both the pristine (unmasked) bitmap and the
// masked bitmap that actually draws. CloneObject copies the shared_ptr (bitmaps
// freed only when the last owner releases them); PaintObject/MaskObject re-derive
// the shared holder's masked bitmap from its pristine copy (all clones see it);
// MirrorObject repoints the one object to a fresh private holder.
//
// ABI (see cb_runtime.h / the catalog DSL): CB Float args arrive as `double`, Int
// as `int32_t`; strings as `const CbString*`; the `Object` handle is a `CbObject*`.

#include "cb_object.h"
#include "cb_object_data.h"
#include "cb_particle.h"
#include "cb_collision_data.h"
#include "cb_camera.h"
#include "cb_gfx.h"           // cb::gfx::image_bitmap / image_pristine / apply_*
#include "cb_map.h"
#include "cb_map_data.h"
#include "cb_runtime_func.h"

#include <allegro5/allegro.h>
#include <allegro5/allegro_image.h>

#include <algorithm>
#include <memory>
#include <string>
#include <vector>

// ─── Shared texture holder ──────────────────────────────────────────────
//
// One ALLEGRO_BITMAP, reference-counted across an object and its clones. The
// destructor frees the bitmap when the last owner drops it — safer than a raw
// `copied` flag (which would dangle if the original is deleted first). Objects
// render by dereferencing tex->bmp live; never cache the pointer.
struct CbTexture {
    ALLEGRO_BITMAP* bmp = nullptr;       // masked — what render_object draws
    ALLEGRO_BITMAP* pristine = nullptr;  // unmasked original — re-key source for MaskObject
    ~CbTexture() {
        if (bmp) al_destroy_bitmap(bmp);
        if (pristine) al_destroy_bitmap(pristine);
    }
};

// Default visibility for newly created objects (CoolBasic's DefaultVisible
// global; DefaultVisible sets it, the ctor reads it).
static bool g_default_visible = true;

// ─── Opaque Object handle ───────────────────────────────────────────────
//
// The CB-visible `Object` type (tag 13). Declared (never defined) via the
// `typedef struct CbObject CbObject` in cb_runtime_func.h; defined here. Always
// passed/returned by pointer. Field defaults mirror CoolBasic's object
// constructor; an alphaBlend `= 255` load write is intentionally NOT replicated (render only
// blends when < 1.0, so we keep the documented 0–1 scale honest).
struct CbObject;

// A single recorded collision: the other object (Null for a
// map-wall hit — a Map is not an Object, so GetCollision yields Null there), the
// contact normal angle (degrees), and the contact point in world coordinates.
struct CbCollision {
    CbObject* other;
    double angle;
    double x;
    double y;
};

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
    bool animLooping = false;              // default false (CoolBasic leaves it uninit)
    bool playing = false;
    bool isFloor;                          // ctor arg
    bool painted = false;

    // ─── Collision ────────────────────────────────────
    // range1/range2 = collision bounds (box: width,height; circle: diameter in
    // range1). Default 0×0; LoadObject/LoadAnimObject/CloneObject set them to the
    // image size, MakeObject/MakeObjectFloor leave them 0 (so a made object's
    // collisions are inert until ObjectRange is called — faithful to CoolBasic).
    // checkCollisions gates this object's checks for the current tick
    // (ResetObjectCollision clears it; the update tick resets it to true).
    // `collisions` is this frame's recorded contacts (1-based GetCollision/
    // CollisionX/Y/Angle), wiped each update tick.
    double range1 = 0.0, range2 = 0.0;
    bool checkCollisions = true;
    std::vector<CbCollision> collisions;

    // ─── Picking ──────────────────────────────────────
    // 0 = not pickable; 1 = box, 2 = circle, 3 = pixel (raycast no-op). Set by
    // ObjectPickable; read by ObjectPick's raycast and CameraPick's point test.
    int pickStyle = 0;

    // ─── Particle emitter ─────────────────────────────────────
    // Non-null exactly for an emitter (MakeEmitter). This pointer IS the
    // emitter-kind discriminator: an
    // emitter renders its particles instead of a sprite, updates them each tick,
    // defers its DeleteObject until the particles drain, and is excluded from
    // picking/collision. The `tex`/frameWidth/frameHeight fields hold the
    // particle image (copied from MakeEmitter's Image), and maxFrames its strip
    // length, reusing the object texture machinery.
    std::unique_ptr<cb::particle::CbEmitterState> emitter;

    explicit CbObject(bool floor)
        : visible(g_default_visible), maskColor(al_map_rgb(0, 0, 0)), isFloor(floor) {}
};

namespace cb::object {

namespace {

// The live registry (no-id design). `live_objects` is creation order (the
// InitObjectList/NextObject walk); `floor_objects`/`regular_objects` are the draw
// chains (floor drawn before regular). Every live object appears in `live_objects`
// and exactly one draw chain.
std::vector<CbObject*> live_objects;
std::vector<CbObject*> floor_objects;
std::vector<CbObject*> regular_objects;

// Deleted emitters draining their remaining particles. DeleteObject on
// an emitter with live particles stops it spawning and moves it here (out of
// live_objects, so no longer enumerable/addressable) but keeps it in its draw
// chain so the particles finish; update_all drains and frees them.
std::vector<CbObject*> rogue_emitters;

// Shared stateful enumeration cursor (InitObjectList resets it; NextObject
// advances). Non-reentrant — a single shared iterator, as in CoolBasic.
std::size_t enum_index = 0;

// ─── Collision-check registry ──────────────────────────
//
// SetupCollision is a *persistent* registration (CoolBasic's collision-check
// list): each entry is re-tested every update tick, not one-shot. Cleared only
// by ClearCollisions or when an object is deleted. `a` is the colliding object;
// `b` is the target object (or null when `bIsMap`, i.e. the active tilemap is the
// target). typeA/typeB: 1=box, 2=circle, 4=map(B only). handling: 0=report,
// 1=stop, 2=slide. safeX/safeY is the last collision-free position (seeded to a's
// position at setup, updated by each test) — the box/circle resolvers push back
// relative to it.
struct CbCollisionCheck {
    CbObject* a;
    CbObject* b;
    bool bIsMap;
    int typeA, typeB, handling;
    double safeX, safeY;
};

std::vector<CbCollisionCheck> collision_checks;

// ─── Pickable registry + last-pick state ───────────────
//
// ObjectPickable adds/removes objects here; ObjectPick raycasts the picker's
// facing ray against each and keeps the nearest hit, recording it for
// PickedObject/X/Y/Angle. A single set of last-pick slots — non-reentrant.
std::vector<CbObject*> pickable_objects;
CbObject* last_picked = nullptr;
double last_picked_x = 0.0;
double last_picked_y = 0.0;
double last_picked_angle = 0.0;

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
    cb::gfx::apply_bitmap_defaults();
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
    cb::gfx::apply_bitmap_defaults();
    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* b = al_clone_bitmap(src);
    al_set_new_bitmap_flags(prev_flags);
    return b;
}

// Loads an object's *pristine* (unmasked) bitmap. Self-inits the subsystems
// al_load_bitmap needs (idempotent) and mirrors the memory-bitmap fallback so
// objects load headless. Masking is applied later by set_object_texture, which
// keeps the pristine copy so MaskObject can re-key to any colour.
ALLEGRO_BITMAP* load_object_bitmap(const std::string& path) {
    if (!al_is_system_installed()) al_init();
    if (!al_is_image_addon_initialized()) al_init_image_addon();
    cb::gfx::apply_bitmap_defaults();
    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* bmp = al_load_bitmap(path.c_str());
    al_set_new_bitmap_flags(prev_flags);
    return bmp;
}

// Installs `pristine` as the object's texture: keeps it as the unmasked original
// and derives the drawn bitmap as a masked clone keyed by o->maskColor — the
// pristine + masked bitmap pair. Takes ownership of `pristine`. On clone failure
// the object still holds the pristine (so it is not leaked and a later
// MaskObject can recover).
void set_object_texture(CbObject* o, ALLEGRO_BITMAP* pristine) {
    o->tex = std::make_shared<CbTexture>();
    o->tex->pristine = pristine;
    o->tex->bmp = clone_bitmap_headless(pristine);
    if (o->tex->bmp) al_convert_mask_to_alpha(o->tex->bmp, o->maskColor);
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

// startPlaying: resets currentFrame to startf for a fresh one-shot, or when a
// continuous play falls outside the new range.
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

// LoopObject body: always loops; no -1 stop sentinel (matches CoolBasic).
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
    // Tile the bitmap across the camera's visible draw area.
    // Visual-only; cannot be golden-asserted. Imageless floor (size 0) would spin
    // the fill loops forever — guard it (CoolBasic never reaches here unpainted).
    double sizeX = o->sizeX, sizeY = o->sizeY;
    if (sizeX <= 0.0 || sizeY <= 0.0) return;

    double camX = cb_rt_camera_x();
    double camY = cb_rt_camera_y();
    double scrW = 0.0, scrH = 0.0;
    cb::camera::draw_area(&scrW, &scrH);

    double areaTop = camY + 0.5 * scrH;
    double areaBottom = camY - 0.5 * scrH;
    double areaLeft = camX - 0.5 * scrW;
    double areaRight = camX + 0.5 * scrW;

    // The fill is computed in flipped-Y space, then the draw's convertCoords
    // flips Y again — so the final blit Y is -iterY.
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

// Draw an emitter's live particles. Camera world space is already
// active (called from render_all's bracket). Each particle is a translated,
// centred sprite — no rotation, no ghost alpha. For an
// animated strip the frame is chosen forward over the particle's life and the
// cell sliced with the correct row/offset math.
void render_particles(const CbObject* o) {
    const cb::particle::CbEmitterState& e = *o->emitter;
    if (!o->tex || !o->tex->bmp || e.particles.empty()) return;
    ALLEGRO_BITMAP* bmp = o->tex->bmp;
    bool animated = e.frameCount > 1 && o->frameWidth > 0 && o->frameHeight > 0;
    int32_t tw = al_get_bitmap_width(bmp);
    float fullCx = al_get_bitmap_width(bmp) * 0.5f;
    float fullCy = al_get_bitmap_height(bmp) * 0.5f;
    for (const cb::particle::CbParticle& p : e.particles) {
        float dx = (float)p.x;
        float dy = (float)(-p.y);  // convertCoords Y-flip
        if (animated) {
            int32_t frame = cb::particle::particle_frame(p.lifeTime,
                                                         e.particleLifeTime,
                                                         e.frameCount);
            int32_t col = 0, row = 0, left = 0, top = 0;
            if (!cb_object_frame_slice(tw, o->frameWidth, o->frameHeight, frame,
                                       col, row, left, top)) {
                continue;
            }
            al_draw_bitmap_region(bmp, (float)left, (float)top,
                                  (float)o->frameWidth, (float)o->frameHeight,
                                  dx - o->frameWidth * 0.5f,
                                  dy - o->frameHeight * 0.5f, 0);
        } else {
            al_draw_bitmap(bmp, dx - fullCx, dy - fullCy, 0);
        }
    }
}

void render_object(const CbObject* o) {
    if (o->emitter) {
        if (o->visible) render_particles(o);
        return;
    }
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

// Resolve the emitter state behind an Object handle for the Particle* commands.
// Returns null and raises a clean runtime error (trap channel) if the
// handle is not an emitter — classic CB blind-casts here (UB); we refuse. The
// Particle* commands are typed to take an Object, so the type checker can't catch
// a plain-object argument (trap, not a silent no-op).
cb::particle::CbEmitterState* require_emitter(CbObject* o, const char* cmd) {
    if (o && o->emitter) return o->emitter.get();
    const CbHostApi* h = cb_host();
    if (h && h->raise_error) {
        std::string msg = std::string(cmd) +
                          ": object is not a particle emitter (create it with MakeEmitter)";
        CbString* s = cb_rt_string_from_literal(
            reinterpret_cast<const uint8_t*>(msg.data()), msg.size());
        h->raise_error(s);       // host copies the bytes at the boundary
        cb_rt_string_release(s);
    }
    return nullptr;
}

}  // namespace

// ─── Creation / destruction ─────────────────────────────────────────────

// LoadObject(path): a single-frame image as an object (Null on load failure).
extern "C" CbObject* cb_rt_load_object(const CbString* path) {
    CbObject* o = new CbObject(false);
    ALLEGRO_BITMAP* bmp = load_object_bitmap(read_cb_string(path));
    if (!bmp) {
        delete o;
        return nullptr;
    }
    set_object_texture(o, bmp);  // keeps `bmp` as pristine, derives masked copy
    o->sizeX = al_get_bitmap_width(bmp);
    o->sizeY = al_get_bitmap_height(bmp);
    o->range1 = o->sizeX;  // LoadObject seeds the collision range to image size
    o->range2 = o->sizeY;
    o->painted = true;
    register_object(o);
    return o;
}

// LoadObject(path, rotQuality): rotQuality is accepted but ignored (as in CoolBasic).
extern "C" CbObject* cb_rt_load_object_rq(const CbString* path, int32_t rot_quality) {
    (void)rot_quality;
    return cb_rt_load_object(path);
}

// LoadAnimObject(path, frameW, frameH, startFrame, frameCount): a sprite-sheet
// object. Validates frame geometry like CoolBasic (Null on bad dims / load).
extern "C" CbObject* cb_rt_load_anim_object(const CbString* path, int32_t frame_w,
                                            int32_t frame_h, int32_t start_frame,
                                            int32_t frame_count) {
    CbObject* o = new CbObject(false);
    ALLEGRO_BITMAP* bmp = load_object_bitmap(read_cb_string(path));
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
    set_object_texture(o, bmp);  // keeps `bmp` as pristine, derives masked copy
    o->frameWidth = frame_w;
    o->frameHeight = frame_h;
    o->sizeX = frame_w;
    o->sizeY = frame_h;
    o->range1 = o->sizeX;  // collision range seeded to a single frame's size
    o->range2 = o->sizeY;
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
// 0; visibility forced true (faithful to CoolBasic). Copies frame/anim metadata.
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
    o->range1 = src->sizeX;  // clone range = source IMAGE size (CoolBasic)
    o->range2 = src->sizeY;
    o->visible = true;  // CoolBasic forces visible=true on a clone
    // posX/posY/angle/currentFrame stay at constructor defaults (0) — NOT copied.
    register_object(o);
    return o;
}

// DeleteObject(obj): unregisters and frees. The shared texture's bitmap is freed
// only when the last owner (a clone) releases it. Dangling-handle if reused.
extern "C" void cb_rt_delete_object(CbObject* o) {
    if (!o) return;
    // Emitter graceful drain: a deleted emitter that still has live
    // particles stops spawning and finishes them before being freed. It leaves
    // live_objects (no longer addressable/enumerable) but stays in its draw chain
    // so the particles keep rendering; update_rogue_emitters drains then frees it.
    // (This also smooths an emitter auto-deleted by ObjectLife, whose particles
    // would otherwise be dropped abruptly on life expiry.)
    if (o->emitter && !o->emitter->stop && !o->emitter->particles.empty()) {
        erase_from(live_objects, o);
        erase_from(pickable_objects, o);  // emitters are never pickable; defensive
        if (last_picked == o) last_picked = nullptr;
        o->emitter->stop = true;
        rogue_emitters.push_back(o);
        return;
    }
    erase_from(live_objects, o);
    erase_from(floor_objects, o);
    erase_from(regular_objects, o);
    erase_from(rogue_emitters, o);
    erase_from(pickable_objects, o);
    if (last_picked == o) last_picked = nullptr;
    // Drop any collision check that references the deleted object (else the next
    // tick would test a dangling pointer). CoolBasic does the same.
    collision_checks.erase(
        std::remove_if(collision_checks.begin(), collision_checks.end(),
                       [o](const CbCollisionCheck& c) { return c.a == o || c.b == o; }),
        collision_checks.end());
    delete o;
}

// ClearObjects(): deletes all objects. Objects-only — the active tilemap is left
// alone (LoadMap/MakeMap own the map's lifetime).
extern "C" void cb_rt_clear_objects(void) {
    for (CbObject* o : live_objects) delete o;
    // Rogue (deleted, still-draining) emitters are no longer in live_objects, so
    // free them separately. They remain in regular_objects, but that is
    // cleared below without dereferencing, so there is no double free.
    for (CbObject* o : rogue_emitters) delete o;
    live_objects.clear();
    floor_objects.clear();
    regular_objects.clear();
    rogue_emitters.clear();
    pickable_objects.clear();
    last_picked = nullptr;
    collision_checks.clear();  // every check referenced a now-deleted object
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

// MoveObject(obj, forward): 2-arg form — `side` defaults to 0. This is the
// common CoolBasic idiom (`MoveObject obj, dist`); the compiler fills the
// omitted side/z with 0 (CoolBasic always pops 4: z, side, fwrd, id).
extern "C" void cb_rt_move_object_fwd(CbObject* o, double forward) {
    cb_rt_move_object(o, forward, 0.0);
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
    // Paint from the image's *pristine* bitmap so the object's own maskColor
    // governs the key (and MaskObject can later re-key from this pristine).
    ALLEGRO_BITMAP* srcPristine = cb::gfx::image_pristine(img);
    if (!srcPristine) return;
    ALLEGRO_BITMAP* pristine = clone_bitmap_headless(srcPristine);
    if (!pristine) return;
    ALLEGRO_BITMAP* masked = clone_bitmap_headless(pristine);
    if (masked) al_convert_mask_to_alpha(masked, o->maskColor);
    if (!o->tex) o->tex = std::make_shared<CbTexture>();
    if (o->tex->bmp) al_destroy_bitmap(o->tex->bmp);
    if (o->tex->pristine) al_destroy_bitmap(o->tex->pristine);
    o->tex->pristine = pristine;
    o->tex->bmp = masked;
    o->sizeX = al_get_bitmap_width(pristine);
    o->sizeY = al_get_bitmap_height(pristine);
    o->painted = true;
}

// PaintObject(obj, source): copies another object's texture (a fresh clone, in
// place inside the shared holder). An unpainted source leaves obj unpainted.
extern "C" void cb_rt_paint_object_object(CbObject* o, const CbObject* src) {
    if (!o || !src) return;
    if (src->tex && src->tex->bmp) {
        // Clone both bitmaps so the painted object can still re-key (MaskObject).
        ALLEGRO_BITMAP* masked = clone_bitmap_headless(src->tex->bmp);
        if (!masked) return;
        ALLEGRO_BITMAP* pristine = src->tex->pristine
            ? clone_bitmap_headless(src->tex->pristine) : nullptr;
        if (!o->tex) o->tex = std::make_shared<CbTexture>();
        if (o->tex->bmp) al_destroy_bitmap(o->tex->bmp);
        if (o->tex->pristine) al_destroy_bitmap(o->tex->pristine);
        o->tex->bmp = masked;
        o->tex->pristine = pristine;
        o->painted = true;
    } else {
        if (o->tex && o->tex->bmp) {
            al_destroy_bitmap(o->tex->bmp);
            o->tex->bmp = nullptr;
        }
        if (o->tex && o->tex->pristine) {
            al_destroy_bitmap(o->tex->pristine);
            o->tex->pristine = nullptr;
        }
        o->painted = false;
    }
    o->sizeX = src->sizeX;
    o->sizeY = src->sizeY;
}

// MaskObject(obj, r, g, b): re-keys the transparent colour. Re-clones the masked
// bitmap from the pristine copy first, so the new key replaces any prior one
// (e.g. the black auto-mask applied at load) instead of stacking — without a
// pristine, re-keying a different colour could never restore previously-keyed
// pixels. Mutates the shared holder, so all clones see the new mask.
extern "C" void cb_rt_mask_object(CbObject* o, int32_t r, int32_t g, int32_t b) {
    if (!o || !o->tex || !o->tex->pristine) return;
    o->maskColor = al_map_rgb((unsigned char)r, (unsigned char)g, (unsigned char)b);
    if (o->tex->bmp) al_destroy_bitmap(o->tex->bmp);
    o->tex->bmp = clone_bitmap_headless(o->tex->pristine);
    if (o->tex->bmp) al_convert_mask_to_alpha(o->tex->bmp, o->maskColor);
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
// the old shared bitmap (faithful to CoolBasic, which mirrors into a new target).
extern "C" void cb_rt_mirror_object(CbObject* o, int32_t dir) {
    if (!o || dir < 0 || dir > 2 || o->isFloor || o->emitter) return;
    if (!o->tex || !o->tex->pristine) return;
    ALLEGRO_BITMAP* srcp = o->tex->pristine;
    int w = al_get_bitmap_width(srcp);
    int h = al_get_bitmap_height(srcp);
    int flip = ALLEGRO_FLIP_HORIZONTAL;
    if (dir == 1) flip = ALLEGRO_FLIP_VERTICAL;
    else if (dir == 2) flip = ALLEGRO_FLIP_HORIZONTAL | ALLEGRO_FLIP_VERTICAL;

    // Flip the pristine into a new bitmap, then re-derive the masked copy from it,
    // so a later MaskObject still re-keys correctly. The flip blit uses a verbatim
    // copy blender (ONE,ZERO) so source alpha is preserved exactly rather than
    // composited/premultiplied by the global alpha blender; restore it after.
    ALLEGRO_BITMAP* dst = create_bitmap_headless(w, h);
    if (!dst) return;
    ALLEGRO_BITMAP* prev = al_get_target_bitmap();
    al_set_target_bitmap(dst);
    al_clear_to_color(al_map_rgba(0, 0, 0, 0));
    al_set_blender(ALLEGRO_ADD, ALLEGRO_ONE, ALLEGRO_ZERO);
    al_draw_bitmap(srcp, 0, 0, flip);
    cb::gfx::apply_alpha_blender();
    if (prev) al_set_target_bitmap(prev);

    auto holder = std::make_shared<CbTexture>();
    holder->pristine = dst;
    holder->bmp = clone_bitmap_headless(dst);
    if (holder->bmp) al_convert_mask_to_alpha(holder->bmp, o->maskColor);
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
// tick by the game loop, auto-deleting at 0).
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

// ─── Collision ─────────────────────────────────────────
//
// SetupCollision registers a persistent check; the actual geometry runs once per
// update tick in run_collision_checks (driven by the game loop). The
// pure overlap/resolution math lives in cb_collision_data.h; the map-grid tile
// loops (Rect/CircleMap) are here because they walk the active tilemap. Mode 0
// (report) records the contact but does NOT move the object; modes 1/2 (stop/
// slide) apply the resolved position via positionObject (faithful to CoolBasic).

namespace {

// Validate + register a check. Invalid checks are simply not pushed (CoolBasic
// nulls them at setup). Legal pairings: Box+Box, Circle+Circle, Box+Map,
// Circle+Map. Stop(1) handling is circle-only. (Box↔Circle object pairs are
// rejected here — CoolBasic's CircleRect/RectCircle tests are dead no-ops, so
// such pairs never collide; replicated deliberately.)
void register_collision(CbObject* a, int typeA, CbObject* b, bool bIsMap, int typeB,
                        int handling) {
    if (!a) return;
    if (a->emitter) return;            // emitters never collide (real CB)
    if (b && b->emitter) return;
    if (typeA != 1 && typeA != 2) return;  // colliding type must be Box or Circle
    if (bIsMap) {
        if (typeB != 4) return;  // the Map overload is map-collision only
    } else {
        if (!b) return;                              // object-object needs a target
        if (typeB == 1) { if (typeA != 1) return; }  // Box target ⇒ Box collider
        else if (typeB == 2) { if (typeA != 2) return; }  // Circle ⇒ Circle
        else return;                                 // Map/Pixel invalid for a pair
    }
    if (handling == 1) { if (typeA != 2) return; }   // Stop is circle-only
    else if (handling != 0 && handling != 2) return;
    collision_checks.push_back(
        CbCollisionCheck{a, b, bIsMap, typeA, typeB, handling, a->posX, a->posY});
}

void set_object_range(CbObject* o, double r1, double r2) {
    if (!o) return;
    if (r2 < 0.001) r2 = r1;  // omitted / ~0 second range mirrors the first
    o->range1 = r1;
    o->range2 = r2;
}

// ObjectsOverlap one-shot test (no registration). type 1=box, 2=circle, 3=pixel
// (pixel not implemented → 0, matching CoolBasic's error path). Box uses the
// centred AABB (range1×range2); circle uses range1/2 as the radius.
int32_t objects_overlap_impl(const CbObject* a, const CbObject* b, int32_t type) {
    if (!a || !b) return 0;
    if (a->emitter || b->emitter) return 0;  // emitters never collide
    if (type < 1 || type > 3) return 0;
    if (type == 1) {
        double w1 = a->range1, h1 = a->range2, w2 = b->range1, h2 = b->range2;
        return rect_overlap(a->posX - w1 / 2.0, a->posY + h1 / 2.0, w1, h1,
                            b->posX - w2 / 2.0, b->posY + h2 / 2.0, w2, h2)
                   ? 1
                   : 0;
    }
    if (type == 2) {
        return cb_circle_circle_overlap(a->posX, a->posY, a->range1 / 2.0, b->posX,
                                        b->posY, b->range1 / 2.0)
                   ? 1
                   : 0;
    }
    return 0;  // type 3 (pixel): not implemented
}

// ─── Per-pair test helpers (one update tick) ────────────────────────────

void box_box_test(CbCollisionCheck& c) {
    CbObject* a = c.a;
    CbObject* b = c.b;
    CbBoxResolve r = cb_box_box_resolve(a->posX, a->posY, c.safeY, a->range1,
                                        a->range2, b->posX, b->posY, b->range1,
                                        b->range2);
    for (int i = 0; i < r.hitCount; ++i) {
        a->collisions.push_back({b, r.hits[i].angle, r.hits[i].x, r.hits[i].y});
    }
    c.safeX = r.objX;
    c.safeY = r.objY;
    if (c.handling != 0) {
        a->posX = r.objX;
        a->posY = r.objY;
    }
}

void circle_circle_test(CbCollisionCheck& c) {
    CbObject* a = c.a;
    CbObject* b = c.b;
    CbCircleResolve r = cb_circle_circle_resolve(a->posX, a->posY, c.safeX, c.safeY,
                                                 a->range1 / 2.0, b->posX, b->posY,
                                                 b->range1 / 2.0, c.handling == 1);
    if (r.hitCount) a->collisions.push_back({b, r.hit.angle, r.hit.x, r.hit.y});
    c.safeX = r.objX;
    c.safeY = r.objY;
    if (c.handling != 0) {
        a->posX = r.objX;
        a->posY = r.objY;
    }
}

// Box-vs-tilemap. Two axis passes over the tiles
// around the object; fixed cardinal contact normals (top 270 / right 180 /
// bottom 90 / left 0). The map-wall "other" is Null (a Map is not an Object).
void rect_map_test(CbCollisionCheck& c) {
    const CbMapData* m = cb::map::active_data();
    if (!m || m->tileWidth <= 0 || m->tileHeight <= 0) return;
    CbObject* a = c.a;
    bool collided[4] = {false, false, false, false};
    double tileWidth = m->tileWidth, tileHeight = m->tileHeight;
    double mapSizeX = (double)m->mapWidth * m->tileWidth;
    double mapSizeY = (double)m->mapHeight * m->tileHeight;
    double mapX = m->posX, mapY = m->posY;
    double objX = a->posX, objY = a->posY;
    double objWidth = a->range1, objHeight = a->range2;
    int checkTilesX = (int)std::ceil(objWidth / tileWidth);
    int checkTilesY = (int)std::ceil(objHeight / tileHeight);
    int32_t startTileX = (int32_t)((objX - mapX + mapSizeX / 2.0) / tileWidth) - checkTilesX;
    int32_t startTileY = (int32_t)((-objY + mapY + mapSizeY / 2.0) / tileHeight) - checkTilesY;
    const double eps = 1e-5;

    // X-directional pass (uses the stored safeY).
    for (int32_t tileX = startTileX; tileX <= startTileX + checkTilesX * 2; ++tileX) {
        for (int32_t tileY = startTileY; tileY <= startTileY + checkTilesY * 2; ++tileY) {
            if (!cb_map_get_hit(*m, tileX, tileY)) continue;
            double x = tileX * tileWidth - mapSizeX / 2.0 + mapX;
            double y = mapSizeY / 2.0 - tileY * tileHeight + mapY;
            if (rect_overlap(objX - objWidth / 2.0, c.safeY + objHeight / 2.0, objWidth,
                             objHeight, x, y, tileWidth, tileHeight)) {
                objX = c.safeX;
                if (tileX < startTileX + checkTilesX) {
                    collided[3] = true;
                    objX = x + tileWidth + eps + objWidth / 2.0;
                } else if (tileX > startTileX + checkTilesX) {
                    collided[1] = true;
                    objX = x - eps - objWidth / 2.0;
                }
            }
        }
    }

    // Y-directional pass (uses the freshly-resolved objX).
    for (int32_t tileX = startTileX; tileX <= startTileX + checkTilesX * 2; ++tileX) {
        for (int32_t tileY = startTileY; tileY <= startTileY + checkTilesY * 2; ++tileY) {
            if (!cb_map_get_hit(*m, tileX, tileY)) continue;
            double x = tileX * tileWidth - mapSizeX / 2.0 + mapX;
            double y = mapSizeY / 2.0 - tileY * tileHeight + mapY;
            if (rect_overlap(objX - objWidth / 2.0, objY + objHeight / 2.0, objWidth,
                             objHeight, x, y, tileWidth, tileHeight)) {
                objY = c.safeY;
                if (tileY < startTileY + checkTilesY) {
                    collided[0] = true;
                    objY = y - tileHeight - eps - objHeight / 2.0;
                } else if (tileY > startTileY + checkTilesY) {
                    collided[2] = true;
                    objY = y + eps + objHeight / 2.0;
                }
            }
        }
    }

    c.safeX = objX;
    c.safeY = objY;
    if (c.handling != 0) {
        a->posX = objX;
        a->posY = objY;
    }
    if (collided[0]) a->collisions.push_back({nullptr, 270.0, objX, objY + objHeight / 2.0 + 1.0});
    if (collided[1]) a->collisions.push_back({nullptr, 180.0, objX + objWidth / 2.0 + 1.0, objY});
    if (collided[2]) a->collisions.push_back({nullptr, 90.0, objX, objY - objHeight / 2.0 - 1.0});
    if (collided[3]) a->collisions.push_back({nullptr, 0.0, objX - objWidth / 2.0 - 1.0, objY});
}

// Circle-vs-tilemap — the hardest test: per-axis
// circle-rect tile probing with edge-vs-corner disambiguation (a neighbour-tile
// lookup decides rect-style flush push-out vs corner push-out). A `done` flag
// stops each pass at the first resolved tile.
void circle_map_test(CbCollisionCheck& c) {
    const CbMapData* m = cb::map::active_data();
    if (!m || m->tileWidth <= 0 || m->tileHeight <= 0) return;
    CbObject* a = c.a;
    bool collided[4] = {false, false, false, false};
    double colX[4] = {0.0, 0.0, 0.0, 0.0};
    double colY[4] = {0.0, 0.0, 0.0, 0.0};
    double tileWidth = m->tileWidth, tileHeight = m->tileHeight;
    double mapSizeX = (double)m->mapWidth * m->tileWidth;
    double mapSizeY = (double)m->mapHeight * m->tileHeight;
    double mapX = m->posX, mapY = m->posY;
    double objX = a->posX, objY = a->posY;
    double objR = a->range1 / 2.0;
    int checkTilesX = (int)std::ceil(objR / tileWidth);
    int checkTilesY = (int)std::ceil(objR / tileHeight);
    int32_t startTileX = (int32_t)((objX - mapX + mapSizeX / 2.0) / tileWidth) - checkTilesX;
    int32_t startTileY = (int32_t)((-objY + mapY + mapSizeY / 2.0) / tileHeight) - checkTilesY;
    const double eps = 1e-5;

    // X-resolution pass.
    bool done = false;
    for (int32_t tileY = startTileY; tileY <= startTileY + checkTilesY * 2 && !done; ++tileY) {
        for (int32_t tileX = startTileX; tileX <= startTileX + checkTilesX * 2; ++tileX) {
            if (!cb_map_get_hit(*m, tileX, tileY)) continue;
            double x = tileX * tileWidth - mapSizeX / 2.0 + mapX;
            double y = mapSizeY / 2.0 - tileY * tileHeight + mapY;
            double centerY = y - tileHeight / 2.0;
            if (!cb_circle_rect_overlap(objX, c.safeY + tileHeight, objR, x, y, tileWidth,
                                        tileHeight)) {
                continue;
            }
            bool above = centerY > c.safeY;
            double cornerY = above ? centerY - tileHeight / 2.0 : centerY + tileHeight / 2.0;
            bool rectStyle = above ? (cb_map_get_hit(*m, tileX, tileY + 1) || cornerY < c.safeY)
                                   : (cb_map_get_hit(*m, tileX, tileY - 1) || cornerY > c.safeY);
            if (rectStyle) {
                if (tileX < startTileX + checkTilesX) {
                    collided[3] = true;
                    objX = x + tileWidth + eps + objR;
                    colX[3] = objX - objR;
                    colY[3] = objY;
                } else if (tileX > startTileX + checkTilesX) {
                    collided[1] = true;
                    objX = x - eps - objR;
                    colX[1] = objX + objR;
                    colY[1] = objY;
                }
            } else {
                double cornerX = 0.0;
                bool isCornerSet = false;
                if (tileX < startTileX + checkTilesX) {
                    cornerX = x + tileWidth;
                    isCornerSet = true;
                    collided[3] = true;
                    colX[3] = cornerX;
                    colY[3] = cornerY;
                } else if (tileX > startTileX + checkTilesX) {
                    cornerX = x;
                    isCornerSet = true;
                    collided[1] = true;
                    colX[1] = cornerX;
                    colY[1] = cornerY;
                }
                if (isCornerSet) {
                    double rad = std::atan2(cornerY - c.safeY, cornerX - objX);
                    objX = cornerX - std::cos(rad) * (objR + eps);
                }
            }
            done = true;
            break;
        }
    }

    // Y-resolution pass.
    done = false;
    for (int32_t tileY = startTileY; tileY <= startTileY + checkTilesY * 2 && !done; ++tileY) {
        for (int32_t tileX = startTileX; tileX <= startTileX + checkTilesX * 2; ++tileX) {
            if (!cb_map_get_hit(*m, tileX, tileY)) continue;
            double x = tileX * tileWidth - mapSizeX / 2.0 + mapX;
            double y = mapSizeY / 2.0 - tileY * tileHeight + mapY;
            double centerX = x + tileWidth / 2.0;
            if (!cb_circle_rect_overlap(objX, objY + tileHeight, objR, x, y, tileWidth,
                                        tileHeight)) {
                continue;
            }
            bool rightward = centerX > objX;
            double cornerX = rightward ? x : x + tileWidth;
            bool rectStyle = rightward ? (cb_map_get_hit(*m, tileX - 1, tileY) || cornerX < objX)
                                       : (cb_map_get_hit(*m, tileX + 1, tileY) || cornerX > objX);
            if (rectStyle) {
                if (tileY < startTileY + checkTilesY) {
                    collided[0] = true;
                    objY = y - tileHeight - eps - objR;
                    colX[0] = objX;
                    colY[0] = objY + objR;
                } else if (tileY > startTileY + checkTilesY) {
                    collided[2] = true;
                    objY = y + eps + objR;
                    colX[2] = objX;
                    colY[2] = objY - objR;
                }
            } else {
                double cornerY = 0.0;
                bool isCornerSet = false;
                if (tileY < startTileY + checkTilesY) {
                    cornerY = y - tileHeight;
                    isCornerSet = true;
                    collided[0] = true;
                    colX[0] = cornerX;
                    colY[0] = cornerY;
                } else if (tileY > startTileY + checkTilesY) {
                    cornerY = y;
                    isCornerSet = true;
                    collided[2] = true;
                    colX[2] = cornerX;
                    colY[2] = cornerY;
                }
                if (isCornerSet) {
                    double rad = std::atan2(cornerY - objY, cornerX - objX);
                    objY = cornerY - std::sin(rad) * (objR + eps);
                }
            }
            done = true;
            break;
        }
    }

    c.safeX = objX;
    c.safeY = objY;
    if (c.handling != 0) {
        a->posX = objX;
        a->posY = objY;
    }
    if (collided[0]) a->collisions.push_back({nullptr, 270.0, colX[0], colY[0]});
    if (collided[1]) a->collisions.push_back({nullptr, 180.0, colX[1], colY[1]});
    if (collided[2]) a->collisions.push_back({nullptr, 90.0, colX[2], colY[2]});
    if (collided[3]) a->collisions.push_back({nullptr, 0.0, colX[3], colY[3]});
}

}  // namespace

// SetupCollision(objA, objB, typeA, typeB, handling): register a persistent
// object-object check. Re-tested every update tick until ClearCollisions or an
// object is deleted.
extern "C" void cb_rt_setup_collision(CbObject* obj_a, CbObject* obj_b, int32_t type_a,
                                      int32_t type_b, int32_t handling) {
    register_collision(obj_a, type_a, obj_b, false, type_b, handling);
}

// SetupCollision(objA, map, typeA, typeB, handling): the type-4 map-collision
// overload. The Map handle is accepted for type-honesty but ignored — there is a
// single active map singleton (like EditMap's popped-but-ignored map arg).
extern "C" void cb_rt_setup_collision_map(CbObject* obj_a, CbMap* map, int32_t type_a,
                                          int32_t type_b, int32_t handling) {
    (void)map;
    register_collision(obj_a, type_a, nullptr, true, type_b, handling);
}

// ObjectRange(obj, range1[, range2]): collision bounds. Box uses width=range1,
// height=range2; circle uses diameter=range1. An omitted/≈0 range2 mirrors range1.
extern "C" void cb_rt_object_range(CbObject* o, double range1) {
    set_object_range(o, range1, 0.0);
}
extern "C" void cb_rt_object_range3(CbObject* o, double range1, double range2) {
    set_object_range(o, range1, range2);
}

// ResetObjectCollision(obj): clear this frame's recorded collisions AND skip the
// object for the current tick (checkCollisions=false until the next tick resets it).
extern "C" void cb_rt_reset_object_collision(CbObject* o) {
    if (!o) return;
    o->checkCollisions = false;
    o->collisions.clear();
}

// ClearCollisions(): remove every registered check.
extern "C" void cb_rt_clear_collisions(void) { collision_checks.clear(); }

// CountCollisions(obj): number of collisions recorded for the object this frame.
extern "C" int32_t cb_rt_count_collisions(const CbObject* o) {
    return o ? (int32_t)o->collisions.size() : 0;
}

// GetCollision(obj, index): the other object of the 1-based-indexed collision, or
// Null (out of range, or a map-wall hit — a Map is not an Object).
extern "C" CbObject* cb_rt_get_collision(const CbObject* o, int32_t index) {
    if (!o || index < 1 || (std::size_t)index > o->collisions.size()) return nullptr;
    return o->collisions[(std::size_t)index - 1].other;
}

extern "C" double cb_rt_collision_x(const CbObject* o, int32_t index) {
    if (!o || index < 1 || (std::size_t)index > o->collisions.size()) return 0.0;
    return o->collisions[(std::size_t)index - 1].x;
}
extern "C" double cb_rt_collision_y(const CbObject* o, int32_t index) {
    if (!o || index < 1 || (std::size_t)index > o->collisions.size()) return 0.0;
    return o->collisions[(std::size_t)index - 1].y;
}
extern "C" double cb_rt_collision_angle(const CbObject* o, int32_t index) {
    if (!o || index < 1 || (std::size_t)index > o->collisions.size()) return 0.0;
    return o->collisions[(std::size_t)index - 1].angle;
}

// ObjectsOverlap(a, b[, type]): one-shot overlap test (default box). type 1=box,
// 2=circle, 3=pixel (not implemented → 0).
extern "C" int32_t cb_rt_objects_overlap(const CbObject* a, const CbObject* b) {
    return objects_overlap_impl(a, b, 1);
}
extern "C" int32_t cb_rt_objects_overlap3(const CbObject* a, const CbObject* b,
                                          int32_t type) {
    return objects_overlap_impl(a, b, type);
}

// Re-test every registered collision check (one update tick). Object collision
// lists are wiped per-object by the update tick before this runs; here we only
// append contacts and apply stop/slide position corrections. Glue for the
// game loop (cb_objects_update_all); see cb_object.h.
void run_collision_checks(void) {
    for (CbCollisionCheck& c : collision_checks) {
        CbObject* a = c.a;
        if (!a || !a->checkCollisions || !a->visible) continue;
        if (!c.bIsMap && (!c.b || !c.b->visible)) continue;
        if (c.typeA == 1) {
            if (c.bIsMap) rect_map_test(c);
            else box_box_test(c);  // typeB is Box (guaranteed by register_collision)
        } else if (c.typeA == 2) {
            if (c.bIsMap) circle_map_test(c);
            else circle_circle_test(c);  // typeB is Circle
        }
    }
}

// ─── Picking & line of sight ───────────────────────────

// ObjectPickable(obj, style): 0 = not pickable (removed); 1/2/3 = box/circle/
// pixel. Adds/removes the object from the pickable set.
extern "C" void cb_rt_object_pickable(CbObject* o, int32_t style) {
    if (!o) return;
    if (o->emitter) return;  // emitters are never pickable (real CB)
    if (style == 0) {
        o->pickStyle = 0;
        erase_from(pickable_objects, o);
        return;
    }
    if (style == 1 || style == 2 || style == 3) {
        o->pickStyle = style;
        if (std::find(pickable_objects.begin(), pickable_objects.end(), o) ==
            pickable_objects.end()) {
            pickable_objects.push_back(o);
        }
    }
}

// ObjectPick(picker): raycast from the picker along its facing angle and keep the
// nearest pickable hit. Sets PickedObject/X/Y/Angle.
extern "C" void cb_rt_object_pick(CbObject* picker) {
    if (!picker) return;
    last_picked = nullptr;
    last_picked_x = 0.0;
    last_picked_y = 0.0;
    last_picked_angle = 0.0;
    double best_dsq = -1.0;
    double best_x = 0.0, best_y = 0.0;
    for (CbObject* o : pickable_objects) {
        if (o == picker) continue;
        double hx = 0.0, hy = 0.0;
        bool hit = false;
        if (o->pickStyle == 1) {
            hit = cb_box_ray_cast(picker->posX, picker->posY, picker->angle, o->posX,
                                  o->posY, o->range1, o->range2, hx, hy);
        } else if (o->pickStyle == 2) {
            hit = cb_circle_ray_cast(picker->posX, picker->posY, picker->angle, o->posX,
                                     o->posY, o->range1, hx, hy);
        }
        // pickStyle 3 (pixel): raycast returns false (as in CoolBasic).
        if (!hit) continue;
        double dsq = (picker->posX - hx) * (picker->posX - hx) +
                     (picker->posY - hy) * (picker->posY - hy);
        if (best_dsq < -0.5 || dsq < best_dsq) {
            best_dsq = dsq;
            last_picked = o;
            last_picked_x = hx;
            last_picked_y = hy;
            best_x = hx;
            best_y = hy;
        }
    }
    // PickedAngle is the angle from the picker to the PICKED hit point,
    // in degrees.
    if (last_picked) {
        last_picked_angle = cb_object_angle2(picker->posX, picker->posY, best_x, best_y);
    }
}

// PixelPick(picker[, accuracy]): a registered no-op stub (a stub in CoolBasic too).
extern "C" void cb_rt_pixel_pick(CbObject* picker) { (void)picker; }
extern "C" void cb_rt_pixel_pick_acc(CbObject* picker, int32_t accuracy) {
    (void)picker;
    (void)accuracy;
}

extern "C" CbObject* cb_rt_picked_object(void) { return last_picked; }
extern "C" double cb_rt_picked_x(void) { return last_picked_x; }
extern "C" double cb_rt_picked_y(void) { return last_picked_y; }
extern "C" double cb_rt_picked_angle(void) { return last_picked_angle; }

// ObjectSight(a, b): 1 if a clear line (no map walls) runs between the two
// objects, else 0. With no tilemap loaded there are no walls → 1 (this also
// guards against a null-deref when no map exists).
extern "C" int32_t cb_rt_object_sight(const CbObject* a, const CbObject* b) {
    if (!a || !b) return 0;
    const CbMapData* m = cb::map::active_data();
    if (!m) return 1;
    double x1 = a->posX, y1 = a->posY, x2 = b->posX, y2 = b->posY;
    cb_map_world_to_map(*m, x1, y1);
    cb_map_world_to_map(*m, x2, y2);
    return cb_map_ray_cast(*m, x1, y1, x2, y2) ? 0 : 1;
}

// CameraPick helper: pick the first pickable object whose shape contains the
// world point. Resets PickedObject first, sets only PickedObject (no X/Y/Angle —
// faithful to CoolBasic). Declared in cb_object.h so cb_camera.cpp's CameraPick
// can call it after screen→world.
void pick_at(double wx, double wy) {
    last_picked = nullptr;
    for (CbObject* o : pickable_objects) {
        bool hit = false;
        if (o->pickStyle == 1) {
            hit = cb_can_pick_box(o->posX, o->posY, o->range1, o->range2, wx, wy);
        } else if (o->pickStyle == 2) {
            hit = cb_can_pick_circle(o->posX, o->posY, o->range1, wx, wy);
        }
        if (hit) {
            last_picked = o;
            break;
        }
    }
}

// ScreenPositionObject(obj, sx, sy): move the object to the world point under a
// screen coordinate (screen→world through the camera, then position).
extern "C" void cb_rt_screen_position_object(CbObject* o, double sx, double sy) {
    if (!o) return;
    cb::camera::screen_to_world(&sx, &sy);
    o->posX = sx;
    o->posY = sy;
}

// ─── Particle emitters ─────────────────────────────────────────
//
// A CoolBasic "Effects" emitter IS an Object. MakeEmitter returns the `Object`
// handle, so every object
// command above works on it unchanged; the three Particle* commands configure
// the emitter payload and trap on a non-emitter handle. Emitters render their
// particles (render_particles), step them each tick (update_emitter / the rogue
// drain), and are excluded from picking and collision (real CB).

// MakeEmitter(image, lifeTime): create an emitter at (0,0). `lifeTime` is the
// per-PARTICLE life in DrawScreen frames (not the emitter's own life). The
// particle texture is copied from the image into the object's own masked holder
// (so a later DeleteImage of the source is safe), and the strip's frame geometry
// is captured for animated particles.
extern "C" CbObject* cb_rt_make_emitter(const CbImage* image, int32_t life_time) {
    CbObject* o = new CbObject(false);
    o->emitter = std::make_unique<cb::particle::CbEmitterState>();
    o->emitter->particleLifeTime = life_time;

    ALLEGRO_BITMAP* srcPristine = cb::gfx::image_pristine(image);
    if (srcPristine) {
        ALLEGRO_BITMAP* pristine = clone_bitmap_headless(srcPristine);
        if (pristine) {
            set_object_texture(o, pristine);  // pristine + derived masked copy
            o->sizeX = al_get_bitmap_width(pristine);
            o->sizeY = al_get_bitmap_height(pristine);
        }
    }
    // Frame geometry for an animated particle strip (all 0 for a plain image).
    int32_t fw = 0, fh = 0, fc = 0;
    cb::gfx::image_frame_info(image, &fw, &fh, &fc);
    o->frameWidth = fw;
    o->frameHeight = fh;
    o->maxFrames = fc;
    o->painted = true;
    register_object(o);
    return o;
}

// ParticleMovement(emitter, speed, gravity[, acceleration]): launch speed (px),
// gravity (positive pulls particles down), and an optional per-frame velocity
// scale (default 1.0 = constant; <1 decelerates, >1 accelerates). The 3-arg form
// resets acceleration to the 1.0 default; the 4-arg form is its own overload.
extern "C" void cb_rt_particle_movement(CbObject* o, double speed, double gravity) {
    if (auto* e = require_emitter(o, "ParticleMovement")) {
        e->speed = speed;
        e->gravity = gravity;
        e->acceleration = 1.0;
    }
}
extern "C" void cb_rt_particle_movement_acc(CbObject* o, double speed, double gravity,
                                            double accel) {
    if (auto* e = require_emitter(o, "ParticleMovement")) {
        e->speed = speed;
        e->gravity = gravity;
        e->acceleration = accel;
    }
}

// ParticleEmission(emitter, density, count, spread): emission interval in frames
// (smaller = denser), particles per emission, and the ± spread sector in degrees
// (0..180; 180 = all directions, 0 = a tight stream along the facing direction).
extern "C" void cb_rt_particle_emission(CbObject* o, int32_t density, int32_t count,
                                        int32_t spread) {
    if (auto* e = require_emitter(o, "ParticleEmission")) {
        e->density = (double)density;
        e->count = count;
        e->spread = (double)spread;
    }
}

// ParticleAnimation(emitter, frames): animate the (LoadAnimImage) particle image
// as a `frames`-long strip, played once over each particle's life. Clamped to the
// strip's real frame count so an over-long value can't slice past the sheet
// (classic CB crashes; we clamp).
extern "C" void cb_rt_particle_animation(CbObject* o, int32_t frames) {
    if (auto* e = require_emitter(o, "ParticleAnimation")) {
        if (frames < 0) frames = 0;
        if (o->maxFrames > 0 && frames > o->maxFrames) frames = o->maxFrames;
        e->frameCount = frames;
    }
}

// ─── Game-loop update ──────────────────────────────────

namespace {

// One animation step for an object (the per-object anim update), via the pure
// cb_object_anim_advance over a CbAnimState view of the object's fields.
void advance_object_anim(CbObject* o) {
    CbAnimState s;
    s.current_frame = o->currentFrame;
    s.anim_start_frame = o->animStartFrame;
    s.anim_ending_frame = o->animEndingFrame;
    s.anim_speed = o->animSpeed;
    s.anim_looping = o->animLooping;
    s.playing = o->playing;
    cb_object_anim_advance(s);
    o->currentFrame = s.current_frame;
    o->playing = s.playing;
}

// One emitter tick: bump the spawn counter, integrate+cull existing
// particles, then spawn any emissions due. Spawn randomness draws from the shared
// CB RNG (cb_rt_rnd_max → [0,1)), so Randomize
// affects particles too. ObjectLife decrement/auto-delete is handled by the
// caller (update_all), as for any object.
void update_emitter(CbObject* o) {
    cb::particle::CbEmitterState& e = *o->emitter;
    e.spawnCounter += 1.0;
    cb::particle::integrate_and_cull(e);
    if (e.stop) return;  // defensive; live emitters are never stopped
    cb::particle::spawn_due(e, o->posX, o->posY, o->angle,
                            [] { return cb_rt_rnd_max(1.0); });
}

// Drain deleted emitters: each rogue keeps integrating its particles
// (no spawning) until empty, then leaves its draw chain and is freed. Called once
// per update tick. A rogue created during this same tick's main loop integrates
// once more here (a 1-frame cosmetic nicety, not a correctness issue).
void update_rogue_emitters(void) {
    for (std::size_t i = 0; i < rogue_emitters.size();) {
        CbObject* o = rogue_emitters[i];
        cb::particle::integrate_and_cull(*o->emitter);
        if (o->emitter->particles.empty()) {
            erase_from(floor_objects, o);
            erase_from(regular_objects, o);
            rogue_emitters.erase(rogue_emitters.begin() + (std::ptrdiff_t)i);
            delete o;
        } else {
            ++i;
        }
    }
}

}  // namespace

// The per-frame object update: per object advance animation, decrement ObjectLife
// (auto-delete at 0), and wipe last tick's collisions; then advance map tile
// animation, run every collision check, and re-arm collision checking on all
// survivors. See cb_object.h.
void update_all(void) {
    // Snapshot the live set: auto-delete (life) mutates the registries mid-loop.
    std::vector<CbObject*> snapshot = live_objects;
    for (CbObject* o : snapshot) {
        if (o->emitter) {
            update_emitter(o);  // spawn + step particles
        } else {
            advance_object_anim(o);
        }
        if (o->usingLife && cb_object_life_tick(o->life)) {
            cb_rt_delete_object(o);  // life hit 0 → auto-delete (an emitter with
                                     // live particles defers to the rogue drain)
            continue;
        }
        o->collisions.clear();  // eraseCollisions: wipe last tick's contacts
    }
    update_rogue_emitters();      // drain + free deleted emitters
    cb::map::tick_animation();    // advance animated map tiles
    run_collision_checks();  // re-test every registered check
    for (CbObject* o : live_objects) o->checkCollisions = true;  // re-arm
}

// ─── Render orchestrator (glue for cb_gfx.cpp; see cb_object.h) ──────────
//
// The object draw pass: one world-transform bracket over map background (layer 0)
// → floor objects → regular objects → map foreground (layer 1). A no-op when
// there is nothing to draw. The caller (do_draw_screen) has already set the
// backbuffer as the target.
void render_all(void) {
    if (!cb::map::active() && floor_objects.empty() && regular_objects.empty()) return;
    if (!al_get_target_bitmap()) return;

    al_use_transform(cb::camera::world_transform());
    cb::map::render_layer(0);
    for (CbObject* o : floor_objects) render_object(o);
    for (CbObject* o : regular_objects) render_object(o);
    cb::map::render_layer(1);

    ALLEGRO_TRANSFORM id;
    al_identity_transform(&id);
    al_use_transform(&id);
}

}  // namespace cb::object
