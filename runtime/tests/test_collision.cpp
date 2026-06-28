// Unit tests for the pure collision + picking geometry in
// cb_collision_data.h. No display / Allegro needed — the header is self-contained
// (mirrors test_object.cpp / test_map.cpp / test_camera.cpp). These pin the
// overlap predicates, the contact-angle formula `((rad+π)/π)*180`, and the box/
// circle collision-resolution math (separate-axis push-out, Stop-vs-Slide angle
// source). The map-grid orchestration (Rect/CircleMap) is exercised end-to-end by
// the graphics-gated cb-driver fixture; only its pure predicates live here.

#include "cb_collision_data.h"
#include "cb_map_data.h"  // ObjectSight DDA (cb_map_ray_cast / cb_map_world_to_map)

#include <gtest/gtest.h>

namespace {
constexpr double kEps = 1e-4;
}

// Circle-circle overlap: squared-distance < squared-radius-sum; touching edges
// (dist == sum) do NOT count.
TEST(CollisionOverlap, CircleCircle) {
    EXPECT_TRUE(cb_circle_circle_overlap(0, 0, 5, 8, 0, 5));    // dist²64 < 100
    EXPECT_FALSE(cb_circle_circle_overlap(0, 0, 5, 11, 0, 5));  // dist²121 > 100
    EXPECT_FALSE(cb_circle_circle_overlap(0, 0, 5, 10, 0, 5));  // exactly touching
}

// Circle-rect overlap: the rect is top-left (rx,ry) + (w,h). Inside, far, and a
// diagonal corner-grazing case (the expensive corner branch).
TEST(CollisionOverlap, CircleRect) {
    // Circle centre inside the rect → always collides.
    EXPECT_TRUE(cb_circle_rect_overlap(5, 5, 1, 0, 0, 10, 10));
    // Far to the side → no collision.
    EXPECT_FALSE(cb_circle_rect_overlap(20, 5, 1, 0, 0, 10, 10));
    // Just past a corner → corner-distance test, still touching.
    EXPECT_TRUE(cb_circle_rect_overlap(10.5, 10.5, 1, 0, 0, 10, 10));
    // Further past the corner → outside.
    EXPECT_FALSE(cb_circle_rect_overlap(11.2, 11.2, 1, 0, 0, 10, 10));
}

// Contact-angle formula maps atan2 radians [-π,π] to degrees via ((rad+π)/π)*180
// (note /π, not /2π) — so the range is [0, 360] and rad=0 → 180.
TEST(CollisionAngle, RadToDegrees) {
    EXPECT_NEAR(cb_collision_angle_deg(0.0), 180.0, kEps);
    EXPECT_NEAR(cb_collision_angle_deg(cb_collision_pi), 360.0, kEps);
    EXPECT_NEAR(cb_collision_angle_deg(-cb_collision_pi), 0.0, kEps);
    EXPECT_NEAR(cb_collision_angle_deg(cb_collision_pi / 2.0), 270.0, kEps);
}

// Box-box resolution: object a (0,0, 10×10) overlapping a collider at (5,0,
// 10×10). The collider is to the right, so the X-pass pushes a left to objX=-6
// (= 5 - 5 - 5 - 1) and records one contact at (0,0) with angle 180; the Y-pass
// no longer overlaps, so there is exactly one hit.
TEST(CollisionResolve, BoxBoxPushLeft) {
    CbBoxResolve r = cb_box_box_resolve(0, 0, /*safeY*/ 0, 10, 10, 5, 0, 10, 10);
    EXPECT_EQ(r.hitCount, 1);
    EXPECT_NEAR(r.objX, -6.0, kEps);
    EXPECT_NEAR(r.objY, 0.0, kEps);
    EXPECT_NEAR(r.hits[0].angle, 180.0, kEps);
    EXPECT_NEAR(r.hits[0].x, 0.0, kEps);
    EXPECT_NEAR(r.hits[0].y, 0.0, kEps);

    // Non-overlapping pair → no contact, position unchanged.
    CbBoxResolve none = cb_box_box_resolve(0, 0, 0, 10, 10, 100, 0, 10, 10);
    EXPECT_EQ(none.hitCount, 0);
    EXPECT_NEAR(none.objX, 0.0, kEps);
}

// Circle-circle resolution (Slide): object a radius 5 at (0,0), other radius 5 at
// (6,0). Pushed back to r1+r2+0.5 = 10.5 from the other along the contact angle
// (rad=0 here) → objX = 6 - 10.5 = -4.5; contact at objX + (r1+1) = 1.5.
TEST(CollisionResolve, CircleCircleSlide) {
    CbCircleResolve r =
        cb_circle_circle_resolve(0, 0, /*safeX*/ 0, /*safeY*/ 0, 5, 6, 0, 5, false);
    EXPECT_EQ(r.hitCount, 1);
    EXPECT_NEAR(r.objX, -4.5, kEps);
    EXPECT_NEAR(r.objY, 0.0, kEps);
    EXPECT_NEAR(r.hit.angle, 180.0, kEps);
    EXPECT_NEAR(r.hit.x, 1.5, kEps);
    EXPECT_NEAR(r.hit.y, 0.0, kEps);
}

// Stop vs Slide differ only in the contact-angle source: Slide measures from the
// current position, Stop from the last safe position. With a safe position offset
// in Y, the two resolve to different points (the whole purpose of Stop).
TEST(CollisionResolve, CircleCircleStopDiffersFromSlide) {
    // Object currently at (0,0) but its last safe position was (0,3).
    CbCircleResolve slide =
        cb_circle_circle_resolve(0, 0, 0, 3, 5, 6, 0, 5, false);
    CbCircleResolve stop =
        cb_circle_circle_resolve(0, 0, 0, 3, 5, 6, 0, 5, true);
    EXPECT_EQ(slide.hitCount, 1);
    EXPECT_EQ(stop.hitCount, 1);
    // Slide ignores safe → resolves straight along +x (objY stays 0).
    EXPECT_NEAR(slide.objY, 0.0, kEps);
    // Stop uses the safe position → pushes the object off the x-axis.
    EXPECT_GT(stop.objY, 0.5);
}

// ─── Picking raycasts ───────────────────────────────────────────────────

// Box raycast: picker at (0,0) facing 0° (right) hits the LEFT edge of a 4×4 box
// centred at (10,0) → contact (8, 0). Facing away (180°) misses entirely.
TEST(PickRaycast, BoxFrontHitAndMiss) {
    double hx = 0, hy = 0;
    EXPECT_TRUE(cb_box_ray_cast(0, 0, 0, 10, 0, 4, 4, hx, hy));
    EXPECT_NEAR(hx, 8.0, kEps);
    EXPECT_NEAR(hy, 0.0, kEps);
    EXPECT_FALSE(cb_box_ray_cast(0, 0, 180, 10, 0, 4, 4, hx, hy));
}

// Circle raycast: picker at (0,0) facing 0° hits the near edge of a circle
// (diameter 4 → radius 2) centred at (10,0) → contact (8, 0). A ray starting
// INSIDE the circle does not pick it.
TEST(PickRaycast, CircleFrontHitAndInside) {
    double hx = 0, hy = 0;
    EXPECT_TRUE(cb_circle_ray_cast(0, 0, 0, 10, 0, 4, hx, hy));
    EXPECT_NEAR(hx, 8.0, kEps);
    EXPECT_NEAR(hy, 0.0, kEps);
    // Start inside → false (returns the start point).
    EXPECT_FALSE(cb_circle_ray_cast(10, 0, 0, 10, 0, 4, hx, hy));
}

// Point-in-shape pick tests (CameraPick funnel).
TEST(PickPoint, BoxAndCircleContainment) {
    EXPECT_TRUE(cb_can_pick_box(10, 0, 4, 4, 11, 0));    // |1| < 2
    EXPECT_FALSE(cb_can_pick_box(10, 0, 4, 4, 13, 0));   // |3| > 2
    EXPECT_TRUE(cb_can_pick_circle(10, 0, 4, 11, 0));    // dist 1 < r 2
    EXPECT_FALSE(cb_can_pick_circle(10, 0, 4, 13, 0));   // dist 3 > r 2
}

// ObjectSight DDA: a horizontal ray across a 4×4 tile (16px) map centred on the
// origin. With no wall the line is clear; a wall on the collision layer in the
// ray's path blocks it.
TEST(ObjectSight, RayBlockedByWall) {
    CbMapData m;
    cb_map_create(m, 4, 4, 16, 16);

    // Clear line from world (-20,0) to (20,0).
    double ax = -20, ay = 0, bx = 20, by = 0;
    cb_map_world_to_map(m, ax, ay);
    cb_map_world_to_map(m, bx, by);
    EXPECT_FALSE(cb_map_ray_cast(m, ax, ay, bx, by));

    // Put a wall on the collision layer (2) at grid (2,2), in the ray's row.
    cb_map_edit(m, 2, 2, 2, 1);
    double cx = -20, cy = 0, dx = 20, dy = 0;
    cb_map_world_to_map(m, cx, cy);
    cb_map_world_to_map(m, dx, dy);
    EXPECT_TRUE(cb_map_ray_cast(m, cx, cy, dx, dy));
}
