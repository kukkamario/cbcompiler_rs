#ifndef CB_GEOM_H
#define CB_GEOM_H

// Pure geometry helpers shared by the graphics runtime (cb_gfx.cpp) and the
// native C++ unit tests. Header-only (inline) so this Allegro-free logic can be
// exercised without linking cb_gfx.cpp or Allegro.

// AABB overlap helper (CoolBasic's RectRectTest: box = left=x, right=x+w,
// top=y-h, bottom=y; epsilon keeps shared edges from counting).
inline bool rect_overlap(double x1, double y1, double w1, double h1,
                         double x2, double y2, double w2, double h2) {
    constexpr double eps = 1e-5;
    double l1 = x1, r1 = x1 + w1, t1 = y1 - h1, b1 = y1;
    double l2 = x2, r2 = x2 + w2, t2 = y2 - h2, b2 = y2;
    if (b1 < t2 + eps) return false;
    if (t1 > b2 - eps) return false;
    if (r1 < l2 + eps) return false;
    if (l1 > r2 - eps) return false;
    return true;
}

#endif  // CB_GEOM_H
