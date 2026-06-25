// CoolBasic runtime — string<->number conversion primitives (FD-046).
//
// The conversions that CROSS the String type, centralized here as bare
// exported symbols so the interpreter and a future native/LLVM backend share
// ONE implementation and cannot silently diverge — most critically the
// float->string formatter, where a wrong answer (3.14 vs 3.140000) would have
// nothing to catch it. These service the IR Convert/ConvertExplicit opcodes
// (Str()/Int(s$)/Float(s$) and sema's implicit String coercions); they are NOT
// CB-visible catalog functions (no CB_FN row), exactly like the core
// cb_rt_string_* primitives in cb_string.cpp.
//
// Numeric<->numeric casts (incl. Int(Float) rounding) and Hex$/Bin$/Chr$/Asc
// are deliberately NOT here — see FD-046 for the three-way boundary.
//
// IMPORTANT (FD-016 style): this TU builds/inspects strings ONLY through the
// public core primitives (cb_rt_string_from_literal/_data/_len) — like a
// plugin — and is Allegro-free, outside any CB_NO_ALLEGRO guard, so it ships
// in both the SDK-free and full builds.

#include "cb_convert.h"

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

namespace {

// Build an owning CbString (refcount 1) from a std::string. Conversion results
// are never empty (every number formats to >=1 byte), so a plain from_literal
// is enough; no empty-sentinel fast path needed.
CbString* ret_str(const std::string& r) {
    return cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(r.data()), r.size());
}

// Matches Rust's u8::is_ascii_whitespace (space, HT, LF, FF, CR — NOT vertical
// tab), so cb_rt_string_to_long trims exactly like the interpreter did.
inline bool is_ws(uint8_t b) {
    return b == ' ' || b == '\t' || b == '\n' || b == '\f' || b == '\r';
}

// Float -> String, CoolBasic's 6-significant-digit format (decoded empirically
// from the original runtime; see FD-046).
//
//   - 6 significant digits.
//   - Fixed-point iff the decimal exponent E = floor(log10|x|) is in [-3, 7];
//     scientific otherwise.
//   - Fixed: strip trailing fractional zeros but always keep >=1 (4.0 -> "4.0").
//   - Scientific: m.e±EEE — lowercase e, always-signed 3-digit exponent, the
//     mantissa's trailing zeros stripped with the '.' kept ("1.e+008").
//   - Decimal separator '.'; -0.0 -> "0.0".
//
// Implementation: snprintf("%.5e") already rounds to exactly 6 sig figs, so the
// rounding precedes the fixed/sci decision (matching 99999990.0 -> "1.e+008").
// We then reconstruct from the 6 mantissa digits + the integer exponent,
// reading both locale- and platform-independently (we never depend on the
// decimal separator char or the platform's exponent width).
std::string format_float(double x) {
    if (x == 0.0) return "0.0"; // also covers -0.0 (compares equal, no minus)
    if (std::isnan(x)) return "NaN";
    if (std::isinf(x)) return x < 0 ? "-Inf" : "Inf";

    char buf[40];
    std::snprintf(buf, sizeof buf, "%.5e", x); // "[-]d.ddddde±XX[X]"

    int p = 0;
    bool neg = false;
    if (buf[p] == '-') { neg = true; ++p; }
    else if (buf[p] == '+') { ++p; }

    // The 6 mantissa digits, skipping the separator (whatever char it is).
    char digits[6];
    int nd = 0;
    while (buf[p] != '\0' && buf[p] != 'e' && buf[p] != 'E' && nd < 6) {
        if (buf[p] >= '0' && buf[p] <= '9') digits[nd++] = buf[p];
        ++p;
    }
    while (nd < 6) digits[nd++] = '0'; // defensive; %.5e always yields 6
    std::string D(digits, 6);

    // The exponent: advance to 'e'/'E', then atoi handles its sign + digits.
    while (buf[p] != '\0' && buf[p] != 'e' && buf[p] != 'E') ++p;
    int E = 0;
    if (buf[p] == 'e' || buf[p] == 'E') E = std::atoi(buf + p + 1);

    std::string out;
    if (E >= -3 && E <= 7) {
        // Fixed-point.
        if (E >= 5) {
            // 5..7: all six digits are integer, plus (E-5) trailing zeros.
            out = D;
            out.append(static_cast<std::size_t>(E - 5), '0');
            out += ".0";
        } else if (E >= 0) {
            // 0..4: the decimal point falls inside the six digits.
            std::string ip = D.substr(0, static_cast<std::size_t>(E) + 1);
            std::string fp = D.substr(static_cast<std::size_t>(E) + 1);
            std::size_t last = fp.find_last_not_of('0');
            fp = (last == std::string::npos) ? std::string("0") : fp.substr(0, last + 1);
            out = ip + "." + fp;
        } else {
            // -3..-1: |x| < 1, so "0." then (-E-1) leading zeros then the digits.
            std::string fp(static_cast<std::size_t>(-E - 1), '0');
            fp += D; // D[0] is nonzero (normalized), so a stripping point exists
            std::size_t last = fp.find_last_not_of('0');
            fp = fp.substr(0, last + 1);
            out = "0." + fp;
        }
    } else {
        // Scientific: m.e±EEE.
        std::string frac = D.substr(1); // 5 digits
        std::size_t last = frac.find_last_not_of('0');
        frac = (last == std::string::npos) ? std::string() : frac.substr(0, last + 1);
        std::string mant = D.substr(0, 1) + "." + frac; // "1." when frac empty

        char eb[8];
        int ae = E < 0 ? -E : E;
        std::snprintf(eb, sizeof eb, "e%c%03d", E < 0 ? '-' : '+', ae);
        out = mant + eb;
    }

    if (neg) out = "-" + out;
    return out;
}

} // namespace

// ─── number -> String ────────────────────────────────────────────────────

extern "C" CbString* cb_rt_int_to_string(int32_t v) {
    char buf[16]; // INT32_MIN is 11 chars
    int n = std::snprintf(buf, sizeof buf, "%d", v);
    return cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(buf),
                                     static_cast<std::size_t>(n));
}

extern "C" CbString* cb_rt_long_to_string(int64_t v) {
    char buf[24]; // INT64_MIN is 20 chars
    int n = std::snprintf(buf, sizeof buf, "%lld", static_cast<long long>(v));
    return cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(buf),
                                     static_cast<std::size_t>(n));
}

extern "C" CbString* cb_rt_float_to_string(double v) {
    return ret_str(format_float(v));
}

// ─── String -> number ────────────────────────────────────────────────────

// Lenient leading-integer parse (a direct port of the interpreter's former
// `parse_leading_int`): skip ASCII whitespace, optional +/-, consume digits up
// to the first non-digit (INCLUDING '.', so "22.5" -> 22 and "1e3" -> 1),
// saturating at INT64_MAX/MIN+1, 0 when no digit leads. This intentionally
// diverges from CB's exact-".5"-rounds-up-positives quirk (FD-046 §decision).
extern "C" int64_t cb_rt_string_to_long(const CbString* s) {
    const uint8_t* data = cb_rt_string_data(s);
    std::size_t len = cb_rt_string_len(s);

    std::size_t i = 0;
    while (i < len && is_ws(data[i])) ++i;

    bool neg = false;
    if (i < len && (data[i] == '+' || data[i] == '-')) {
        neg = data[i] == '-';
        ++i;
    }

    std::size_t start = i;
    int64_t val = 0;
    while (i < len && data[i] >= '0' && data[i] <= '9') {
        int64_t d = data[i] - '0';
        // Saturating val*10 + d (mirrors Rust saturating_mul/saturating_add).
        if (val > (INT64_MAX - d) / 10) {
            val = INT64_MAX;
        } else {
            val = val * 10 + d;
        }
        ++i;
    }
    if (i == start) return 0; // no leading digits
    return neg ? -val : val;
}

// Lenient strtod-style prefix parse: skip leading whitespace, optional sign,
// parse a float INCLUDING exponent, stop at the first invalid char, 0.0 on no
// valid prefix ("22yo" -> 22.0, "1.5e2" -> 150.0, "1,5" -> 1.0). Replaces the
// interpreter's former strict full-parse (which gave 0.0 on any trailing junk).
extern "C" double cb_rt_string_to_float(const CbString* s) {
    std::size_t len = cb_rt_string_len(s);
    if (len == 0) return 0.0;
    // strtod needs a NUL-terminated buffer; the inline bytes are not guaranteed
    // terminated, so copy. (strtod stops at the first unparseable byte anyway.)
    std::string tmp(reinterpret_cast<const char*>(cb_rt_string_data(s)), len);
    return std::strtod(tmp.c_str(), nullptr);
}
