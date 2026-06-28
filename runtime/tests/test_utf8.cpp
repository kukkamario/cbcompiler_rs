// Unit tests for the pure UTF-8 helpers in cb_utf8.h. No display /
// Allegro needed.

#include "cb_utf8.h"

#include <gtest/gtest.h>

namespace {
const uint8_t* u8(const char* s) {
    return reinterpret_cast<const uint8_t*>(s);
}
// "héllo": h, é (U+00E9 = C3 A9), l, l, o — 6 bytes, 5 codepoints.
const uint8_t HELLO_ACCENT[] = {'h', 0xC3, 0xA9, 'l', 'l', 'o'};
}  // namespace

TEST(Utf8, CpLenCountsCodepoints) {
    EXPECT_EQ(cp_len(u8("hello"), 5), 5u);
    EXPECT_EQ(cp_len(HELLO_ACCENT, sizeof(HELLO_ACCENT)), 5u);
    const uint8_t euro[] = {0xE2, 0x82, 0xAC};  // U+20AC, 3 bytes, 1 cp
    EXPECT_EQ(cp_len(euro, sizeof(euro)), 1u);
    EXPECT_EQ(cp_len(u8(""), 0), 0u);
}

TEST(Utf8, ByteOffsetOfCp) {
    const std::size_t n = sizeof(HELLO_ACCENT);
    EXPECT_EQ(byte_offset_of_cp(HELLO_ACCENT, n, 0), 0u);  // 'h'
    EXPECT_EQ(byte_offset_of_cp(HELLO_ACCENT, n, 1), 1u);  // 'é' lead byte
    EXPECT_EQ(byte_offset_of_cp(HELLO_ACCENT, n, 2), 3u);  // first 'l' after 'é'
    EXPECT_EQ(byte_offset_of_cp(HELLO_ACCENT, n, 4), 5u);  // 'o'
    // cp_index at or past the codepoint count clamps to byte_len.
    EXPECT_EQ(byte_offset_of_cp(HELLO_ACCENT, n, 5), n);
    EXPECT_EQ(byte_offset_of_cp(HELLO_ACCENT, n, 99), n);
}

TEST(Utf8, EncodeAscii) {
    uint8_t out[4] = {0};
    EXPECT_EQ(encode_utf8('A', out), 1u);
    EXPECT_EQ(out[0], static_cast<uint8_t>('A'));
}

TEST(Utf8, EncodeTwoByte) {
    uint8_t out[4] = {0};
    EXPECT_EQ(encode_utf8(0x00E9, out), 2u);  // é
    EXPECT_EQ(out[0], static_cast<uint8_t>(0xC3));
    EXPECT_EQ(out[1], static_cast<uint8_t>(0xA9));
}

TEST(Utf8, EncodeThreeByte) {
    uint8_t out[4] = {0};
    EXPECT_EQ(encode_utf8(0x20AC, out), 3u);  // €
    EXPECT_EQ(out[0], static_cast<uint8_t>(0xE2));
    EXPECT_EQ(out[1], static_cast<uint8_t>(0x82));
    EXPECT_EQ(out[2], static_cast<uint8_t>(0xAC));
}

TEST(Utf8, EncodeFourByte) {
    uint8_t out[4] = {0};
    EXPECT_EQ(encode_utf8(0x1F600, out), 4u);  // 😀
    EXPECT_EQ(out[0], static_cast<uint8_t>(0xF0));
    EXPECT_EQ(out[1], static_cast<uint8_t>(0x9F));
    EXPECT_EQ(out[2], static_cast<uint8_t>(0x98));
    EXPECT_EQ(out[3], static_cast<uint8_t>(0x80));
}

TEST(Utf8, EncodeRejectsInvalid) {
    uint8_t out[4] = {0};
    EXPECT_EQ(encode_utf8(-1, out), 0u);         // negative
    EXPECT_EQ(encode_utf8(0x110000, out), 0u);   // > U+10FFFF
    EXPECT_EQ(encode_utf8(0xD800, out), 0u);     // surrogate
    EXPECT_EQ(encode_utf8(0xDFFF, out), 0u);     // surrogate
}
