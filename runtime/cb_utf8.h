#ifndef CB_UTF8_H
#define CB_UTF8_H

// Pure UTF-8 helpers shared by the string library (cb_strfuncs.cpp) and the
// native C++ unit tests. Header-only (inline) so this Allegro-free logic can be
// exercised without linking the full runtime. (FD-022)

#include <cstddef>
#include <cstdint>

// Count Unicode codepoints in a UTF-8 buffer: every byte that is not a
// continuation byte (0b10xxxxxx) starts a new codepoint.
inline std::size_t cp_len(const uint8_t* data, std::size_t byte_len) {
    std::size_t n = 0;
    for (std::size_t i = 0; i < byte_len; ++i) {
        if ((data[i] & 0xC0) != 0x80) ++n;
    }
    return n;
}

// Byte offset of the `cp_index`-th codepoint (0-based), clamped to
// [0, byte_len]. cp_index >= codepoint count returns byte_len.
inline std::size_t byte_offset_of_cp(const uint8_t* data, std::size_t byte_len,
                                     std::size_t cp_index) {
    std::size_t seen = 0;
    for (std::size_t i = 0; i < byte_len; ++i) {
        if ((data[i] & 0xC0) != 0x80) {
            if (seen == cp_index) return i;
            ++seen;
        }
    }
    return byte_len;
}

// Encode one codepoint as UTF-8 into out[0..4]; returns the byte length, or 0
// for an invalid codepoint (negative, > U+10FFFF, or a surrogate).
inline std::size_t encode_utf8(int64_t cp, uint8_t out[4]) {
    if (cp < 0 || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)) return 0;
    auto c = static_cast<uint32_t>(cp);
    if (c < 0x80) {
        out[0] = static_cast<uint8_t>(c);
        return 1;
    } else if (c < 0x800) {
        out[0] = static_cast<uint8_t>(0xC0 | (c >> 6));
        out[1] = static_cast<uint8_t>(0x80 | (c & 0x3F));
        return 2;
    } else if (c < 0x10000) {
        out[0] = static_cast<uint8_t>(0xE0 | (c >> 12));
        out[1] = static_cast<uint8_t>(0x80 | ((c >> 6) & 0x3F));
        out[2] = static_cast<uint8_t>(0x80 | (c & 0x3F));
        return 3;
    } else {
        out[0] = static_cast<uint8_t>(0xF0 | (c >> 18));
        out[1] = static_cast<uint8_t>(0x80 | ((c >> 12) & 0x3F));
        out[2] = static_cast<uint8_t>(0x80 | ((c >> 6) & 0x3F));
        out[3] = static_cast<uint8_t>(0x80 | (c & 0x3F));
        return 4;
    }
}

#endif  // CB_UTF8_H
