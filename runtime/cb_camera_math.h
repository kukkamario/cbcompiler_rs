#ifndef CB_CAMERA_MATH_H
#define CB_CAMERA_MATH_H

// Pure 2D affine transform helpers for the camera (FD-036 Phase 2). Header-only
// and Allegro-free so the world<->screen math can be unit-tested without a
// display (mirrors cb_geom.h / test_geom.cpp). The arithmetic reproduces
// Allegro's al_*_transform composition *exactly*, so cb_camera.cpp can build an
// ALLEGRO_TRANSFORM straight from a CbAffine with no second, drift-prone path.

#include <cmath>

// Affine: x' = a*x + c*y + tx ; y' = b*x + d*y + ty. The field layout mirrors
// ALLEGRO_TRANSFORM's 2D slots: a=m[0][0], b=m[0][1], c=m[1][0], d=m[1][1],
// tx=m[3][0], ty=m[3][1] — so populating an ALLEGRO_TRANSFORM is a field copy.
struct CbAffine {
    double a, b, c, d, tx, ty;
};

inline CbAffine cb_affine_identity() {
    return CbAffine{1.0, 0.0, 0.0, 1.0, 0.0, 0.0};
}

// Post-compose a translation (al_translate_transform: adds to the translation
// row; a subsequent rotate/scale then carries it, so ops apply in call order).
inline void cb_affine_translate(CbAffine& m, double x, double y) {
    m.tx += x;
    m.ty += y;
}

// Post-compose a rotation (al_rotate_transform: rotate every row, including the
// translation row — new_r0 = r0*cos - r1*sin; new_r1 = r0*sin + r1*cos).
inline void cb_affine_rotate(CbAffine& m, double theta) {
    double co = std::cos(theta), si = std::sin(theta);
    double t;
    t = m.a;  m.a  = t * co - m.b  * si; m.b  = t * si + m.b  * co;
    t = m.c;  m.c  = t * co - m.d  * si; m.d  = t * si + m.d  * co;
    t = m.tx; m.tx = t * co - m.ty * si; m.ty = t * si + m.ty * co;
}

// Post-compose a scale (al_scale_transform: x-component of each row *= sx, the
// y-component *= sy).
inline void cb_affine_scale(CbAffine& m, double sx, double sy) {
    m.a *= sx;  m.b *= sy;
    m.c *= sx;  m.d *= sy;
    m.tx *= sx; m.ty *= sy;
}

inline void cb_affine_apply(const CbAffine& m, double& x, double& y) {
    double nx = m.a * x + m.c * y + m.tx;
    double ny = m.b * x + m.d * y + m.ty;
    x = nx;
    y = ny;
}

// Inverse of the affine (al_invert_transform's exact formula).
inline CbAffine cb_affine_invert(const CbAffine& m) {
    double det = m.a * m.d - m.c * m.b;
    CbAffine r;
    r.a = m.d / det;
    r.b = -m.b / det;
    r.c = -m.c / det;
    r.d = m.a / det;
    r.tx = (m.c * m.ty - m.d * m.tx) / det;
    r.ty = (m.b * m.tx - m.a * m.ty) / det;
    return r;
}

// The camera world transform:
// identity -> translate(-cx,+cy) -> rotate(rad) -> scale(zoom) ->
// translate(designW/2, designH/2). Centering uses the logical design resolution
// with *integer* halves (CoolBasic centers on getDefaultWidth()/2), NOT the
// window size. Y is NOT inverted here — that lives in the wrappers below.
inline CbAffine cb_build_world_transform(double cx, double cy, double rad_angle,
                                         double zoom, int design_w, int design_h) {
    CbAffine m = cb_affine_identity();
    cb_affine_translate(m, -cx, cy);
    cb_affine_rotate(m, rad_angle);
    cb_affine_scale(m, zoom, zoom);
    cb_affine_translate(m, (double)(design_w / 2), (double)(design_h / 2));
    return m;
}

// Screen<->world wrappers. The Y-flip lives here, not in the matrix:
// screen->world inverts then flips; world->screen flips then transforms.
inline void cb_screen_to_world(const CbAffine& world, double& x, double& y) {
    CbAffine inv = cb_affine_invert(world);
    cb_affine_apply(inv, x, y);
    y = -y;
}

inline void cb_world_to_screen(const CbAffine& world, double& x, double& y) {
    y = -y;
    cb_affine_apply(world, x, y);
}

// The transform fed to al_use_transform for DrawToWorld *user* draws: the world
// transform pre-composed with a Y-flip, so raw world coords map straight to the
// screen (i.e. applying this equals cb_world_to_screen). Algebraically this is
// the world transform with the c/d column negated.
//
// Divergence from CoolBasic (documented, visual-only): CoolBasic flips Y on a
// primitive's anchor point only, leaving width/height in screen orientation;
// folding the flip into the transform flips the whole primitive, giving a true
// world-space extent for Box/Ellipse. Line/Dot/Circle/image/text draws (single
// anchor or symmetric) are identical either way.
inline CbAffine cb_build_render_transform(double cx, double cy, double rad_angle,
                                          double zoom, int design_w, int design_h) {
    CbAffine m = cb_build_world_transform(cx, cy, rad_angle, zoom, design_w, design_h);
    m.c = -m.c;
    m.d = -m.d;
    return m;
}

#endif  // CB_CAMERA_MATH_H
