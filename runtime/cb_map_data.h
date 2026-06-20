#ifndef CB_MAP_DATA_H
#define CB_MAP_DATA_H

// Pure tile-map data + parsing for the tilemap subsystem (FD-036 Phase 3).
// Header-only and Allegro-free so the .til binary parser, the grid accessors,
// and the world<->tile coordinate math can be unit-tested without a display
// (mirrors cb_camera_math.h / test_camera.cpp). cb_map.cpp wraps a CbMapData
// with an Allegro tileset bitmap and the catalog entry points; everything that
// does not touch a bitmap lives here.
//
// The Rust port may pick its own in-memory layout; only the observable
// behaviour and the on-disk .til format must match. The .til format below was
// byte-verified against a real CoolBasic asset.

#include <cmath>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <vector>

// The four tile layers (in-memory array indices, NOT the on-disk order):
//   0 = background (drawn), 1 = foreground (drawn last), 2 = collision
//   (always active, nonzero = solid), 3 = data (per-tile ints, never drawn).
// Tile ids are 1-based in game code (0 = empty); the tileset is sliced 0-based
// after a `tile--`. Anim arrays are sized by `tileCount` (for a loaded map this
// is the .til's stored tile count; for MakeMap it is width*height, faithfully
// odd as CoolBasic's create does). The map is centred on (posX, posY),
// default world origin.
struct CbMapData {
    int32_t mapWidth = 0;
    int32_t mapHeight = 0;
    int32_t tileWidth = 0;
    int32_t tileHeight = 0;
    uint32_t tileCount = 0;
    std::vector<int32_t> layers[4];
    std::vector<int32_t> animLength;
    std::vector<int32_t> animSlowness;
    std::vector<float> currentFrame;
    uint8_t maskR = 0, maskG = 0, maskB = 0;
    double posX = 0.0;
    double posY = 0.0;
};

// ─── Little-endian readers ──────────────────────────────────────────────
// The .til format is little-endian; read byte-by-byte so the parser is correct
// regardless of host endianness.
inline int32_t cb_map_rd_i32(const uint8_t* p) {
    return (int32_t)((uint32_t)p[0] | ((uint32_t)p[1] << 8) |
                     ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24));
}

inline float cb_map_rd_f32(const uint8_t* p) {
    uint32_t bits = (uint32_t)p[0] | ((uint32_t)p[1] << 8) |
                    ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
    float f;
    std::memcpy(&f, &bits, sizeof(f));
    return f;
}

// ─── .til binary parser ──────────────────
// Parses the data half of a .til file (the tileset image is loaded separately,
// in cb_map.cpp). Returns false on any magic/version mismatch or truncation —
// with defensive bounds checks so a short or corrupt file cannot over-read.
//
// Layout (little-endian; two absolute seeks skip editor metadata):
//   0    : magic {40,192,13,139}
//   4    : float version, require 1.0 <= v <= 2.0
//   520  : maskR, maskG, maskB, 1 pad byte
//   820  : int32 tileCount (sizes the anim arrays — NOT the tileset tile count)
//   824  : int32 tileWidth, tileHeight
//   832  : int32 mapWidth, mapHeight (tiles)
//   840  : layers in on-disk order 0, 2, 1, 3, each preceded by a 4-byte magic
//          (descending), each w*h int32 row-major
//   then : tiles magic {250,41,8,162}
//   then : per tile i = 1..tileCount-1: int32 animLength[i], int32 animSlowness[i]
//          (index 0 unused). NB the file stores `tileCount` entries but
//          CoolBasic reads only `tileCount-1`; the trailing 8 bytes are
//          ignored. We replicate that read for byte-compatibility.
inline bool cb_map_parse(const uint8_t* b, size_t len, CbMapData& out) {
    auto have = [&](size_t off, size_t n) { return n <= len && off <= len - n; };

    if (!have(0, 8)) return false;
    if (b[0] != 40 || b[1] != 192 || b[2] != 13 || b[3] != 139) return false;
    float version = cb_map_rd_f32(b + 4);
    if (!(version >= 1.0f && version <= 2.0f)) return false;

    if (!have(520, 4)) return false;
    out.maskR = b[520];
    out.maskG = b[521];
    out.maskB = b[522];

    if (!have(820, 20)) return false;
    int32_t tileCount = cb_map_rd_i32(b + 820);
    if (tileCount < 0) return false;
    out.tileCount = (uint32_t)tileCount;
    out.tileWidth = cb_map_rd_i32(b + 824);
    out.tileHeight = cb_map_rd_i32(b + 828);
    out.mapWidth = cb_map_rd_i32(b + 832);
    out.mapHeight = cb_map_rd_i32(b + 836);
    // Defensive: CoolBasic trusts these; we reject degenerate dims so the
    // cell count / index math below can't overflow or divide by zero.
    if (out.mapWidth <= 0 || out.mapHeight <= 0) return false;
    if (out.tileWidth <= 0 || out.tileHeight <= 0) return false;

    const size_t cells = (size_t)out.mapWidth * (size_t)out.mapHeight;

    // Magic bytes, then the on-disk layer order. The 5th magic precedes the
    // per-tile animation block.
    static const uint8_t magics[5][4] = {
        {254, 45, 12, 166}, {253, 44, 11, 165}, {252, 43, 10, 164},
        {251, 42, 9, 163},  {250, 41, 8, 162},
    };
    static const int disk_order[4] = {0, 2, 1, 3};

    size_t off = 840;
    for (int i = 0; i < 4; ++i) {
        if (!have(off, 4)) return false;
        if (b[off] != magics[i][0] || b[off + 1] != magics[i][1] ||
            b[off + 2] != magics[i][2] || b[off + 3] != magics[i][3]) {
            return false;
        }
        off += 4;
        if (!have(off, cells * 4)) return false;
        const int layer = disk_order[i];
        out.layers[layer].resize(cells);
        for (size_t j = 0; j < cells; ++j) {
            out.layers[layer][j] = cb_map_rd_i32(b + off + j * 4);
        }
        off += cells * 4;
    }

    if (!have(off, 4)) return false;
    if (b[off] != magics[4][0] || b[off + 1] != magics[4][1] ||
        b[off + 2] != magics[4][2] || b[off + 3] != magics[4][3]) {
        return false;
    }
    off += 4;

    out.animLength.assign(out.tileCount, 0);
    out.animSlowness.assign(out.tileCount, 1);
    out.currentFrame.assign(out.tileCount, 0.0f);
    for (uint32_t i = 1; i < out.tileCount; ++i) {
        if (!have(off, 8)) break;  // tolerate a short anim block (defensive)
        out.animLength[i] = cb_map_rd_i32(b + off);
        out.animSlowness[i] = cb_map_rd_i32(b + off + 4);
        off += 8;
    }
    return true;
}

// ─── In-place construction (MakeMap) ──────────────
// Allocates four zeroed layers. tileCount = width*height (faithful to
// CoolBasic, where it merely sizes the anim arrays).
inline void cb_map_create(CbMapData& out, int32_t w, int32_t h, int32_t tile_w,
                          int32_t tile_h) {
    out.mapWidth = w;
    out.mapHeight = h;
    out.tileWidth = tile_w;
    out.tileHeight = tile_h;
    const size_t cells = (size_t)(w > 0 ? w : 0) * (size_t)(h > 0 ? h : 0);
    for (int i = 0; i < 4; ++i) out.layers[i].assign(cells, 0);
    out.tileCount = (uint32_t)cells;
    out.animLength.assign(out.tileCount, 0);
    out.animSlowness.assign(out.tileCount, 1);
    out.currentFrame.assign(out.tileCount, 0.0f);
}

// ─── Grid accessors ─────────────────────────────────────────────────────
// Every accessor bounds-checks the layer index (an out-of-range layer indexed
// unchecked would be UB; under unsafe_code="deny" we return 0 / no-op instead)
// and the tile coordinates (0 outside the map, matching CoolBasic).
inline bool cb_map_layer_valid(int layer) { return layer >= 0 && layer < 4; }

inline int32_t cb_map_get(const CbMapData& m, int layer, int32_t tx, int32_t ty) {
    if (!cb_map_layer_valid(layer)) return 0;
    if (tx < 0 || tx >= m.mapWidth || ty < 0 || ty >= m.mapHeight) return 0;
    return m.layers[layer][(size_t)ty * (size_t)m.mapWidth + (size_t)tx];
}

inline int32_t cb_map_get_hit(const CbMapData& m, int32_t tx, int32_t ty) {
    return cb_map_get(m, 2, tx, ty);  // collision = layer 2
}

inline void cb_map_edit(CbMapData& m, int layer, int32_t tx, int32_t ty, int32_t tile) {
    if (!cb_map_layer_valid(layer)) return;
    if (tx < 0 || tx >= m.mapWidth || ty < 0 || ty >= m.mapHeight) return;
    m.layers[layer][(size_t)ty * (size_t)m.mapWidth + (size_t)tx] = tile;
}

// World coordinates -> tile id. The map is centred on (posX, posY); world Y is
// up. Uses CoolBasic's truncating int() cast (not floor), so boundary behaviour
// matches.
inline int32_t cb_map_get_world(const CbMapData& m, int layer, double x, double y) {
    if (!cb_map_layer_valid(layer)) return 0;
    int32_t tx = (int32_t)((x - m.posX + m.mapWidth * m.tileWidth * 0.5) / m.tileWidth);
    int32_t ty = (int32_t)(-(y - m.posY - m.mapHeight * m.tileHeight * 0.5) / m.tileHeight);
    return cb_map_get(m, layer, tx, ty);
}

// Source rect (top-left) in the tileset for a 1-based tile id. Returns false for
// the empty tile (0). The slice: `tile--`, framesX = tilesetWidth / tileWidth,
// fx = tile % framesX, fy = tile / framesX (the CoolBasic tile slice, correct
// as written — unlike the image-frame slice in cb_gfx.cpp).
inline bool cb_map_tile_src(const CbMapData& m, int32_t tile, int32_t tileset_w,
                            int32_t& sx, int32_t& sy) {
    if (tile == 0) return false;
    tile -= 1;
    int32_t frames_x = m.tileWidth > 0 ? tileset_w / m.tileWidth : 0;
    if (frames_x <= 0) return false;
    int32_t fx = tile % frames_x;
    int32_t fy = tile / frames_x;
    sx = fx * m.tileWidth;
    sy = fy * m.tileHeight;
    return true;
}

// World anchor (pre-Y-flip) for the top-left of grid tile (gx, gy): the inverse
// of worldCoordinatesToMapCoordinates. The render loop draws the tile bitmap at
// (wx, -wy) under the plain world transform (the anchor Y is flipped for world
// draws).
inline void cb_map_tile_anchor(const CbMapData& m, int32_t gx, int32_t gy,
                               double& wx, double& wy) {
    wx = (double)gx * m.tileWidth - m.mapWidth * m.tileWidth * 0.5 + m.posX;
    wy = m.mapHeight * m.tileHeight * 0.5 - (double)gy * m.tileHeight + m.posY;
}

// ─── Line of sight (FD-036 Phase 5) ──────────────────
// World coordinates -> tilemap-relative pixels (origin at the map's top-left,
// Y growing down). The inverse of mapCoordinatesToWorldCoordinates.
inline void cb_map_world_to_map(const CbMapData& m, double& x, double& y) {
    x = x - m.posX + m.mapWidth * m.tileWidth * 0.5;
    y = -y + m.posY + m.mapHeight * m.tileHeight * 0.5;
}

// DDA ray walk over the collision layer (layer 2). x1/y1/x2/y2 are tilemap-
// relative pixels (run cb_map_world_to_map first). Returns true if a wall
// (nonzero collision tile) lies strictly between the two points — ObjectSight
// negates this (1 = clear line). Reproduces CoolBasic's grid traversal exactly,
// incl. the 1e-5 nudge and the "stop at the end tile or out of bounds" guard.
inline bool cb_map_ray_cast(const CbMapData& m, double x1, double y1, double x2,
                            double y2) {
    if (m.tileWidth <= 0 || m.tileHeight <= 0) return false;
    x1 += 1e-5;
    y1 += 1e-5;
    x2 += 1e-5;
    y2 += 1e-5;
    int test_x = (int)(x1 / m.tileWidth);
    int test_y = (int)(y1 / m.tileHeight);
    double dir_x = x2 - x1, dir_y = y2 - y1;
    double dist_sq = dir_x * dir_x + dir_y * dir_y;
    if (dist_sq < 1e-5) return false;  // same point
    double nf = 1.0 / std::sqrt(dist_sq);
    dir_x *= nf;
    dir_y *= nf;
    double delta_x = m.tileWidth / std::fabs(dir_x);
    double delta_y = m.tileHeight / std::fabs(dir_y);
    double max_x = test_x * m.tileWidth - x1;
    double max_y = test_y * m.tileHeight - y1;
    if (dir_x >= 0.0) max_x += m.tileWidth;
    if (dir_y >= 0.0) max_y += m.tileHeight;
    max_x /= dir_x;
    max_y /= dir_y;
    int step_x = dir_x < 0 ? -1 : 1;
    int step_y = dir_y < 0 ? -1 : 1;
    int end_x = (int)(x2 / m.tileWidth);
    int end_y = (int)(y2 / m.tileHeight);
    while ((test_x != end_x || test_y != end_y) && test_x >= 0 && test_x < m.mapWidth &&
           test_y >= 0 && test_y < m.mapHeight) {
        if (max_x < max_y) {
            max_x += delta_x;
            test_x += step_x;
        } else {
            max_y += delta_y;
            test_y += step_y;
        }
        if (cb_map_get_hit(m, test_x, test_y)) return true;
    }
    return false;
}

// ─── Tile animation advance (FD-036) ──────────────────────
// Advance one animated tile's currentFrame by `timestep` seconds, following the
// CoolBasic formula: cur += timestep / (slowness * animSpeed), reset to 0 once
// (int)cur *exceeds* animLength — so the tile cycles tile..tile+animLength, i.e.
// animLength+1 frames (animLength==1 is a 2-frame tile). The render samples
// `tile + (int)cur`. Caller passes animSpeed > 0 and animLength > 0; a
// non-positive slowness is treated as 1 (matches the tick's guard). Pure, so the
// frame/wrap math is headless-unit-tested while the wall-clock timestep that
// feeds it stays in cb_map.cpp.
inline float cb_map_advance_frame(float cur, int32_t animLength, int32_t slowness,
                                  float animSpeed, float timestep) {
    if (slowness <= 0) slowness = 1;
    cur += timestep / ((float)slowness * animSpeed);
    if ((int32_t)cur > animLength) cur = 0.0f;
    return cur;
}

#endif  // CB_MAP_DATA_H
