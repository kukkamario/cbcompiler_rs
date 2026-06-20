// CoolBasic tilemap runtime (FD-036 Phase 3).
//
// One active tilemap (cbEnchanted keeps a single `CBMap *tileMap`): a tile grid
// with four layers (0=background, 1=foreground, 2=collision, 3=data) plus a
// tileset bitmap. Loaded from a .til binary + a tileset image, or made empty.
// The pure data — the .til parser, the grid accessors, and the world<->tile
// coordinate math — lives in the Allegro-free cb_map_data.h so it unit-tests
// without a display; this TU adds the tileset bitmap, the catalog entry points,
// and the camera-space render pass.
//
// ABI (see cb_runtime.h / the catalog DSL): CB Float args arrive as `double`,
// Int as `int32_t`; strings as `const CbString*`; the `Map` opaque handle is a
// `CbMap*`. LoadMap/MakeMap return the active map (Null on failure).
//
// Rendering: in cbEnchanted the map draws inside the object draw order
// (drawObjects, FD-036 Phase 4/5). With no object subsystem yet, cb_gfx.cpp
// calls cb_map_render_active() from DrawScreen as a standalone pass; Phase 5
// relocates it into drawObjects. The actual blit needs a display, so it is
// exercised by the visual/manual smoke; the coordinate math is unit-tested in
// runtime/tests/test_map.cpp and the data path by the graphics-gated fixture.

#include "cb_map.h"
#include "cb_map_data.h"
#include "cb_camera.h"
#include "cb_runtime_func.h"

#include <allegro5/allegro.h>
#include <allegro5/allegro_image.h>

// Internal glue: the live bitmap behind an `Image` handle (defined in cb_gfx.cpp)
// — used by PaintObject(Map, Image). Forward-declared rather than widening a
// public header (mirrors cb_object.cpp / cb_input.cpp's cb_gfx glue).
extern "C" ALLEGRO_BITMAP* cb_gfx_image_bitmap(const CbImage* img);

#include <fstream>
#include <string>
#include <utility>
#include <vector>

// ─── Opaque Map handle ──────────────────────────────────────────────────
//
// The CB-visible `Map` type (tag 14). Wraps the parsed grid + the tileset
// bitmap. `painted` mirrors cbEnchanted: a MakeMap'd map has no tileset and is
// not drawn until one is supplied; a LoadMap'd map is painted. `layerShowing`
// gates the background (0) and foreground (1) draws (SetMap).
struct CbMap {
    CbMapData data;
    ALLEGRO_BITMAP* texture = nullptr;
    bool painted = false;
    bool visible = true;
    uint8_t layerShowing[2] = {1, 1};
    // Per-tile animation rate, set by PlayObject(Map). cbEnchanted's map is a
    // CBObject whose inherited animSpeed is the tile formula's divisor; it
    // starts at 0, so tiles do not advance until PlayObject sets a positive
    // speed. 0 = stopped.
    float animSpeed = 0.0f;
};

namespace {

// The single active tilemap (cbEnchanted's `tileMap`). LoadMap/MakeMap free and
// replace it. Process-global is safe: the VM is single-threaded (FD-036).
CbMap* active_map = nullptr;

// Wall-clock time (al_get_time seconds) of the last tile-animation tick; -1 means
// unseeded. Re-seeded whenever the map stops so a resume doesn't advance by the
// whole pause. See cb_map_tick_animation.
double map_anim_last_time = -1.0;

void replace_active_map(CbMap* m) {
    if (active_map) {
        if (active_map->texture) al_destroy_bitmap(active_map->texture);
        delete active_map;
    }
    active_map = m;
}

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

bool read_file(const std::string& path, std::vector<uint8_t>& out) {
    std::ifstream f(path, std::ios::binary);
    if (!f) return false;
    f.seekg(0, std::ios::end);
    std::streamoff size = f.tellg();
    if (size <= 0) return false;
    f.seekg(0, std::ios::beg);
    out.resize((size_t)size);
    f.read(reinterpret_cast<char*>(out.data()), size);
    return static_cast<bool>(f);
}

// Loads the tileset bitmap and bakes the .til's mask colour to alpha (cbEnchanted
// loadTileset: `load(path, al_map_rgb(maskR, maskG, maskB))`). Mirrors the
// memory-bitmap fallback so it loads without a display.
ALLEGRO_BITMAP* load_tileset(const std::string& path, uint8_t r, uint8_t g, uint8_t b) {
    // Self-init the subsystems al_load_bitmap needs (idempotent — a no-op once
    // cb_gfx's ensure_init has run after any Screen/MakeImage call). The render
    // pass uses only core bitmap drawing, which needs no addon.
    if (!al_is_system_installed()) al_init();
    if (!al_is_image_addon_initialized()) al_init_image_addon();

    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* bmp = al_load_bitmap(path.c_str());
    al_set_new_bitmap_flags(prev_flags);
    if (!bmp) return nullptr;
    al_convert_mask_to_alpha(bmp, al_map_rgb(r, g, b));
    return bmp;
}

// Draws one layer (0=background, 1=foreground) under the already-set plain world
// transform. Each tile's anchor Y is flipped (cbEnchanted convertCoords) so the
// bitmap stays upright. Iterates the whole grid and lets Allegro clip off-screen
// tiles — the viewport cull is a deferred optimisation, pixel-identical here.
void draw_layer(int level) {
    CbMap* m = active_map;
    if (level < 0 || level > 1) return;
    if (m->layerShowing[level] < 1) return;
    const CbMapData& d = m->data;
    int32_t tileset_w = al_get_bitmap_width(m->texture);
    for (int32_t gy = 0; gy < d.mapHeight; ++gy) {
        for (int32_t gx = 0; gx < d.mapWidth; ++gx) {
            int32_t tile = cb_map_get(d, level, gx, gy);
            if (tile <= 0) continue;
            // Animated tiles advance through consecutive tileset ids;
            // currentFrame stays 0 until the Phase 5 game-loop update tick.
            int32_t draw_id = tile;
            if ((uint32_t)tile < d.currentFrame.size()) {
                draw_id = tile + (int32_t)d.currentFrame[tile];
            }
            int32_t sx = 0, sy = 0;
            if (!cb_map_tile_src(d, draw_id, tileset_w, sx, sy)) continue;
            double wx = 0.0, wy = 0.0;
            cb_map_tile_anchor(d, gx, gy, wx, wy);
            al_draw_bitmap_region(m->texture, (float)sx, (float)sy,
                                  (float)d.tileWidth, (float)d.tileHeight,
                                  (float)wx, (float)(-wy), 0);
        }
    }
}

void set_tile_impl(int32_t tile, int32_t length, int32_t slowness) {
    if (!active_map || tile < 0) return;
    CbMapData& d = active_map->data;
    if ((uint32_t)tile >= d.tileCount) {
        uint32_t new_count = (uint32_t)tile + 1;
        // Grow the anim arrays, defaulting new slots correctly. (cbEnchanted's
        // setTile has a realloc bug here: it writes the slowness default into
        // the *old* freed array, leaving the new slots uninitialised — fixed.)
        d.animLength.resize(new_count, 0);
        d.animSlowness.resize(new_count, 1);
        d.currentFrame.resize(new_count, 0.0f);
        d.tileCount = new_count;
    }
    d.animLength[tile] = length;
    d.animSlowness[tile] = slowness;
}

}  // namespace

// ─── Creation / destruction ─────────────────────────────────────────────

// LoadMap(mapPath, tilesetPath): parse the .til, load+mask the tileset, replace
// any existing map. Returns Null on any failure (FD-018 null-opaque precedent).
extern "C" CbMap* cb_rt_load_map(const CbString* map_path, const CbString* tileset_path) {
    std::vector<uint8_t> bytes;
    if (!read_file(read_cb_string(map_path), bytes)) return nullptr;

    CbMapData data;
    if (!cb_map_parse(bytes.data(), bytes.size(), data)) return nullptr;

    ALLEGRO_BITMAP* tex = load_tileset(read_cb_string(tileset_path),
                                       data.maskR, data.maskG, data.maskB);
    if (!tex) return nullptr;

    CbMap* m = new CbMap();
    m->data = std::move(data);
    m->texture = tex;
    m->painted = true;
    replace_active_map(m);
    return active_map;
}

// MakeMap(wTiles, hTiles, tileW, tileH): an empty map with no tileset (not
// painted, so it does not render until one is supplied — matches cbEnchanted).
extern "C" CbMap* cb_rt_make_map(int32_t w_tiles, int32_t h_tiles, int32_t tile_w,
                                 int32_t tile_h) {
    CbMap* m = new CbMap();
    cb_map_create(m->data, w_tiles, h_tiles, tile_w, tile_h);
    replace_active_map(m);
    return active_map;
}

// ─── Queries (operate on the single active map; 0 when none) ────────────

extern "C" int32_t cb_rt_map_width(void) {
    return active_map ? active_map->data.mapWidth : 0;
}

extern "C" int32_t cb_rt_map_height(void) {
    return active_map ? active_map->data.mapHeight : 0;
}

// GetMap(layer, x, y): tile id at world coordinates (0 outside / no map).
extern "C" int32_t cb_rt_get_map(int32_t layer, double x, double y) {
    if (!active_map) return 0;
    return cb_map_get_world(active_map->data, layer, x, y);
}

// GetMap2(layer, tx, ty): tile id at a 1-based grid position (0 outside / no map).
extern "C" int32_t cb_rt_get_map2(int32_t layer, int32_t tx, int32_t ty) {
    if (!active_map) return 0;
    return cb_map_get(active_map->data, layer, tx - 1, ty - 1);
}

// ─── Mutation ───────────────────────────────────────────────────────────

// EditMap(map, layer, tx, ty, tile): `map` is popped but ignored — the single
// active map is edited. 1-based grid; out-of-bounds ignored.
extern "C" void cb_rt_edit_map(CbMap* map_ignored, int32_t layer, int32_t tx,
                               int32_t ty, int32_t tile) {
    (void)map_ignored;
    if (!active_map) return;
    cb_map_edit(active_map->data, layer, tx - 1, ty - 1, tile);
}

// SetMap(backLayer, overLayer): toggle visibility of background (0) and
// foreground (1). 0 = hidden, nonzero = shown.
extern "C" void cb_rt_set_map(int32_t back_layer, int32_t over_layer) {
    if (!active_map) return;
    active_map->layerShowing[0] = (uint8_t)back_layer;
    active_map->layerShowing[1] = (uint8_t)over_layer;
}

// SetTile(tile, animLength): per-tile animation; slowness defaults to 1.
extern "C" void cb_rt_set_tile(int32_t tile, int32_t anim_length) {
    set_tile_impl(tile, anim_length, 1);
}

// SetTile(tile, animLength, animSlowness): the explicit-slowness form.
extern "C" void cb_rt_set_tile_slow(int32_t tile, int32_t anim_length,
                                    int32_t anim_slowness) {
    set_tile_impl(tile, anim_length, anim_slowness);
}

// ─── Appearance ─────────────────────────────────────────────────────────

// PaintObject(map, image): repaints the active tilemap's tileset with an image.
// The `map` handle is popped but ignored (single active map, like EditMap).
// cbEnchanted: maps can only be painted with an image (objectinterface.cpp:265).
extern "C" void cb_rt_paint_object_map(CbMap* map_ignored, const CbImage* img) {
    (void)map_ignored;
    if (!active_map) return;
    ALLEGRO_BITMAP* src = cb_gfx_image_bitmap(img);
    if (!src) return;

    int prev_flags = al_get_new_bitmap_flags();
    int flags = prev_flags;
    if (!al_get_current_display()) flags |= ALLEGRO_MEMORY_BITMAP;
    al_set_new_bitmap_flags(flags);
    ALLEGRO_BITMAP* clone = al_clone_bitmap(src);
    al_set_new_bitmap_flags(prev_flags);
    if (!clone) return;

    const CbMapData& d = active_map->data;
    al_convert_mask_to_alpha(clone, al_map_rgb(d.maskR, d.maskG, d.maskB));
    if (active_map->texture) al_destroy_bitmap(active_map->texture);
    active_map->texture = clone;
    active_map->painted = true;
}

// ─── Render pass (glue for the Phase-4 object orchestrator; see cb_map.h) ─

extern "C" int cb_map_active(void) { return active_map != nullptr ? 1 : 0; }

// FD-036 Phase 5: expose the active map's parsed grid for object map-collision
// (type 4) and ObjectSight. Null when no map is loaded — callers guard it (the
// faithful no-op, vs cbEnchanted's null-deref). The CbMapData carries the tile
// dims, the layer-2 collision grid, and the map's world position/centring.
extern "C" const CbMapData* cb_map_active_data(void) {
    return active_map ? &active_map->data : nullptr;
}

// FD-036: advance animated map tiles, faithful to cbEnchanted (cbmap.cpp:366-379).
// Time-based: currentFrame += elapsedSeconds / (slowness * animSpeed), where the
// elapsed time is the real wall-clock delta since the last tick (al_get_time) —
// so the animation is frame-rate independent and paces like cbEnchanted (with no
// FrameLimit the game loop runs unbounded, so a per-tick step would race away).
// A tile resets to frame 0 once (int)currentFrame *exceeds* animLength, so it
// cycles tile..tile+animLength — animLength+1 frames (`length` = "following tiles
// attached"; animLength==1 is a 2-frame tile). The render samples
// `tile + (int)currentFrame[tile]`. Runs only while playing (animSpeed > 0, set
// by PlayObject(Map)).
//
// (Supersedes the Phase-5 "deterministic frame-step": it was frame-rate-dependent
// and far too fast, and its wrap never advanced an animLength==1 tile. No headless
// test exercises tile animation — every fixture asset has animLength==0 — so the
// determinism it bought was moot.)
extern "C" void cb_map_tick_animation(void) {
    if (!active_map || active_map->animSpeed <= 0.0f) {
        map_anim_last_time = -1.0;  // stopped → re-seed the delta on resume
        return;
    }
    double now = al_get_time();
    if (map_anim_last_time < 0.0) {  // first tick since (re)start: seed, don't jump
        map_anim_last_time = now;
        return;
    }
    const float timestep = (float)(now - map_anim_last_time);
    map_anim_last_time = now;

    CbMapData& d = active_map->data;
    const float spd = active_map->animSpeed;
    for (uint32_t i = 0; i < d.tileCount; ++i) {
        if (i >= d.animLength.size() || d.animLength[i] <= 0) continue;
        int32_t slow = (i < d.animSlowness.size()) ? d.animSlowness[i] : 1;
        d.currentFrame[i] =
            cb_map_advance_frame(d.currentFrame[i], d.animLength[i], slow, spd, timestep);
    }
}

// ─── PlayObject(Map): start/stop tile animation ─────────────────────────────
//
// cbEnchanted's map is a CBObject, so PlayObject sets its inherited animSpeed
// (the per-tile formula's divisor) and marks it playing. The startFrame/
// continuous args do not apply to tile animation — each tile wraps by its own
// animLength — so only `speed` is used; endFrame == -1 stops (mirrors the
// object form). The Map first param disambiguates these from the Object
// PlayObject overloads (see cb_object.cpp); the same 1/3/4/5 arity family.
static void map_play(CbMap* m, int32_t end_f, double speed) {
    if (!m) return;
    m->animSpeed = (end_f == -1) ? 0.0f : (float)speed;
}
extern "C" void cb_rt_play_map(CbMap* m) { map_play(m, 0, 0.1); }
extern "C" void cb_rt_play_map3(CbMap* m, int32_t start_f, int32_t end_f) {
    (void)start_f;
    map_play(m, end_f, 0.1);
}
extern "C" void cb_rt_play_map4(CbMap* m, int32_t start_f, int32_t end_f, double speed) {
    (void)start_f;
    map_play(m, end_f, speed);
}
extern "C" void cb_rt_play_map5(CbMap* m, int32_t start_f, int32_t end_f, double speed,
                                int32_t continuous) {
    (void)start_f;
    (void)continuous;
    map_play(m, end_f, speed);
}

// Draws one layer under the world transform the caller (cb_objects_render_all)
// has already set — no transform bracket here, so the map composites in the
// object draw order (background before objects, foreground after).
extern "C" void cb_map_render_layer(int slot) {
    if (!active_map || !active_map->painted || !active_map->visible) return;
    if (!active_map->texture || !al_get_target_bitmap()) return;
    if (slot < 0 || slot > 1) return;
    draw_layer(slot);
}
