// FD-046: unit tests for the string<->number conversion primitives
// (cb_convert.cpp). These drive the bare extern "C" cb_rt_*_to_string /
// cb_rt_string_to_* symbols directly. No display / Allegro / trap host is
// touched. They are the source of truth for the format spec decoded from the
// original CoolBasic runtime, and the interpreter is required to produce
// byte-identical output (it now calls these same symbols).

#include "cb_convert.h" // also pulls cb_runtime_core.h (CbString + string prims)

#include <gtest/gtest.h>

#include <cmath>
#include <cstdint>
#include <limits>
#include <string>

namespace {

// RAII CbString from raw bytes (so embedded NULs survive).
struct Str {
    CbString* s;
    explicit Str(const std::string& v)
        : s(cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(v.data()), v.size())) {}
    ~Str() { cb_rt_string_release(s); }
    Str(const Str&) = delete;
    Str& operator=(const Str&) = delete;
    operator const CbString*() const { return s; }
};

// Consume an owned CbString return value into a std::string (releases it).
std::string take(CbString* s) {
    std::string r(reinterpret_cast<const char*>(cb_rt_string_data(s)), cb_rt_string_len(s));
    cb_rt_string_release(s);
    return r;
}

int64_t to_long(const std::string& v) {
    Str s(v);
    return cb_rt_string_to_long(s);
}

double to_float(const std::string& v) {
    Str s(v);
    return cb_rt_string_to_float(s);
}

} // namespace

// ─── number -> String ────────────────────────────────────────────────────

TEST(Convert, IntToString) {
    EXPECT_EQ(take(cb_rt_int_to_string(0)), "0");
    EXPECT_EQ(take(cb_rt_int_to_string(42)), "42");
    EXPECT_EQ(take(cb_rt_int_to_string(-7)), "-7");
    EXPECT_EQ(take(cb_rt_int_to_string(INT32_MIN)), "-2147483648");
    EXPECT_EQ(take(cb_rt_int_to_string(INT32_MAX)), "2147483647");
}

TEST(Convert, LongToString) {
    EXPECT_EQ(take(cb_rt_long_to_string(0)), "0");
    EXPECT_EQ(take(cb_rt_long_to_string(-7)), "-7");
    EXPECT_EQ(take(cb_rt_long_to_string(INT64_C(9223372036854775807))), "9223372036854775807");
    EXPECT_EQ(take(cb_rt_long_to_string(INT64_MIN)), "-9223372036854775808");
}

// The load-bearing one: 6-significant-digit CB float format.
TEST(Convert, FloatToStringFixedPoint) {
    EXPECT_EQ(take(cb_rt_float_to_string(4.0)), "4.0");
    EXPECT_EQ(take(cb_rt_float_to_string(100.0)), "100.0");
    EXPECT_EQ(take(cb_rt_float_to_string(3.14)), "3.14");
    EXPECT_EQ(take(cb_rt_float_to_string(-3.14)), "-3.14");
    EXPECT_EQ(take(cb_rt_float_to_string(1.0 / 3.0)), "0.333333");
    EXPECT_EQ(take(cb_rt_float_to_string(10.0 / 3.0)), "3.33333");
    EXPECT_EQ(take(cb_rt_float_to_string(0.001)), "0.001");
    EXPECT_EQ(take(cb_rt_float_to_string(1e7)), "10000000.0");
}

TEST(Convert, FloatToStringSixSigFigRounding) {
    EXPECT_EQ(take(cb_rt_float_to_string(1234567.0)), "1234570.0");
    EXPECT_EQ(take(cb_rt_float_to_string(12345678.0)), "12345700.0");
}

TEST(Convert, FloatToStringScientific) {
    EXPECT_EQ(take(cb_rt_float_to_string(1e8)), "1.e+008");
    EXPECT_EQ(take(cb_rt_float_to_string(0.0001)), "1.e-004");
    EXPECT_EQ(take(cb_rt_float_to_string(123456700.0)), "1.23457e+008");
    EXPECT_EQ(take(cb_rt_float_to_string(0.0001234)), "1.234e-004");
    // 6-sig-fig rounding precedes the fixed/sci decision: 9.999999e7 rounds to
    // 1e8 (E 7 -> 8), so this tips into scientific.
    EXPECT_EQ(take(cb_rt_float_to_string(99999990.0)), "1.e+008");
}

TEST(Convert, FloatToStringSpecialValues) {
    EXPECT_EQ(take(cb_rt_float_to_string(0.0)), "0.0");
    EXPECT_EQ(take(cb_rt_float_to_string(-0.0)), "0.0"); // no minus
    // Beyond the CB oracle (not probed) — pinned so interp/native stay identical.
    const double inf = std::numeric_limits<double>::infinity();
    EXPECT_EQ(take(cb_rt_float_to_string(std::numeric_limits<double>::quiet_NaN())), "NaN");
    EXPECT_EQ(take(cb_rt_float_to_string(inf)), "Inf");
    EXPECT_EQ(take(cb_rt_float_to_string(-inf)), "-Inf");
}

// ─── String -> long (lenient leading-int, clean truncation) ──────────────

TEST(Convert, StringToLong) {
    EXPECT_EQ(to_long("  -3x"), -3);
    EXPECT_EQ(to_long("+7"), 7);
    EXPECT_EQ(to_long("007"), 7);
    EXPECT_EQ(to_long("42   "), 42);
    EXPECT_EQ(to_long("3x"), 3);
    EXPECT_EQ(to_long("1e3"), 1); // stops at 'e' (unlike float)
    // Intentional divergence from CB (which rounds an exact ".5" up): we stop
    // at '.', so "22.5" -> 22, not 23.
    EXPECT_EQ(to_long("22.5"), 22);
    EXPECT_EQ(to_long("Hello"), 0);
    EXPECT_EQ(to_long(""), 0);
    EXPECT_EQ(to_long("- 6"), 0); // space after sign breaks the run
    EXPECT_EQ(to_long("0x1F"), 0); // stops at 'x' after the leading 0
}

TEST(Convert, StringToLongSaturates) {
    EXPECT_EQ(to_long("99999999999999999999999"), INT64_MAX);
    EXPECT_EQ(to_long("-99999999999999999999999"), -INT64_MAX); // == INT64_MIN + 1
}

// ─── String -> float (lenient strtod prefix parse) ───────────────────────

TEST(Convert, StringToFloat) {
    EXPECT_DOUBLE_EQ(to_float("3x"), 3.0);
    EXPECT_DOUBLE_EQ(to_float("22yo"), 22.0);
    EXPECT_DOUBLE_EQ(to_float("3.14xyz"), 3.14);
    EXPECT_DOUBLE_EQ(to_float("1.5e2"), 150.0);
    EXPECT_DOUBLE_EQ(to_float(".5"), 0.5);
    EXPECT_DOUBLE_EQ(to_float("5."), 5.0);
    EXPECT_DOUBLE_EQ(to_float("1.2.3"), 1.2);
    EXPECT_DOUBLE_EQ(to_float("+.25"), 0.25);
    EXPECT_DOUBLE_EQ(to_float("1,5"), 1.0); // comma is not a separator
    EXPECT_DOUBLE_EQ(to_float("xyz"), 0.0);
    EXPECT_DOUBLE_EQ(to_float(""), 0.0);
}
