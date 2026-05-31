// CoolBasic runtime — String library (FD-013 Batch 2; split out per FD-016).
//
// The CB-visible string functions (Upper/Lower/Trim/Left/Right/StrRemove/
// InStr/Chr/Hex). These are FUNCTIONALITY built on top of the core string
// ABI, so they live in the functionality library, not in the core
// cb_string.cpp.
//
// IMPORTANT (FD-016): this TU reaches the string type ONLY through the public
// core primitives declared in cb_runtime_core.h — `cb_rt_string_from_literal`,
// `cb_rt_string_len`, `cb_rt_string_data`, `cb_rt_string_retain`, and the
// immortal empty sentinel via `cb_runtime_string_api.empty`. It deliberately
// does NOT touch cb_string.cpp's private internals (alloc_with_data, data_of,
// CB_EMPTY_STRING_INSTANCE). That keeps the core's surface minimal and makes
// this library a worked example of how a plugin builds strings using nothing
// but the core ABI.
//
// Two deliberate departures from the legacy implementation, per FD-013:
//   - CODEPOINT semantics. The legacy ran on UTF-32, so char-indexed ops were
//     O(1); the v4 ABI stores UTF-8, so Left/Right/StrRemove/InStr walk the
//     bytes to map a 1-based character index to a byte offset. CB strings are
//     UTF-8 (§3.1) and the docs count "characters", so this is the correct
//     visible behaviour.
//   - CLAMP, never abort. Legacy Left/Right called a fatal error() on
//     out-of-range arguments; there is no runtime->interpreter trap channel
//     here, so we saturate instead (Left("hi",5) -> "hi", n<=0 -> "").
//
// Upper/Lower do ASCII-only case mapping: bytes >= 0x80 (UTF-8 continuation
// and lead bytes) are passed through untouched, so multibyte sequences are
// preserved. Full Unicode casing would need case tables — out of scope.

#include "cb_runtime_func.h"

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <vector>

namespace {

// Count Unicode codepoints in a UTF-8 buffer: every byte that is not a
// continuation byte (0b10xxxxxx) starts a new codepoint.
std::size_t cp_len(const uint8_t* data, std::size_t byte_len) {
    std::size_t n = 0;
    for (std::size_t i = 0; i < byte_len; ++i) {
        if ((data[i] & 0xC0) != 0x80) ++n;
    }
    return n;
}

// Byte offset of the `cp_index`-th codepoint (0-based), clamped to
// [0, byte_len]. cp_index >= codepoint count returns byte_len.
std::size_t byte_offset_of_cp(const uint8_t* data, std::size_t byte_len,
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
std::size_t encode_utf8(int64_t cp, uint8_t out[4]) {
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

// Owning reference to the immortal empty sentinel; the canonical "" result.
// Reached through the public CbStringApi rather than the core's private
// CB_EMPTY_STRING_INSTANCE symbol. retain on the sentinel is a no-op.
CbString* make_empty() {
    return cb_rt_string_retain(const_cast<CbString*>(cb_runtime_string_api.empty));
}

// Build an owning CbString from a byte range. Empty range -> sentinel.
CbString* make_from_bytes(const uint8_t* data, std::size_t len) {
    if (len == 0) return make_empty();
    return cb_rt_string_from_literal(data, len);
}

inline bool is_ascii_space(uint8_t b) {
    return b == ' ' || b == '\t' || b == '\r' || b == '\n';
}

} // namespace

extern "C" CbString* cb_rt_str_upper(const CbString* s) {
    std::size_t len = cb_rt_string_len(s);
    if (len == 0) return make_empty();
    const uint8_t* src = cb_rt_string_data(s);
    std::vector<uint8_t> buf(len);
    for (std::size_t i = 0; i < len; ++i) {
        uint8_t b = src[i];
        buf[i] = (b >= 'a' && b <= 'z') ? static_cast<uint8_t>(b - 32) : b;
    }
    return cb_rt_string_from_literal(buf.data(), len);
}

extern "C" CbString* cb_rt_str_lower(const CbString* s) {
    std::size_t len = cb_rt_string_len(s);
    if (len == 0) return make_empty();
    const uint8_t* src = cb_rt_string_data(s);
    std::vector<uint8_t> buf(len);
    for (std::size_t i = 0; i < len; ++i) {
        uint8_t b = src[i];
        buf[i] = (b >= 'A' && b <= 'Z') ? static_cast<uint8_t>(b + 32) : b;
    }
    return cb_rt_string_from_literal(buf.data(), len);
}

extern "C" CbString* cb_rt_str_trim(const CbString* s) {
    std::size_t len = cb_rt_string_len(s);
    const uint8_t* data = cb_rt_string_data(s);
    std::size_t begin = 0, end = len;
    while (begin < end && is_ascii_space(data[begin])) ++begin;
    while (end > begin && is_ascii_space(data[end - 1])) --end;
    return make_from_bytes(data + begin, end - begin);
}

extern "C" CbString* cb_rt_str_left(const CbString* s, int32_t n) {
    if (n <= 0) return make_empty();
    std::size_t len = cb_rt_string_len(s);
    const uint8_t* data = cb_rt_string_data(s);
    std::size_t cut = byte_offset_of_cp(data, len, static_cast<std::size_t>(n));
    return make_from_bytes(data, cut);
}

extern "C" CbString* cb_rt_str_right(const CbString* s, int32_t n) {
    if (n <= 0) return make_empty();
    std::size_t len = cb_rt_string_len(s);
    const uint8_t* data = cb_rt_string_data(s);
    std::size_t total = cp_len(data, len);
    std::size_t want = static_cast<std::size_t>(n);
    if (want >= total) return cb_rt_string_retain(const_cast<CbString*>(s));
    std::size_t start = byte_offset_of_cp(data, len, total - want);
    return make_from_bytes(data + start, len - start);
}

extern "C" CbString* cb_rt_str_remove(const CbString* s, int32_t pos, int32_t count) {
    std::size_t len = cb_rt_string_len(s);
    const uint8_t* data = cb_rt_string_data(s);
    // pos is 1-based; clamp to a 0-based codepoint index in [0, cp_total].
    std::size_t total = cp_len(data, len);
    std::size_t cp_start = pos <= 1 ? 0 : static_cast<std::size_t>(pos - 1);
    if (cp_start > total) cp_start = total;
    std::size_t cp_count = count <= 0 ? 0 : static_cast<std::size_t>(count);
    std::size_t cp_end = cp_start + cp_count;
    if (cp_end > total) cp_end = total;

    std::size_t b_start = byte_offset_of_cp(data, len, cp_start);
    std::size_t b_end = byte_offset_of_cp(data, len, cp_end);
    std::size_t kept = b_start + (len - b_end);
    if (kept == 0) return make_empty();

    std::vector<uint8_t> buf;
    buf.reserve(kept);
    buf.insert(buf.end(), data, data + b_start);
    buf.insert(buf.end(), data + b_end, data + len);
    return cb_rt_string_from_literal(buf.data(), buf.size());
}

// Find the first byte offset of `needle` in `hay[from..]`, or SIZE_MAX.
static std::size_t byte_find(const uint8_t* hay, std::size_t hlen,
                             const uint8_t* needle, std::size_t nlen,
                             std::size_t from) {
    if (nlen == 0) return from <= hlen ? from : SIZE_MAX;
    if (nlen > hlen) return SIZE_MAX;
    for (std::size_t i = from; i + nlen <= hlen; ++i) {
        if (std::memcmp(hay + i, needle, nlen) == 0) return i;
    }
    return SIZE_MAX;
}

// 1-based codepoint index of `find` in `s`, or -1. `start` (when given) is a
// 1-based codepoint position to begin searching from.
static int32_t instr_impl(const CbString* s, const CbString* find, int32_t start) {
    std::size_t hlen = cb_rt_string_len(s);
    const uint8_t* hay = cb_rt_string_data(s);
    std::size_t nlen = cb_rt_string_len(find);
    const uint8_t* needle = cb_rt_string_data(find);

    std::size_t cp_start = start <= 1 ? 0 : static_cast<std::size_t>(start - 1);
    std::size_t from = byte_offset_of_cp(hay, hlen, cp_start);

    std::size_t at = byte_find(hay, hlen, needle, nlen, from);
    if (at == SIZE_MAX) return -1;
    // Convert the byte offset of the match to a 1-based codepoint index.
    return static_cast<int32_t>(cp_len(hay, at)) + 1;
}

extern "C" int32_t cb_rt_str_instr(const CbString* s, const CbString* find) {
    return instr_impl(s, find, 1);
}

extern "C" int32_t cb_rt_str_instr_from(const CbString* s, const CbString* find, int32_t start) {
    return instr_impl(s, find, start);
}

extern "C" CbString* cb_rt_chr(int32_t code) {
    uint8_t buf[4];
    std::size_t n = encode_utf8(code, buf);
    if (n == 0) return make_empty();
    return cb_rt_string_from_literal(buf, n);
}

extern "C" CbString* cb_rt_hex(int32_t value) {
    // Uppercase, zero-padded to 8 hex digits (matches legacy CBF_hex).
    static const char digits[] = "0123456789ABCDEF";
    uint8_t buf[8];
    uint32_t u = static_cast<uint32_t>(value);
    for (int i = 7; i >= 0; --i) {
        buf[i] = static_cast<uint8_t>(digits[u & 0xF]);
        u >>= 4;
    }
    return cb_rt_string_from_literal(buf, 8);
}
