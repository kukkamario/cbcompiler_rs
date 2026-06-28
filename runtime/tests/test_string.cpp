// Unit tests for the core string primitives added for the native
// backend — cb_rt_string_compare (the shared ordering oracle) and
// cb_rt_string_char_len (the codepoint count behind CB `Len(s$)`). These drive
// the bare extern "C" symbols directly; no display / Allegro / trap host. The
// interpreter is required to agree with both (it calls cb_rt_string_compare and
// uses the identical char-length predicate).

#include "cb_runtime_core.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <string>

namespace {

// RAII CbString from raw bytes (so embedded NULs / multibyte survive).
struct Str {
    CbString* s;
    explicit Str(const std::string& v)
        : s(cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(v.data()), v.size())) {}
    ~Str() { cb_rt_string_release(s); }
    Str(const Str&) = delete;
    Str& operator=(const Str&) = delete;
    operator const CbString*() const { return s; }
};

int cmp(const std::string& a, const std::string& b) {
    Str sa(a), sb(b);
    return cb_rt_string_compare(sa, sb);
}

} // namespace

// ─── cb_rt_string_compare ────────────────────────────────────────────────

TEST(StringCompare, EqualStringsAreZero) {
    EXPECT_EQ(cmp("hello", "hello"), 0);
    EXPECT_EQ(cmp("", ""), 0);
}

TEST(StringCompare, LexicographicByteOrder) {
    // First differing byte decides, by unsigned value (Rust slice Ord over u8).
    EXPECT_LT(cmp("abc", "abd"), 0);
    EXPECT_GT(cmp("abd", "abc"), 0);
    EXPECT_LT(cmp("Z", "a"), 0); // 'Z'(0x5A) < 'a'(0x61)
}

TEST(StringCompare, PrefixIsLessThanLonger) {
    EXPECT_LT(cmp("ab", "abc"), 0);
    EXPECT_GT(cmp("abc", "ab"), 0);
    EXPECT_LT(cmp("", "a"), 0);
    EXPECT_GT(cmp("a", ""), 0);
}

TEST(StringCompare, HighBytesAreUnsigned) {
    // A multibyte UTF-8 lead byte (0xC3 for 'ä') is > any ASCII byte: 'ä' > 'z'.
    EXPECT_GT(cmp("\xC3\xA4", "z"), 0);
}

TEST(StringCompare, NullOperandsTreatedAsEmpty) {
    Str a("x");
    EXPECT_GT(cb_rt_string_compare(a, nullptr), 0);
    EXPECT_LT(cb_rt_string_compare(nullptr, a), 0);
    EXPECT_EQ(cb_rt_string_compare(nullptr, nullptr), 0);
}

TEST(StringCompare, ResultIsNormalized) {
    // Documented contract: exactly -1 / 0 / 1.
    EXPECT_EQ(cmp("a", "b"), -1);
    EXPECT_EQ(cmp("b", "a"), 1);
}

// ─── cb_rt_string_char_len ───────────────────────────────────────────────

TEST(StringCharLen, AsciiCountsBytes) {
    Str s("hello");
    EXPECT_EQ(cb_rt_string_char_len(s), 5u);
    EXPECT_EQ(cb_rt_string_len(s), 5u);
}

TEST(StringCharLen, CountsCodepointsNotBytes) {
    // "äbc" is 3 codepoints but 4 bytes ('ä' = 0xC3 0xA4).
    Str s("\xC3\xA4""bc");
    EXPECT_EQ(cb_rt_string_char_len(s), 3u);
    EXPECT_EQ(cb_rt_string_len(s), 4u);
}

TEST(StringCharLen, EmptyAndNull) {
    Str e("");
    EXPECT_EQ(cb_rt_string_char_len(e), 0u);
    EXPECT_EQ(cb_rt_string_char_len(nullptr), 0u);
}
