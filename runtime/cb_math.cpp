// CoolBasic math runtime (FD-013), C++20.
//
// CoolBasic semantics honored here:
//   - Trigonometric functions work in DEGREES, not radians. Sin/Cos/Tan
//     convert their argument deg->rad; ASin/ACos/ATan convert their result
//     rad->deg.
//   - Rnd/Rand/Randomize expose a seedable PRNG. Rnd is [0,max) / [low,high);
//     Rand is inclusive [0,max] / [low,high]. Without a Randomize call the
//     sequence is reproducible (fixed default seed).
//   - GetAngle/WrapAngle return degrees in [0,360).
//
// Implementation choices:
//   - PI comes from std::numbers::pi rather than a hand-written literal.
//   - Randomness uses a Mersenne Twister (std::mt19937_64) with
//     std::uniform_*_distribution, avoiding the modulo bias and short period
//     of a `rand() % n` approach.
//   - Everything computes in `double`: the catalog maps both `float` and
//     `double` to CB_TYPE_FLOAT and CB's Float is an f64 at the interpreter
//     boundary, so narrowing to 32-bit float would needlessly lose precision.

#include "cb_runtime.h"

#include <cmath>
#include <cstdint>
#include <numbers>
#include <random>

namespace {

inline double to_rad(double deg) { return deg * std::numbers::pi / 180.0; }
inline double to_deg(double rad) { return rad * 180.0 / std::numbers::pi; }

inline double square(double a) { return a * a; }

// Process-wide PRNG. Seeded with a fixed constant so that, absent a
// Randomize() call, programs are reproducible run-to-run (CoolBasic's
// default-seed behaviour). Randomize() reseeds it.
std::mt19937_64& rng() {
    static std::mt19937_64 engine{0x9E3779B97F4A7C15ULL};
    return engine;
}

} // namespace

// ─── Trigonometry (degrees) ───────────────────────────────────────────

extern "C" double cb_rt_sin(double deg) { return std::sin(to_rad(deg)); }
extern "C" double cb_rt_cos(double deg) { return std::cos(to_rad(deg)); }
extern "C" double cb_rt_tan(double deg) { return std::tan(to_rad(deg)); }

extern "C" double cb_rt_asin(double x) { return to_deg(std::asin(x)); }
extern "C" double cb_rt_acos(double x) { return to_deg(std::acos(x)); }
extern "C" double cb_rt_atan(double x) { return to_deg(std::atan(x)); }

// ─── General math ─────────────────────────────────────────────────────

extern "C" double cb_rt_sqrt(double x) { return std::sqrt(x); }
extern "C" double cb_rt_log(double x) { return std::log(x); }
extern "C" double cb_rt_log10(double x) { return std::log10(x); }

extern "C" int32_t cb_rt_round_up(double x) { return static_cast<int32_t>(std::ceil(x)); }
extern "C" int32_t cb_rt_round_down(double x) { return static_cast<int32_t>(std::floor(x)); }

// ─── Min / Max (int and float overloads) ──────────────────────────────

extern "C" int32_t cb_rt_max_int(int32_t a, int32_t b) { return a > b ? a : b; }
extern "C" int32_t cb_rt_min_int(int32_t a, int32_t b) { return a < b ? a : b; }
extern "C" double  cb_rt_max_float(double a, double b) { return a > b ? a : b; }
extern "C" double  cb_rt_min_float(double a, double b) { return a < b ? a : b; }

// ─── Geometry ─────────────────────────────────────────────────────────

extern "C" double cb_rt_distance(double x1, double y1, double x2, double y2) {
    return std::sqrt(square(x1 - x2) + square(y1 - y2));
}

extern "C" double cb_rt_get_angle(double x1, double y1, double x2, double y2) {
    return to_deg(std::numbers::pi - std::atan2(y1 - y2, x1 - x2));
}

extern "C" double cb_rt_wrap_angle(double a) {
    a = std::fmod(a, 360.0);
    if (a < 0.0) {
        a += 360.0;
    }
    return a;
}

// ─── Curves & overlap (FD-017) ────────────────────────────────────────
//
// CurveValue/CurveAngle ease `current` toward `target` by `1/smoothness` of
// the gap each call. CurveAngle takes the shortest path around the 360° wrap.
// BoxOverlap is an AABB intersection test run in world space (Y up), so Y is
// negated before the rect test — observable only at the rectangle edges.

extern "C" double cb_rt_curve_value(double target, double current, double smoothness) {
    return current + (target - current) / smoothness;
}

extern "C" double cb_rt_curve_angle(double target, double current, double smoothness) {
    double diff = current - target;
    while (diff > 180.0) diff -= 360.0;
    while (diff < -180.0) diff += 360.0;
    return cb_rt_wrap_angle(current - diff / smoothness);
}

namespace {
// AABB overlap. Each box is (left=x, right=x+w, bottom=y, top=y-h); the 1e-5
// epsilon keeps shared edges from registering as overlaps.
bool rect_rect_test(double x1, double y1, double w1, double h1,
                    double x2, double y2, double w2, double h2) {
    constexpr double eps = 1e-5;
    double left1 = x1, right1 = x1 + w1, top1 = y1 - h1, bottom1 = y1;
    double left2 = x2, right2 = x2 + w2, top2 = y2 - h2, bottom2 = y2;
    if (bottom1 < top2 + eps) return false;
    if (top1 > bottom2 - eps) return false;
    if (right1 < left2 + eps) return false;
    if (left1 > right2 - eps) return false;
    return true;
}
} // namespace

extern "C" int32_t cb_rt_box_overlap(double x1, double y1, double w1, double h1,
                                     double x2, double y2, double w2, double h2) {
    // Negate Y so the test runs in world space (Y up); see rect_rect_test.
    return rect_rect_test(x1, -y1, w1, h1, x2, -y2, w2, h2) ? 1 : 0;
}

// ─── Random ───────────────────────────────────────────────────────────
//
// Random semantics follow CoolBasic exactly (FD-017): `randf()` is [0,1);
// `rand(n)` is uniform_int over the INCLUSIVE [0,n]. Hence Rand(low,high) is
// inclusive [low,high] and Rnd(low,high) is [low,high). The `high < low` branch
// is the documented special case (NOT a swap): Rnd -> randf()*low,
// Rand -> rand(low). Non-deterministic across seeds by design, so fixtures
// assert ranges only.

namespace {
double randf() { return std::uniform_real_distribution<double>(0.0, 1.0)(rng()); }

// uniform_int over inclusive [0, n]; n <= 0 -> 0 (n==0 is trivially 0; the
// guard also keeps a negative bound out of the distribution, which would
// otherwise be undefined).
int32_t rand_n(int32_t n) {
    if (n <= 0) return 0;
    return std::uniform_int_distribution<int32_t>(0, n)(rng());
}
} // namespace

extern "C" double cb_rt_rnd_max(double max) {
    // Rnd(max) == Rnd(0, max): max<0 hits the high<low branch -> randf()*0 == 0.
    if (max < 0.0) return 0.0;
    return randf() * max;  // [0, max)
}

extern "C" double cb_rt_rnd_range(double low, double high) {
    if (high < low) return randf() * low;       // special case (not a swap)
    return low + randf() * (high - low);         // [low, high)
}

extern "C" int32_t cb_rt_rand_max(int32_t max) {
    return rand_n(max);  // inclusive [0, max]
}

extern "C" int32_t cb_rt_rand_range(int32_t low, int32_t high) {
    if (high < low) return rand_n(low);          // special case -> [0, low]
    return low + rand_n(high - low);             // inclusive [low, high]
}

extern "C" void cb_rt_randomize(int32_t seed) { rng().seed(static_cast<uint64_t>(static_cast<uint32_t>(seed))); }
