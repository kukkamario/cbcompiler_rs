// FD-022: unit tests for the pure AABB helper in cb_geom.h. No display / Allegro
// needed — the header is self-contained inline logic.

#include "cb_geom.h"

#include <gtest/gtest.h>

// rect_overlap's native convention: box = left=x, right=x+w, top=y-h, bottom=y.
TEST(RectOverlap, OverlappingBoxesOverlap) {
    // Box1 x[0,10] y[-10,0]; Box2 x[5,15] y[-15,-5] — share x[5,10], y[-10,-5].
    EXPECT_TRUE(rect_overlap(0, 0, 10, 10, 5, -5, 10, 10));
}

TEST(RectOverlap, DisjointInXDoNotOverlap) {
    EXPECT_FALSE(rect_overlap(0, 0, 10, 10, 20, 0, 10, 10));
}

TEST(RectOverlap, DisjointInYDoNotOverlap) {
    // Box1 y[-10,0]; Box2 bottom=-20,h=10 -> y[-30,-20], fully below.
    EXPECT_FALSE(rect_overlap(0, 0, 10, 10, 0, -20, 10, 10));
}

TEST(RectOverlap, SharedEdgeDoesNotCount) {
    // Box2 sits immediately to the right (l2 == r1); the epsilon keeps a shared
    // edge from registering as a collision.
    EXPECT_FALSE(rect_overlap(0, 0, 10, 10, 10, 0, 10, 10));
}

TEST(RectOverlap, ContainmentOverlaps) {
    // Box2 fully inside Box1.
    EXPECT_TRUE(rect_overlap(0, 0, 10, 10, 2, -2, 4, 4));
}

// Documents the FD-022 finding behind ImagesCollide: feeding screen-space
// top-left rectangles through rect_overlap with Y negated (how images_overlap
// and images_collide call it) yields the same boolean as a direct screen-space
// AABB. This is why the "mixed convention" in cb_rt_images_collide is correct.
namespace {
bool screen_space_overlap(double x1, double y1, double w1, double h1,
                          double x2, double y2, double w2, double h2) {
    return !(x1 >= x2 + w2 || x2 >= x1 + w1 ||
             y1 >= y2 + h2 || y2 >= y1 + h1);
}
}  // namespace

TEST(RectOverlap, NegatedYMatchesScreenSpace) {
    struct Case { double x1, y1, w1, h1, x2, y2, w2, h2; };
    const Case cases[] = {
        {0, 0, 10, 10, 5, 5, 10, 10},     // overlap (down-right)
        {0, 0, 10, 10, 0, 8, 10, 10},     // overlap (below)
        {0, 0, 10, 10, 50, 50, 10, 10},   // disjoint
        {0, 0, 10, 10, 0, -8, 10, 10},    // overlap (above, negative y)
        {0, 0, 10, 10, 11, 0, 10, 10},    // disjoint (just past in x)
    };
    for (const Case& c : cases) {
        bool negated = rect_overlap(c.x1, -c.y1, c.w1, c.h1,
                                    c.x2, -c.y2, c.w2, c.h2);
        bool screen = screen_space_overlap(c.x1, c.y1, c.w1, c.h1,
                                           c.x2, c.y2, c.w2, c.h2);
        EXPECT_EQ(negated, screen)
            << "mismatch for box at (" << c.x1 << "," << c.y1 << ") vs ("
            << c.x2 << "," << c.y2 << ")";
    }
}
