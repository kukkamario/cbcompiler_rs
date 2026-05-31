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
#include <string>
#include <utility>
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

// Copy a CbString's raw UTF-8 bytes into an owned std::string. Convenient for
// the functions below that lean on std::string's find/replace/substr (Replace,
// CountWords, GetWord) — those only ever cut at whole-substring boundaries, so
// byte-level operations stay UTF-8-safe.
std::string bytes_of(const CbString* s) {
    return std::string(reinterpret_cast<const char*>(cb_rt_string_data(s)),
                       cb_rt_string_len(s));
}

// Build an owning CbString from a std::string. Empty -> immortal sentinel.
CbString* ret_str(const std::string& r) {
    return make_from_bytes(reinterpret_cast<const uint8_t*>(r.data()), r.size());
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
    if (at == SIZE_MAX) return 0;  // not found — 0 per spec (cb_runtime.md §Strings)
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

// ─── FD-017 completeness pass ─────────────────────────────────────────────
//
// All char indices/counts are 1-based and measured in Unicode codepoints (the
// UTF-8 divergence from cbEnchanted's single-byte CP-1252 is intentional and
// documented — FD-017 Q1). Out-of-range arguments clamp or return ""; nothing
// aborts. Where a choice diverges from the cbEnchanted reference it is called
// out inline.

// `len` codepoints from 1-based `pos`. pos<=0 -> "" (cbEnchanted errors here).
// pos past the end -> "". Negative len -> "" (cbEnchanted's size_type wrap that
// turns negative len into "rest of string" is an unintended quirk we drop).
extern "C" CbString* cb_rt_str_mid(const CbString* s, int32_t pos, int32_t len) {
    if (pos <= 0 || len <= 0) return make_empty();
    std::size_t blen = cb_rt_string_len(s);
    const uint8_t* d = cb_rt_string_data(s);
    std::size_t total = cp_len(d, blen);
    std::size_t cp_start = static_cast<std::size_t>(pos - 1);
    if (cp_start >= total) return make_empty();
    std::size_t cp_end = cp_start + static_cast<std::size_t>(len);
    if (cp_end > total) cp_end = total;
    std::size_t b0 = byte_offset_of_cp(d, blen, cp_start);
    std::size_t b1 = byte_offset_of_cp(d, blen, cp_end);
    return make_from_bytes(d + b0, b1 - b0);
}

// Replace every (non-overlapping) occurrence of `find` with `repl`. Empty
// `find` -> `s` unchanged. Pure substring replacement, so byte-level is safe.
extern "C" CbString* cb_rt_str_replace(const CbString* s, const CbString* find,
                                       const CbString* repl) {
    std::string f = bytes_of(find);
    if (f.empty()) return cb_rt_string_retain(const_cast<CbString*>(s));
    std::string str = bytes_of(s);
    std::string r = bytes_of(repl);
    std::string::size_type p = 0;
    while ((p = str.find(f, p)) != std::string::npos) {
        str.replace(p, f.size(), r);
        p += r.size();
    }
    return ret_str(str);
}

// Left-align `s` into a field `len` codepoints wide: pad spaces on the right if
// shorter, truncate to the first `len` codepoints if longer. len<0 -> "".
extern "C" CbString* cb_rt_str_lset(const CbString* s, int32_t len) {
    if (len < 0) return make_empty();
    std::size_t want = static_cast<std::size_t>(len);
    std::size_t blen = cb_rt_string_len(s);
    const uint8_t* d = cb_rt_string_data(s);
    std::size_t total = cp_len(d, blen);
    if (want <= total) {
        std::size_t cut = byte_offset_of_cp(d, blen, want);
        return make_from_bytes(d, cut);
    }
    std::string r(reinterpret_cast<const char*>(d), blen);
    r.append(want - total, ' ');
    return ret_str(r);
}

// Right-align `s` into a field `len` codepoints wide: pad spaces on the left if
// shorter, keep the rightmost `len` codepoints if longer. len<0 -> "".
extern "C" CbString* cb_rt_str_rset(const CbString* s, int32_t len) {
    if (len < 0) return make_empty();
    std::size_t want = static_cast<std::size_t>(len);
    std::size_t blen = cb_rt_string_len(s);
    const uint8_t* d = cb_rt_string_data(s);
    std::size_t total = cp_len(d, blen);
    if (want > total) {
        std::string r(want - total, ' ');
        r.append(reinterpret_cast<const char*>(d), blen);
        return ret_str(r);
    }
    std::size_t start = byte_offset_of_cp(d, blen, total - want);
    return make_from_bytes(d + start, blen - start);
}

// Value of the first character. Under our codepoint semantics this is the first
// Unicode codepoint (the inverse of Chr); cbEnchanted returns the first CP-1252
// byte (0–255). ASCII agrees. Empty string -> 0.
extern "C" int32_t cb_rt_str_asc(const CbString* s) {
    std::size_t blen = cb_rt_string_len(s);
    if (blen == 0) return 0;
    const uint8_t* d = cb_rt_string_data(s);
    uint8_t b0 = d[0];
    int32_t cp;
    int extra;
    if (b0 < 0x80) {
        return b0;
    } else if ((b0 & 0xE0) == 0xC0) {
        cp = b0 & 0x1F;
        extra = 1;
    } else if ((b0 & 0xF0) == 0xE0) {
        cp = b0 & 0x0F;
        extra = 2;
    } else if ((b0 & 0xF8) == 0xF0) {
        cp = b0 & 0x07;
        extra = 3;
    } else {
        return b0;  // invalid lead byte — return the raw byte
    }
    for (int i = 1; i <= extra && static_cast<std::size_t>(i) < blen; ++i) {
        cp = (cp << 6) | (d[i] & 0x3F);
    }
    return cp;
}

// 32-bit binary string, MSB first, always 32 chars (no leading-zero trim).
extern "C" CbString* cb_rt_bin(int32_t value) {
    uint8_t buf[32];
    uint32_t u = static_cast<uint32_t>(value);
    for (int i = 31; i >= 0; --i) {
        buf[i] = static_cast<uint8_t>((u & 1u) ? '1' : '0');
        u >>= 1;
    }
    return cb_rt_string_from_literal(buf, 32);
}

// `s` repeated `count` times. cbEnchanted quirk: the result is seeded with one
// copy of `s` and the loop appends count-1 more, so count<1 still yields ONE
// copy. Replicated for parity.
extern "C" CbString* cb_rt_str_repeat(const CbString* s, int32_t count) {
    std::size_t blen = cb_rt_string_len(s);
    if (blen == 0) return make_empty();
    std::size_t copies = count < 1 ? 1 : static_cast<std::size_t>(count);
    const uint8_t* d = cb_rt_string_data(s);
    std::string r;
    r.reserve(blen * copies);
    for (std::size_t i = 0; i < copies; ++i) {
        r.append(reinterpret_cast<const char*>(d), blen);
    }
    return ret_str(r);
}

// Reversed string. Reverses by CODEPOINT (cbEnchanted reverses bytes, which
// corrupts multibyte UTF-8 — we keep the result valid UTF-8).
extern "C" CbString* cb_rt_str_flip(const CbString* s) {
    std::size_t blen = cb_rt_string_len(s);
    if (blen == 0) return make_empty();
    const uint8_t* d = cb_rt_string_data(s);
    // Collect each codepoint's [start,end) byte range, then emit in reverse.
    std::vector<std::pair<std::size_t, std::size_t>> ranges;
    std::size_t i = 0;
    while (i < blen) {
        std::size_t start = i;
        ++i;
        while (i < blen && (d[i] & 0xC0) == 0x80) ++i;
        ranges.emplace_back(start, i);
    }
    std::string r;
    r.reserve(blen);
    for (auto it = ranges.rbegin(); it != ranges.rend(); ++it) {
        r.append(reinterpret_cast<const char*>(d + it->first),
                 it->second - it->first);
    }
    return ret_str(r);
}

// Insert `txt` at 1-based codepoint `pos` (pos<=1 -> front, pos past end ->
// append). pos<0 -> "" (cbEnchanted errors). NOTE: this uses proper 1-based
// indexing (pos-1), matching StrRemove/StrMove and the documented contract;
// cbEnchanted's StrInsert omits the -1 its siblings apply — an off-by-one we
// deliberately do not reproduce.
extern "C" CbString* cb_rt_str_insert(const CbString* s, int32_t pos,
                                      const CbString* txt) {
    if (pos < 0) return make_empty();
    std::size_t blen = cb_rt_string_len(s);
    const uint8_t* d = cb_rt_string_data(s);
    std::size_t total = cp_len(d, blen);
    std::size_t cp_at = pos <= 1 ? 0 : static_cast<std::size_t>(pos - 1);
    if (cp_at > total) cp_at = total;
    std::size_t b_at = byte_offset_of_cp(d, blen, cp_at);
    std::size_t tlen = cb_rt_string_len(txt);
    const uint8_t* td = cb_rt_string_data(txt);
    std::string r;
    r.reserve(blen + tlen);
    r.append(reinterpret_cast<const char*>(d), b_at);
    r.append(reinterpret_cast<const char*>(td), tlen);
    r.append(reinterpret_cast<const char*>(d + b_at), blen - b_at);
    return ret_str(r);
}

// Cut `len` codepoints at 1-based `pos` and re-insert them `offset` codepoints
// further along. pos<=0 / len<0 / offset<0 -> "". If the cut runs past the end,
// `s` is returned unchanged. Mirrors cbEnchanted (in codepoints).
extern "C" CbString* cb_rt_str_move(const CbString* s, int32_t pos, int32_t len,
                                    int32_t offset) {
    if (pos <= 0 || len < 0 || offset < 0) return make_empty();
    std::size_t blen = cb_rt_string_len(s);
    const uint8_t* d = cb_rt_string_data(s);
    std::size_t total = cp_len(d, blen);
    std::size_t uPos = static_cast<std::size_t>(pos);   // 1-based
    std::size_t uLen = static_cast<std::size_t>(len);
    std::size_t uOff = static_cast<std::size_t>(offset);
    if (uPos - 1 + uLen > total) {
        return cb_rt_string_retain(const_cast<CbString*>(s));  // unchanged
    }
    std::size_t cut_start = uPos - 1;
    std::size_t cut_end = cut_start + uLen;
    std::size_t b_cs = byte_offset_of_cp(d, blen, cut_start);
    std::size_t b_ce = byte_offset_of_cp(d, blen, cut_end);
    std::string txt(reinterpret_cast<const char*>(d + b_cs), b_ce - b_cs);
    // Remaining string after the cut.
    std::string rem;
    rem.reserve(blen - (b_ce - b_cs));
    rem.append(reinterpret_cast<const char*>(d), b_cs);
    rem.append(reinterpret_cast<const char*>(d + b_ce), blen - b_ce);
    // Re-insert at (pos-1)+offset codepoints into `rem`, clamped to its end.
    std::size_t rem_total = total - uLen;
    std::size_t ins_cp = cut_start + uOff;
    if (ins_cp > rem_total) ins_cp = rem_total;
    std::size_t b_ins = byte_offset_of_cp(
        reinterpret_cast<const uint8_t*>(rem.data()), rem.size(), ins_cp);
    std::string out;
    out.reserve(rem.size() + txt.size());
    out.append(rem, 0, b_ins);
    out.append(txt);
    out.append(rem, b_ins, std::string::npos);
    return ret_str(out);
}

// Number of `sep`-separated words. Empty `s` -> 0; empty `sep` -> space. Count
// is 1 + occurrences of `sep` (search advances one byte per match, mirroring
// cbEnchanted).
extern "C" int32_t cb_rt_count_words(const CbString* s, const CbString* sep) {
    std::string str = bytes_of(s);
    if (str.empty()) return 0;
    std::string sp = bytes_of(sep);
    if (sp.empty()) sp = " ";
    std::string::size_type p = 0;
    int32_t count = 1;
    while ((p = str.find(sp, p)) != std::string::npos) {
        ++p;
        ++count;
    }
    return count;
}

// The n-th (1-based) `sep`-separated word. Empty `sep` -> space. n past the
// last word yields the final segment (mirrors cbEnchanted).
extern "C" CbString* cb_rt_get_word(const CbString* s, int32_t n,
                                    const CbString* sep) {
    std::string str = bytes_of(s);
    std::string sp = bytes_of(sep);
    if (sp.empty()) sp = " ";
    std::string::size_type l = sp.size();
    std::string::size_type sep_pos;
    for (int32_t i = 1; i < n; ++i) {
        sep_pos = str.find(sp);
        if (sep_pos != std::string::npos) {
            str = str.substr(sep_pos + l);
        }
    }
    sep_pos = str.find(sp);
    if (sep_pos != std::string::npos) return ret_str(str.substr(0, sep_pos));
    return ret_str(str);
}
