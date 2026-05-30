// CoolBasic math runtime — ported from ../CBCompiler/Runtime/cb_math.cpp
// and mathinterface.cpp, modernized for C++20.
//
// Semantics preserved from the legacy implementation:
//   - Trigonometric functions work in DEGREES, not radians (a CoolBasic
//     quirk). Sin/Cos/Tan convert their argument deg->rad; ASin/ACos/ATan
//     convert their result rad->deg.
//   - Rnd/Rand/Randomize expose a seedable PRNG. Rnd is [0,max) / [min,max);
//     Rand is [0,max) / [min,max). Without a Randomize call the sequence is
//     reproducible (fixed default seed), matching legacy determinism.
//   - GetAngle/WrapAngle return degrees in [0,360).
//
// Improvements over the legacy port:
//   - PI comes from std::numbers::pi rather than a hand-written literal.
//   - Randomness uses a Mersenne Twister (std::mt19937_64) with
//     std::uniform_*_distribution, avoiding the modulo bias and short period
//     of the legacy `rand() % n` approach.
//
// The catalog maps both `float` and `double` to CB_TYPE_FLOAT; CB's Float
// is an f64 at the interpreter boundary, so every function here uses
// `double` (the legacy code used 32-bit float, which would needlessly lose
// precision against our wider ABI).

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
// Randomize() call, programs are reproducible run-to-run (matching the
// legacy default-seed behaviour). Randomize() reseeds it.
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

// ─── Random ───────────────────────────────────────────────────────────
//
// Non-deterministic across seeds by design, so the fixture suite exercises
// these only via range assertions, never golden values.

extern "C" double cb_rt_rnd_max(double max) {
    if (max <= 0.0) {
        return 0.0;
    }
    return std::uniform_real_distribution<double>(0.0, max)(rng());
}

extern "C" double cb_rt_rnd_range(double min, double max) {
    if (max <= min) {
        return min;
    }
    return std::uniform_real_distribution<double>(min, max)(rng());
}

extern "C" int32_t cb_rt_rand_max(int32_t max) {
    if (max <= 0) {
        return 0;
    }
    // [0, max) == inclusive [0, max-1].
    return std::uniform_int_distribution<int32_t>(0, max - 1)(rng());
}

extern "C" int32_t cb_rt_rand_range(int32_t min, int32_t max) {
    if (max <= min) {
        return min;
    }
    // [min, max) == inclusive [min, max-1].
    return std::uniform_int_distribution<int32_t>(min, max - 1)(rng());
}

extern "C" void cb_rt_randomize(int32_t seed) { rng().seed(static_cast<uint64_t>(static_cast<uint32_t>(seed))); }
