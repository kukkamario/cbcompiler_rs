// FD-036 Phase 4: unit tests for the pure object math in cb_object_data.h. No
// display / Allegro needed — the header is self-contained (mirrors
// test_map.cpp / test_camera.cpp). These pin the GetAngle2/PointObject and
// Distance2 formulas, the MoveObject heading, the animated-frame slice (with a
// multi-row non-square sheet to lock the corrected /framesX math), the rotated
// ObjectSizeX/Y bounding box, the TurnObject 0..360 wrap, the per-tick animation
// advance + stop, and the ObjectLife decrement.

#include "cb_object_data.h"

#include <gtest/gtest.h>

#include <cstdint>

namespace {
constexpr double kEps = 1e-4;
}

// GetAngle2(a, b) / PointObject: cardinal directions give clean degrees.
// b right of a -> 0, above (world +Y) -> 90, left -> 180, below (world -Y) -> 270.
TEST(ObjectAngle, Angle2Cardinals) {
    EXPECT_NEAR(cb_object_angle2(0, 0, 1, 0), 0.0, kEps);
    EXPECT_NEAR(cb_object_angle2(0, 0, 0, 1), 90.0, kEps);
    EXPECT_NEAR(cb_object_angle2(0, 0, -1, 0), 180.0, kEps);
    EXPECT_NEAR(cb_object_angle2(0, 0, 0, -1), 270.0, kEps);
    // Off-origin: only the delta matters.
    EXPECT_NEAR(cb_object_angle2(5, 5, 6, 5), 0.0, kEps);
}

TEST(ObjectAngle, Distance2) {
    EXPECT_NEAR(cb_object_distance2(0, 0, 3, 4), 5.0, kEps);
    EXPECT_NEAR(cb_object_distance2(1, 1, 1, 1), 0.0, kEps);
    // Symmetric.
    EXPECT_NEAR(cb_object_distance2(0, 0, 3, 4),
                cb_object_distance2(3, 4, 0, 0), kEps);
}

// MoveObject heading: forward follows the angle, side is 90° clockwise of it.
TEST(ObjectMove, HeadingDelta) {
    double dx = 0, dy = 0;
    cb_object_move_delta(0.0, 5.0, 0.0, dx, dy);  // forward along 0°
    EXPECT_NEAR(dx, 5.0, kEps);
    EXPECT_NEAR(dy, 0.0, kEps);

    cb_object_move_delta(0.0, 0.0, 3.0, dx, dy);  // side at 0° -> heading-90°
    EXPECT_NEAR(dx, 0.0, kEps);
    EXPECT_NEAR(dy, -3.0, kEps);

    cb_object_move_delta(90.0, 2.0, 0.0, dx, dy);  // forward along 90°
    EXPECT_NEAR(dx, 0.0, kEps);
    EXPECT_NEAR(dy, 2.0, kEps);
}

// Animated-frame slice on a multi-row, non-square sheet: texture 4px wide, 2x3
// frames -> framesX = 2. Frames 2..5 (rows 1,2) lock the /framesX + *frameHeight
// math; the cbimage.cpp Phase-1 bug (/framesY, *frameWidth) would mis-slice them.
TEST(ObjectFrame, MultiRowNonSquareSlice) {
    struct Case {
        int32_t frame, col, row, left, top;
    };
    const Case cases[] = {
        {0, 0, 0, 0, 0}, {1, 1, 0, 2, 0}, {2, 0, 1, 0, 3},
        {3, 1, 1, 2, 3}, {4, 0, 2, 0, 6}, {5, 1, 2, 2, 6},
    };
    for (const Case& c : cases) {
        int32_t col = -1, row = -1, left = -1, top = -1;
        ASSERT_TRUE(cb_object_frame_slice(4, 2, 3, c.frame, col, row, left, top))
            << "frame " << c.frame;
        EXPECT_EQ(col, c.col) << "frame " << c.frame;
        EXPECT_EQ(row, c.row) << "frame " << c.frame;
        EXPECT_EQ(left, c.left) << "frame " << c.frame;
        EXPECT_EQ(top, c.top) << "frame " << c.frame;
    }
    // Degenerate sheet -> false, outputs zeroed.
    int32_t col = 9, row = 9, left = 9, top = 9;
    EXPECT_FALSE(cb_object_frame_slice(4, 0, 3, 0, col, row, left, top));
    EXPECT_EQ(col, 0);
}

// ObjectSizeX/Y: stored size at angle 0; rotated AABB otherwise.
TEST(ObjectSize, RotatedBoundingBox) {
    EXPECT_EQ(cb_object_size_x(10, 4, 0.0), 10);
    EXPECT_EQ(cb_object_size_y(10, 4, 0.0), 4);
    // 90°: width and height swap.
    EXPECT_EQ(cb_object_size_x(10, 4, 90.0), 4);
    EXPECT_EQ(cb_object_size_y(10, 4, 90.0), 10);
    // 45° of a 2x2: |2*0.7071| + |0.7071*2| = 2.828 -> round(3.328) = 3.
    EXPECT_EQ(cb_object_size_x(2, 2, 45.0), 3);
    EXPECT_EQ(cb_object_size_y(2, 2, 45.0), 3);
}

// TurnObject: relative one-shot rotate, wrapped to [0, 360] (faithful >360/<0).
TEST(ObjectTurn, WrapsTo360) {
    EXPECT_NEAR(cb_object_turn(90.0, 30.0), 120.0, kEps);
    EXPECT_NEAR(cb_object_turn(350.0, 20.0), 10.0, kEps);   // 370 -> 10
    EXPECT_NEAR(cb_object_turn(10.0, -20.0), 350.0, kEps);  // -10 -> 350
    // Exactly 360 stays 360 (cbEnchanted wraps only on > 360).
    EXPECT_NEAR(cb_object_turn(0.0, 360.0), 360.0, kEps);
}

// Forward animation advances by animSpeed and wraps at the end; a looping anim
// keeps playing, a one-shot stops at the wrap.
TEST(ObjectAnim, ForwardLoopAndStop) {
    CbAnimState loop;
    loop.current_frame = 0.0;
    loop.anim_start_frame = 0;
    loop.anim_ending_frame = 2;
    loop.anim_speed = 1.0;
    loop.anim_looping = true;
    loop.playing = true;

    cb_object_anim_advance(loop);
    EXPECT_NEAR(loop.current_frame, 1.0, kEps);
    EXPECT_TRUE(loop.playing);
    cb_object_anim_advance(loop);
    EXPECT_NEAR(loop.current_frame, 2.0, kEps);
    cb_object_anim_advance(loop);  // (int)3 > 2 -> wrap to start, still looping
    EXPECT_NEAR(loop.current_frame, 0.0, kEps);
    EXPECT_TRUE(loop.playing);

    CbAnimState once;
    once.current_frame = 0.0;
    once.anim_start_frame = 0;
    once.anim_ending_frame = 1;
    once.anim_speed = 1.0;
    once.anim_looping = false;
    once.playing = true;

    cb_object_anim_advance(once);
    EXPECT_NEAR(once.current_frame, 1.0, kEps);
    EXPECT_TRUE(once.playing);
    cb_object_anim_advance(once);  // (int)2 > 1 -> wrap, not looping -> stop
    EXPECT_NEAR(once.current_frame, 0.0, kEps);
    EXPECT_FALSE(once.playing);

    // A stopped animation never advances.
    CbAnimState idle;
    idle.playing = false;
    idle.current_frame = 4.0;
    cb_object_anim_advance(idle);
    EXPECT_NEAR(idle.current_frame, 4.0, kEps);
}

// ObjectLife: decrement per tick, auto-delete (return true) when it reaches 0.
TEST(ObjectLife, DecrementToZero) {
    uint32_t life = 3;
    EXPECT_FALSE(cb_object_life_tick(life));
    EXPECT_EQ(life, 2u);
    EXPECT_FALSE(cb_object_life_tick(life));
    EXPECT_EQ(life, 1u);
    EXPECT_TRUE(cb_object_life_tick(life));
    EXPECT_EQ(life, 0u);

    uint32_t one = 1;
    EXPECT_TRUE(cb_object_life_tick(one));
}
