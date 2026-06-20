// FD-036 Phase 2: unit tests for the pure camera affine in cb_camera_math.h. No
// display / Allegro needed — the header is self-contained inline math (mirrors
// test_geom.cpp). These pin the world<->screen transform: centering on the
// design resolution, the Y-inversion living in the wrappers (not the matrix),
// and round-trip identity. The render transform fed to al_use_transform for
// DrawToWorld is checked to equal world->screen.

#include "cb_camera_math.h"

#include <gtest/gtest.h>

#include <cmath>

namespace {
constexpr double kPi = 3.14159265358979323846;
constexpr double kEps = 1e-9;

// Default logical design resolution (CoolBasic's 400x300).
constexpr int kW = 400;
constexpr int kH = 300;
}  // namespace

// Camera at origin, zoom 1, angle 0: world (0,0) maps to the screen center,
// which is the design resolution's half (200, 150) — NOT the window size.
TEST(CameraTransform, OriginMapsToDesignCenter) {
    CbAffine world = cb_build_world_transform(0.0, 0.0, 0.0, 1.0, kW, kH);
    double x = 0.0, y = 0.0;
    cb_world_to_screen(world, x, y);
    EXPECT_NEAR(x, kW / 2, kEps);
    EXPECT_NEAR(y, kH / 2, kEps);
}

// Centering uses the design resolution with integer halves (CoolBasic's
// getDefaultWidth()/2): an odd design width centers at floor(w/2), and changing
// the design size moves the center — proving it is not the window size.
TEST(CameraTransform, CenteringUsesIntegerDesignHalves) {
    CbAffine odd = cb_build_world_transform(0.0, 0.0, 0.0, 1.0, 401, 301);
    double x = 0.0, y = 0.0;
    cb_world_to_screen(odd, x, y);
    EXPECT_NEAR(x, 200.0, kEps);  // 401 / 2 == 200, not 200.5
    EXPECT_NEAR(y, 150.0, kEps);  // 301 / 2 == 150

    CbAffine other = cb_build_world_transform(0.0, 0.0, 0.0, 1.0, 800, 600);
    double bx = 0.0, by = 0.0;
    cb_world_to_screen(other, bx, by);
    EXPECT_NEAR(bx, 400.0, kEps);
    EXPECT_NEAR(by, 300.0, kEps);
}

// The Y-inversion lives in the wrapper, not the matrix: placing the camera on a
// world point maps that point to the screen center regardless of its world Y.
TEST(CameraTransform, CameraCenteredPointMapsToScreenCenter) {
    CbAffine world = cb_build_world_transform(120.0, -47.0, 0.0, 1.0, kW, kH);
    double x = 120.0, y = -47.0;
    cb_world_to_screen(world, x, y);
    EXPECT_NEAR(x, kW / 2, kEps);
    EXPECT_NEAR(y, kH / 2, kEps);
}

// screen -> world -> screen is the identity (the inverse is exact for doubles).
TEST(CameraTransform, ScreenWorldScreenRoundTrip) {
    CbAffine world = cb_build_world_transform(100.0, 50.0, kPi / 2.0, 2.0, kW, kH);
    const double pts[][2] = {{0, 0}, {200, 150}, {399, 299}, {37.5, 212.25}};
    for (const auto& p : pts) {
        double x = p[0], y = p[1];
        cb_screen_to_world(world, x, y);
        cb_world_to_screen(world, x, y);
        EXPECT_NEAR(x, p[0], 1e-6) << "x round-trip at (" << p[0] << "," << p[1] << ")";
        EXPECT_NEAR(y, p[1], 1e-6) << "y round-trip at (" << p[0] << "," << p[1] << ")";
    }
}

// world -> screen -> world is also the identity.
TEST(CameraTransform, WorldScreenWorldRoundTrip) {
    CbAffine world = cb_build_world_transform(-30.0, 8.0, -kPi / 6.0, 0.5, kW, kH);
    const double pts[][2] = {{0, 0}, {-100, 250}, {64.25, -17.75}};
    for (const auto& p : pts) {
        double x = p[0], y = p[1];
        cb_world_to_screen(world, x, y);
        cb_screen_to_world(world, x, y);
        EXPECT_NEAR(x, p[0], 1e-6) << "x round-trip at (" << p[0] << "," << p[1] << ")";
        EXPECT_NEAR(y, p[1], 1e-6) << "y round-trip at (" << p[0] << "," << p[1] << ")";
    }
}

// The render transform (fed to al_use_transform for DrawToWorld) applied to raw
// world coords must equal world->screen — i.e. it folds in the wrapper Y-flip.
TEST(CameraTransform, RenderTransformEqualsWorldToScreen) {
    const double cx = 75.0, cy = -22.0, rad = kPi / 3.0, zoom = 1.7;
    CbAffine world = cb_build_world_transform(cx, cy, rad, zoom, kW, kH);
    CbAffine render = cb_build_render_transform(cx, cy, rad, zoom, kW, kH);
    const double pts[][2] = {{0, 0}, {120, -60}, {-200, 33.5}};
    for (const auto& p : pts) {
        double wx = p[0], wy = p[1];
        cb_world_to_screen(world, wx, wy);

        double rx = p[0], ry = p[1];
        cb_affine_apply(render, rx, ry);

        EXPECT_NEAR(rx, wx, kEps) << "render x at (" << p[0] << "," << p[1] << ")";
        EXPECT_NEAR(ry, wy, kEps) << "render y at (" << p[0] << "," << p[1] << ")";
    }
}

// Zoom scales distance from the screen center; angle 0, camera at origin.
TEST(CameraTransform, ZoomScalesFromCenter) {
    CbAffine world = cb_build_world_transform(0.0, 0.0, 0.0, 2.0, kW, kH);
    double x = 10.0, y = 0.0;  // 10 units right of the camera, world Y up
    cb_world_to_screen(world, x, y);
    // Screen center is (200,150); zoom 2 places it 20px right, same Y.
    EXPECT_NEAR(x, 220.0, kEps);
    EXPECT_NEAR(y, 150.0, kEps);
}
