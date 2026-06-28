#ifndef CB_OBJECT_DATA_H
#define CB_OBJECT_DATA_H

// Pure object math for the sprite-Object subsystem. Header-only
// and Allegro-free so the angle/distance/heading/frame-slice/size/turn/animation/
// life helpers can be unit-tested without a display (mirrors cb_camera_math.h /
// cb_map_data.h). cb_object.cpp wraps a live CbObject (Allegro bitmap + registry)
// around these; everything that does NOT touch a bitmap lives here.
//
// The Rust port may pick its own in-memory layout; only the observable
// behaviour must match CoolBasic. Angles are degrees, 0° = right, growing
// clockwise in screen terms (Y is flipped at draw time, as in cb_map).
//
// Boundary rule: anything touching an ALLEGRO_BITMAP stays out of this header.

#include <cmath>
#include <cstdint>

inline constexpr double cb_object_pi = 3.14159265358979323846;

// ─── Angle / distance between two object centres ────────────────────────
// GetAngle2(a, b) and PointObject(a→b) share one formula: the angle from `a` to
// `b` in degrees. Note the mixed signs and the /π·180 (degrees) — reproduced
// verbatim so headings match CoolBasic exactly.
inline double cb_object_angle2(double ax, double ay, double bx, double by) {
    return (cb_object_pi - std::atan2(-ay + by, ax - bx)) / cb_object_pi * 180.0;
}

// Distance2(a, b): plain Euclidean distance between the two centres. Symmetric.
inline double cb_object_distance2(double ax, double ay, double bx, double by) {
    double dx = bx - ax;
    double dy = by - ay;
    return std::sqrt(dx * dx + dy * dy);
}

// ─── MoveObject heading ─────────────────────────────────────────────────
// Advances along the object's facing angle: `forward` along the heading,
// `side` perpendicular (heading − 90°). Returns the world delta to add to the
// position. Y is NOT flipped here — the caller stores raw world coordinates.
inline void cb_object_move_delta(double angle_deg, double forward, double side,
                                 double& dx, double& dy) {
    double a = angle_deg / 180.0 * cb_object_pi;
    double a90 = (angle_deg - 90.0) / 180.0 * cb_object_pi;
    dx = std::cos(a) * forward + std::cos(a90) * side;
    dy = std::sin(a) * forward + std::sin(a90) * side;
}

// ─── TurnObject 0..360 wrap ─────────────────────────────────────────────
// Relative one-shot rotate; keeps the angle in [0, 360]. Faithful to CoolBasic's
// `>360` / `<0` while-loops (note: not `>=360`).
inline double cb_object_turn(double angle_deg, double speed) {
    double a = angle_deg + speed;
    if (a < 0.0) {
        while (a < 0.0) a += 360.0;
    } else if (a > 360.0) {
        while (a > 360.0) a -= 360.0;
    }
    return a;
}

// ─── Animated-frame slice ───────────────────────────────────────────────
// Source cell for `frame` of an animated object sheet. `framesX = textureW /
// frameW`; `col = frame % framesX`; `row = frame / framesX` (the object frame
// slice is correct as written — unlike the image-frame slice in cb_gfx.cpp, which
// needed the /framesX, *frameHeight fix). `frame` is 0-based and taken
// modulo framesX. Returns false (and leaves the outputs 0) for a degenerate
// sheet so the caller draws nothing rather than dividing by zero.
inline bool cb_object_frame_slice(int32_t texture_w, int32_t frame_w,
                                  int32_t frame_h, int32_t frame, int32_t& col,
                                  int32_t& row, int32_t& left, int32_t& top) {
    col = row = left = top = 0;
    if (frame_w <= 0 || frame_h <= 0) return false;
    int32_t frames_x = texture_w / frame_w;
    if (frames_x <= 0) return false;
    col = frame % frames_x;
    row = frame / frames_x;
    left = col * frame_w;
    top = row * frame_h;
    return true;
}

// ─── ObjectSizeX / ObjectSizeY ──────────────────────────────────────────
// Bounding extent accounting for rotation. At angle 0 the stored size is
// returned untouched; otherwise the rotated AABB |w·cosθ| + |h·sinθ| is rounded
// to the nearest int (CoolBasic's `int32_t(size + 0.5f)`).
inline int32_t cb_object_size_x(double size_x, double size_y, double angle_deg) {
    if (angle_deg == 0.0) return (int32_t)size_x;
    double a = angle_deg / 180.0 * cb_object_pi;
    double s = std::fabs(size_x * std::cos(a)) + std::fabs(std::sin(a) * size_y);
    return (int32_t)(s + 0.5);
}

inline int32_t cb_object_size_y(double size_x, double size_y, double angle_deg) {
    if (angle_deg == 0.0) return (int32_t)size_y;
    double a = angle_deg / 180.0 * cb_object_pi;
    double s = std::fabs(size_y * std::cos(a)) + std::fabs(std::sin(a) * size_x);
    return (int32_t)(s + 0.5);
}

// ─── Animation advance + life ───────────────────────────────────────────
// The per-update-tick state the game loop will drive. Pinned here now so
// the behaviour is regression-locked before the loop exists. The fields mirror
// CoolBasic's per-object animation members.
struct CbAnimState {
    double current_frame = 0.0;
    int32_t anim_start_frame = 0;
    int32_t anim_ending_frame = 0;
    double anim_speed = 0.0;
    bool anim_looping = false;
    bool playing = false;
};

// One animation step. Forward (start < end) steps up and wraps to start when it
// passes the end; reverse (start > end) steps down and wraps to end. A non-
// looping animation stops (playing=false) at the wrap. A no-op when not playing.
inline void cb_object_anim_advance(CbAnimState& s) {
    if (!s.playing) return;
    if (s.anim_start_frame < s.anim_ending_frame) {
        s.current_frame += s.anim_speed;
        if ((int32_t)s.current_frame > s.anim_ending_frame) {
            s.current_frame = s.anim_start_frame;
            if (!s.anim_looping) s.playing = false;
        }
    } else {
        s.current_frame -= s.anim_speed;
        if ((int32_t)s.current_frame < s.anim_start_frame) {
            s.current_frame = s.anim_ending_frame;
            if (!s.anim_looping) s.playing = false;
        }
    }
}

// Decrements an object's life by one tick; returns true when it hits 0 (the
// object should be auto-deleted). Faithful to CoolBasic's `--life; if (life <=
// 0)` on an unsigned counter — a life that starts at 0 underflows and does NOT
// delete (a CoolBasic quirk preserved deliberately).
inline bool cb_object_life_tick(uint32_t& life) {
    --life;
    return life == 0;
}

#endif  // CB_OBJECT_DATA_H
