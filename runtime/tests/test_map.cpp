// FD-036 Phase 3: unit tests for the pure tilemap data + parser in cb_map_data.h.
// No display / Allegro needed — the header is self-contained (mirrors
// test_camera.cpp). These pin the .til binary format (magic bytes, the two
// absolute seeks, the on-disk layer order 0/2/1/3, and the tileCount-vs-
// tileCount-1 anim read), the grid accessors and their bounds/layer checks, the
// world<->tile centering math, and the tileset slice + tile-anchor math.
//
// The real CoolBasic asset D:\CoolBasic\Media\testmap.til byte-verified the
// format during development; the graphics-gated cb-driver fixture loads it end
// to end. Here we craft a small synthetic .til so the parser is regression-pinned
// without committing a binary into the runtime tree.

#include "cb_map_data.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <cstring>
#include <vector>

namespace {

void put_i32(std::vector<uint8_t>& b, size_t off, int32_t v) {
    uint32_t u = (uint32_t)v;
    b[off + 0] = (uint8_t)(u & 0xff);
    b[off + 1] = (uint8_t)((u >> 8) & 0xff);
    b[off + 2] = (uint8_t)((u >> 16) & 0xff);
    b[off + 3] = (uint8_t)((u >> 24) & 0xff);
}

void put_f32(std::vector<uint8_t>& b, size_t off, float f) {
    uint32_t u;
    std::memcpy(&u, &f, sizeof(u));
    put_i32(b, off, (int32_t)u);
}

// A 3x2 map, 16x16 tiles, tileCount=4. On-disk layer order 0,2,1,3. The anim
// block stores 4 entries but the parser reads only tileCount-1 = 3 (the 4th is
// trailing junk, mirroring the real asset's tileCount-vs-read mismatch).
std::vector<uint8_t> make_til() {
    std::vector<uint8_t> b(988, 0);
    b[0] = 40; b[1] = 192; b[2] = 13; b[3] = 139;  // magic
    put_f32(b, 4, 1.3f);                            // version (in [1.0, 2.0])
    b[520] = 10; b[521] = 20; b[522] = 30;          // mask RGB (+ pad at 523)
    put_i32(b, 820, 4);                             // tileCount
    put_i32(b, 824, 16);                            // tileWidth
    put_i32(b, 828, 16);                            // tileHeight
    put_i32(b, 832, 3);                             // mapWidth
    put_i32(b, 836, 2);                             // mapHeight

    const uint8_t magics[5][4] = {
        {254, 45, 12, 166}, {253, 44, 11, 165}, {252, 43, 10, 164},
        {251, 42, 9, 163},  {250, 41, 8, 162},
    };
    // Data in array-index terms: layer0 background, layer1 foreground,
    // layer2 collision, layer3 data. Written in on-disk order 0,2,1,3.
    const int32_t L0[6] = {1, 2, 3, 4, 5, 6};
    const int32_t L2[6] = {0, 1, 0, 1, 0, 1};
    const int32_t L1[6] = {7, 0, 0, 0, 0, 8};
    const int32_t L3[6] = {100, 0, 0, 0, 0, 200};
    const int32_t* disk[4] = {L0, L2, L1, L3};

    size_t off = 840;
    for (int i = 0; i < 4; ++i) {
        for (int k = 0; k < 4; ++k) b[off + k] = magics[i][k];
        off += 4;
        for (int j = 0; j < 6; ++j) { put_i32(b, off, disk[i][j]); off += 4; }
    }
    for (int k = 0; k < 4; ++k) b[off + k] = magics[4][k];
    off += 4;
    // anim entries (animLength, animSlowness): E0->index1, E1->index2,
    // E2->index3; E3 is trailing and must be ignored.
    const int32_t anim[4][2] = {{2, 5}, {0, 1}, {4, 3}, {99, 99}};
    for (int i = 0; i < 4; ++i) {
        put_i32(b, off, anim[i][0]);
        put_i32(b, off + 4, anim[i][1]);
        off += 8;
    }
    return b;
}

}  // namespace

TEST(MapParse, HeaderAndDims) {
    auto bytes = make_til();
    CbMapData m;
    ASSERT_TRUE(cb_map_parse(bytes.data(), bytes.size(), m));
    EXPECT_EQ(m.mapWidth, 3);
    EXPECT_EQ(m.mapHeight, 2);
    EXPECT_EQ(m.tileWidth, 16);
    EXPECT_EQ(m.tileHeight, 16);
    EXPECT_EQ(m.tileCount, 4u);
    EXPECT_EQ(m.maskR, 10);
    EXPECT_EQ(m.maskG, 20);
    EXPECT_EQ(m.maskB, 30);
}

// The on-disk order is 0,2,1,3 — a misread would swap foreground/collision.
TEST(MapParse, LayersInDiskOrder) {
    auto bytes = make_til();
    CbMapData m;
    ASSERT_TRUE(cb_map_parse(bytes.data(), bytes.size(), m));
    const int32_t L0[6] = {1, 2, 3, 4, 5, 6};
    const int32_t L1[6] = {7, 0, 0, 0, 0, 8};
    const int32_t L2[6] = {0, 1, 0, 1, 0, 1};
    const int32_t L3[6] = {100, 0, 0, 0, 0, 200};
    for (int j = 0; j < 6; ++j) {
        EXPECT_EQ(m.layers[0][j], L0[j]) << "bg j=" << j;
        EXPECT_EQ(m.layers[1][j], L1[j]) << "fg j=" << j;
        EXPECT_EQ(m.layers[2][j], L2[j]) << "coll j=" << j;
        EXPECT_EQ(m.layers[3][j], L3[j]) << "data j=" << j;
    }
}

// The parser reads tileCount-1 entries (i=1..3); index 0 and the trailing E3
// are not read.
TEST(MapParse, AnimReadsTileCountMinusOne) {
    auto bytes = make_til();
    CbMapData m;
    ASSERT_TRUE(cb_map_parse(bytes.data(), bytes.size(), m));
    ASSERT_EQ(m.animLength.size(), 4u);
    EXPECT_EQ(m.animLength[1], 2);
    EXPECT_EQ(m.animSlowness[1], 5);
    EXPECT_EQ(m.animLength[2], 0);
    EXPECT_EQ(m.animSlowness[2], 1);
    EXPECT_EQ(m.animLength[3], 4);
    EXPECT_EQ(m.animSlowness[3], 3);
    // The trailing E3 (99,99) was not read into any slot.
    EXPECT_EQ(m.animLength[0], 0);
}

TEST(MapParse, RejectsBadMagicAndVersion) {
    CbMapData m;
    // Truncated.
    EXPECT_FALSE(cb_map_parse(nullptr, 0, m));
    auto bad_magic = make_til();
    bad_magic[2] = 0;
    EXPECT_FALSE(cb_map_parse(bad_magic.data(), bad_magic.size(), m));
    auto bad_ver = make_til();
    put_f32(bad_ver, 4, 3.0f);  // outside [1.0, 2.0]
    EXPECT_FALSE(cb_map_parse(bad_ver.data(), bad_ver.size(), m));
    // A short buffer (header only) must fail rather than over-read.
    auto truncated = make_til();
    truncated.resize(900);
    EXPECT_FALSE(cb_map_parse(truncated.data(), truncated.size(), m));
}

TEST(MapGrid, GetAndBounds) {
    auto bytes = make_til();
    CbMapData m;
    ASSERT_TRUE(cb_map_parse(bytes.data(), bytes.size(), m));
    EXPECT_EQ(cb_map_get(m, 0, 0, 0), 1);
    EXPECT_EQ(cb_map_get(m, 0, 2, 1), 6);   // idx = 1*3 + 2 = 5
    EXPECT_EQ(cb_map_get(m, 1, 0, 0), 7);   // foreground
    EXPECT_EQ(cb_map_get(m, 3, 2, 1), 200); // data layer, idx = 1*3 + 2 = 5
    // Out of bounds -> 0.
    EXPECT_EQ(cb_map_get(m, 0, 3, 0), 0);
    EXPECT_EQ(cb_map_get(m, 0, -1, 0), 0);
    EXPECT_EQ(cb_map_get(m, 0, 0, 2), 0);
    // Out-of-range layer -> 0 (defensive; cbEnchanted would index OOB).
    EXPECT_EQ(cb_map_get(m, 4, 0, 0), 0);
    EXPECT_EQ(cb_map_get(m, -1, 0, 0), 0);
    // getHit reads the collision layer (2).
    EXPECT_EQ(cb_map_get_hit(m, 0, 0), 0);
    EXPECT_EQ(cb_map_get_hit(m, 0, 1), 1);  // idx = 1*3 + 0 = 3 -> 1
}

TEST(MapGrid, EditInBoundsAndOutOfBounds) {
    auto bytes = make_til();
    CbMapData m;
    ASSERT_TRUE(cb_map_parse(bytes.data(), bytes.size(), m));
    cb_map_edit(m, 0, 1, 0, 42);
    EXPECT_EQ(cb_map_get(m, 0, 1, 0), 42);
    // Out-of-bounds / bad layer edits are ignored (no crash, no change).
    cb_map_edit(m, 0, 99, 99, 7);
    cb_map_edit(m, 9, 0, 0, 7);
    EXPECT_EQ(cb_map_get(m, 0, 0, 0), 1);
}

// World coords -> tile, centred on the origin: mapW*tileW*0.5 = 24,
// mapH*tileH*0.5 = 16. World (0,0) -> tile (1,1) -> layer0 idx 4 -> 5.
TEST(MapGrid, GetWorldCentering) {
    auto bytes = make_til();
    CbMapData m;
    ASSERT_TRUE(cb_map_parse(bytes.data(), bytes.size(), m));
    EXPECT_EQ(cb_map_get_world(m, 0, 0.0, 0.0), 5);
    // Far outside the map region -> 0.
    EXPECT_EQ(cb_map_get_world(m, 0, 1000.0, 0.0), 0);
    EXPECT_EQ(cb_map_get_world(m, 0, 0.0, -1000.0), 0);
}

// Tileset slice: 16px tiles in a 64px-wide tileset -> framesX = 4. 1-based ids
// slice after a tile--; the empty tile (0) yields no rect.
TEST(MapRenderMath, TileSrcRect) {
    auto bytes = make_til();
    CbMapData m;
    ASSERT_TRUE(cb_map_parse(bytes.data(), bytes.size(), m));
    int32_t sx = -1, sy = -1;
    EXPECT_FALSE(cb_map_tile_src(m, 0, 64, sx, sy));  // empty
    EXPECT_TRUE(cb_map_tile_src(m, 1, 64, sx, sy));
    EXPECT_EQ(sx, 0);
    EXPECT_EQ(sy, 0);
    EXPECT_TRUE(cb_map_tile_src(m, 5, 64, sx, sy));   // tile-- = 4 -> (0,1)
    EXPECT_EQ(sx, 0);
    EXPECT_EQ(sy, 16);
    EXPECT_TRUE(cb_map_tile_src(m, 4, 64, sx, sy));   // tile-- = 3 -> (3,0)
    EXPECT_EQ(sx, 48);
    EXPECT_EQ(sy, 0);
}

// Tile world anchor (pre-Y-flip): inverse of worldCoordinatesToMapCoordinates.
TEST(MapRenderMath, TileAnchor) {
    auto bytes = make_til();
    CbMapData m;
    ASSERT_TRUE(cb_map_parse(bytes.data(), bytes.size(), m));
    double wx = 0.0, wy = 0.0;
    cb_map_tile_anchor(m, 0, 0, wx, wy);
    EXPECT_DOUBLE_EQ(wx, -24.0);
    EXPECT_DOUBLE_EQ(wy, 16.0);
    cb_map_tile_anchor(m, 1, 1, wx, wy);
    EXPECT_DOUBLE_EQ(wx, -8.0);
    EXPECT_DOUBLE_EQ(wy, 0.0);
}

// MakeMap-style construction: zeroed layers, tileCount = w*h.
TEST(MapCreate, EmptyGrid) {
    CbMapData m;
    cb_map_create(m, 8, 6, 16, 16);
    EXPECT_EQ(m.mapWidth, 8);
    EXPECT_EQ(m.mapHeight, 6);
    EXPECT_EQ(m.tileCount, 48u);
    EXPECT_EQ(cb_map_get(m, 0, 0, 0), 0);
    cb_map_edit(m, 0, 3, 2, 9);
    EXPECT_EQ(cb_map_get(m, 0, 3, 2), 9);
}
