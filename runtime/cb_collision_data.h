#ifndef CB_COLLISION_DATA_H
#define CB_COLLISION_DATA_H

// Pure collision + picking geometry for the sprite-Object subsystem (FD-036
// Phase 5). Header-only and Allegro-free so the overlap predicates, the box/
// circle collision-resolution math, the contact-angle formula, and the object
// raycast/point tests can be unit-tested without a display (mirrors
// cb_object_data.h / cb_camera_math.h / cb_map_data.h). cb_object.cpp wraps the
// live registry, the map-grid orchestration (Rect/CircleMap), and the catalog
// entry points around these helpers; everything that does NOT touch a bitmap or
// the tilemap grid lives here.
//
// Ported from cbEnchanted's CollisionCheck (src/collisioncheck.cpp) + the
// CBObject ray/pick helpers (src/cbobject.cpp). The Rust port may pick its own
// in-memory layout; only the observable behaviour must match. World coordinates
// are Y-up (the camera flips Y at draw time); angles are degrees.
//
// Boundary rule: anything touching an ALLEGRO_BITMAP or the active tilemap grid
// stays out of this header (the Rect/CircleMap tile loops live in cb_object.cpp).

#include "cb_geom.h"  // rect_overlap == cbEnchanted's static RectRectTest

#include <cmath>
#include <cstdint>

inline constexpr double cb_collision_pi = 3.14159265358979323846;

// ─── Contact-angle formula (collisioncheck.cpp:198) ─────────────────────
// cbEnchanted maps a contact normal's atan2 result (rad in [-π, π]) to degrees
// with `((rad + π) / π) * 180` — note the `/π`, NOT `/2π`. Programs read these
// angles, so reproduce it verbatim. Range: [0, 360].
inline double cb_collision_angle_deg(double rad) {
    return ((rad + cb_collision_pi) / cb_collision_pi) * 180.0;
}

// ─── Overlap predicates ─────────────────────────────────────────────────
// CircleCircleTest (collisioncheck.cpp:825): squared-distance < squared-sum.
inline bool cb_circle_circle_overlap(double x1, double y1, double r1, double x2,
                                     double y2, double r2) {
    double dx = x2 - x1;
    double dy = y2 - y1;
    double min_dist = r1 + r2;
    return dx * dx + dy * dy < min_dist * min_dist;
}

// CircleRectTest (collisioncheck.cpp:797, http://stackoverflow.com/a/402010).
// The rect is given by its top-left corner (rectX, rectY) + size; the circle by
// its centre + radius. Epsilon matches cbEnchanted so shared edges agree.
inline bool cb_circle_rect_overlap(double circle_x, double circle_y,
                                   double circle_r, double rect_x, double rect_y,
                                   double rect_w, double rect_h) {
    double half_w = rect_w / 2.0;
    double half_h = rect_h / 2.0;
    constexpr double eps = 1e-5;
    double dist_x = std::fabs(circle_x - rect_x - half_w);
    double dist_y = std::fabs(circle_y - rect_y - half_h);
    if (dist_x > half_w + circle_r - eps) return false;
    if (dist_y > half_h + circle_r - eps) return false;
    if (dist_x <= half_w + eps) return true;
    if (dist_y <= half_h + eps) return true;
    double corner_sq =
        (dist_x - half_w) * (dist_x - half_w) + (dist_y - half_h) * (dist_y - half_h);
    return corner_sq <= circle_r * circle_r + eps;
}

// ─── Collision-resolution results ───────────────────────────────────────
// A single recorded contact: the normal angle (degrees) and the contact point.
// The caller fills in the "other object" handle (this header is object-agnostic).
struct CbContact {
    double angle = 0.0;
    double x = 0.0;
    double y = 0.0;
};

// Box-box resolves up to two contacts (an X-pass and a Y-pass); circle-circle
// at most one. `objX/objY` is the post-resolution position the caller writes
// back to the object (and stores as the new safe position).
struct CbBoxResolve {
    double objX = 0.0;
    double objY = 0.0;
    int hitCount = 0;
    CbContact hits[2];
};

struct CbCircleResolve {
    double objX = 0.0;
    double objY = 0.0;
    int hitCount = 0;
    CbContact hit;
};

// ─── Box-box resolution (collisioncheck.cpp:173-245) ────────────────────
// Object 1 (objX/objY, objW=range1, objH=range2) vs collider (cObjX/cObjY,
// cObjW/cObjH). The X-pass tests against the stored `safeY`, the Y-pass against
// the freshly-resolved objY — faithful to cbEnchanted's separate-axis pushout.
// The "hacky fix" averages the half-sums into the overlap-test extents.
inline CbBoxResolve cb_box_box_resolve(double objX, double objY, double safeY,
                                       double objW, double objH, double cObjX,
                                       double cObjY, double cObjW, double cObjH) {
    CbBoxResolve out;
    double chckW = (objW + cObjW) / 2.0;
    double chckH = (objH + cObjH) / 2.0;

    // X-direction.
    if (rect_overlap(objX, safeY, chckW, chckH, cObjX, cObjY, chckW, chckH)) {
        double ang = cb_collision_angle_deg(std::atan2(cObjY - safeY, cObjX - objX));
        if (objX > cObjX) {  // collider to the left
            objX = cObjX + cObjW / 2.0 + objW / 2.0 + 1.0;
            out.hits[out.hitCount++] = {ang, objX - objW / 2.0 - 1.0, objY};
        } else {  // collider to the right
            objX = cObjX - cObjW / 2.0 - objW / 2.0 - 1.0;
            out.hits[out.hitCount++] = {ang, objX + objW / 2.0 + 1.0, objY};
        }
    }

    // Y-direction.
    if (rect_overlap(objX, objY, chckW, chckH, cObjX, cObjY, chckW, chckH)) {
        double ang = cb_collision_angle_deg(std::atan2(cObjY - objY, cObjX - objX));
        if (objY > cObjY) {  // collider below
            objY = cObjY + cObjH / 2.0 + objH / 2.0 + 1.0;
            out.hits[out.hitCount++] = {ang, objX, objY - objH / 2.0 - 1.0};
        } else {  // collider above
            objY = cObjY - cObjH / 2.0 - objH / 2.0 - 1.0;
            out.hits[out.hitCount++] = {ang, objX, objY + objH / 2.0 + 1.0};
        }
    }

    out.objX = objX;
    out.objY = objY;
    return out;
}

// ─── Circle-circle resolution (collisioncheck.cpp:248-297) ──────────────
// Radii `r1`/`r2` are already halved (range1/2). Stop measures the push-back
// angle from the last safe position (straight push-back); Slide measures it from
// the current position (tangential slide). Both push obj1 to r1+r2+0.5 from obj2;
// the contact point sits r1+1 out from the resolved centre.
inline CbCircleResolve cb_circle_circle_resolve(double objX, double objY,
                                                double safeX, double safeY,
                                                double r1, double oX, double oY,
                                                double r2, bool is_stop) {
    CbCircleResolve out;
    double dx = oX - objX;
    double dy = oY - objY;
    double dist = dx * dx + dy * dy;
    double min_dist = r1 + r2;
    if (dist < min_dist * min_dist) {
        double rad = is_stop ? std::atan2(oY - safeY, oX - safeX) : std::atan2(dy, dx);
        objX = oX - std::cos(rad) * (r1 + r2 + 0.5);
        objY = oY - std::sin(rad) * (r1 + r2 + 0.5);
        out.hit = {cb_collision_angle_deg(rad), objX + std::cos(rad) * (r1 + 1.0),
                   objY + std::sin(rad) * (r1 + 1.0)};
        out.hitCount = 1;
    }
    out.objX = objX;
    out.objY = objY;
    return out;
}

#endif  // CB_COLLISION_DATA_H
