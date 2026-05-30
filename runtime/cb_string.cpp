// CoolBasic runtime — refcounted opaque string implementation.
//
// Port of legacy LString / LStringData (G:\projects\CBCompiler\Runtime\
// lstring.{h,cpp}), modulo:
//   - UTF-32 internal representation dropped. CB §3.1 mandates UTF-8 and
//     §5.3 forbids `[]` indexing on strings, so codepoint-indexed access
//     never needs to be O(1) — string library functions pay the walk cost.
//   - Cached `mUtf8String` pointer dropped — inline data is already UTF-8.
//   - `LString` smart-pointer wrapper dropped. RAII lives Rust-side
//     (`CbStringHandle` in cb-backend-interp).
//   - Static-data sentinel encoding switched from
//     `mOffset != sizeof(LStringData)` to signed `refcount < 0`. Cleaner
//     match for LLVM constant initializers.
//
// All consumers (Rust, future LLVM IR emission) see CbString only through
// the primitives below, exposed via CbStringApi on the catalog. The struct
// layout is private to this TU.

#include "cb_runtime.h"

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>

struct CbString {
    int32_t      refcount;   // accessed via std::atomic_ref; -1 == immortal sentinel
    uint32_t     pad;        // explicit pad so the size_t fields land on 8-byte alignment
    std::size_t  byte_len;
    std::size_t  capacity;
    std::size_t  offset;     // bytes from struct base to data start
};

static_assert(sizeof(CbString) % alignof(std::size_t) == 0,
              "CbString header must keep size_t fields aligned for inline data layout");

// Canonical empty-string sentinel. Declared as a const aggregate so it
// lives in .rodata; refcount = -1 means retain/release short-circuit.
// Exposed through cb_runtime_string_api.empty rather than as a public
// symbol — callers compare via that field, not by symbol name.
static const CbString CB_EMPTY_STRING_INSTANCE = {
    /* refcount */ -1,
    /* pad      */ 0,
    /* byte_len */ 0,
    /* capacity */ 0,
    /* offset   */ sizeof(CbString),
};

// ─── Internal helpers ───────────────────────────────────────────────────

static inline std::atomic_ref<int32_t> refcount_of(CbString* s) {
    return std::atomic_ref<int32_t>(s->refcount);
}

// Plain-load read of the sentinel bit. Safe under the data-race rules
// because once a string is constructed with negative refcount we never
// flip it positive, and a string constructed positive never reaches
// negative — release frees the block at refcount==1 before any decrement
// could go below zero. So `refcount < 0` is a stable, race-free property.
static inline bool is_static(const CbString* s) {
    return std::atomic_ref<int32_t>(const_cast<int32_t&>(s->refcount))
        .load(std::memory_order_relaxed) < 0;
}

static inline uint8_t* data_of(CbString* s) {
    return reinterpret_cast<uint8_t*>(s) + s->offset;
}

static inline const uint8_t* data_of(const CbString* s) {
    return reinterpret_cast<const uint8_t*>(s) + s->offset;
}

// Single-block allocation: header + inline data. Refcount starts at 1;
// caller is responsible for filling the data region.
static CbString* alloc_with_data(std::size_t len) {
    CbString* s = static_cast<CbString*>(std::malloc(sizeof(CbString) + len));
    if (!s) std::abort();
    s->refcount = 1;
    s->pad      = 0;
    s->byte_len = len;
    s->capacity = len;
    s->offset   = sizeof(CbString);
    return s;
}

// ─── Primitives ─────────────────────────────────────────────────────────

extern "C" CbString* cb_rt_string_retain(CbString* s) {
    if (s && !is_static(s)) {
        refcount_of(s).fetch_add(1, std::memory_order_relaxed);
    }
    return s;
}

extern "C" void cb_rt_string_release(CbString* s) {
    if (!s || is_static(s)) return;
    if (refcount_of(s).fetch_sub(1, std::memory_order_acq_rel) == 1) {
        std::free(s);
    }
}

extern "C" CbString* cb_rt_string_from_literal(const uint8_t* data, std::size_t len) {
    CbString* s = alloc_with_data(len);
    if (len > 0) std::memcpy(data_of(s), data, len);
    return s;
}

extern "C" std::size_t cb_rt_string_len(const CbString* s) {
    return s ? s->byte_len : 0;
}

extern "C" const uint8_t* cb_rt_string_data(const CbString* s) {
    return s ? data_of(s) : nullptr;
}

extern "C" CbString* cb_rt_string_concat(const CbString* a, const CbString* b) {
    // Empty-operand fast paths: retain the non-empty side and avoid the
    // allocation. Mirrors the legacy `LString::operator+` early exit.
    std::size_t la = cb_rt_string_len(a);
    std::size_t lb = cb_rt_string_len(b);
    if (la == 0 && lb == 0) {
        return cb_rt_string_retain(const_cast<CbString*>(&CB_EMPTY_STRING_INSTANCE));
    }
    if (la == 0) return cb_rt_string_retain(const_cast<CbString*>(b));
    if (lb == 0) return cb_rt_string_retain(const_cast<CbString*>(a));

    CbString* out = alloc_with_data(la + lb);
    std::memcpy(data_of(out),      data_of(a), la);
    std::memcpy(data_of(out) + la, data_of(b), lb);
    return out;
}

extern "C" int32_t cb_rt_string_test_refcount(const CbString* s) {
    if (!s) return 0;
    return std::atomic_ref<int32_t>(const_cast<int32_t&>(s->refcount))
        .load(std::memory_order_relaxed);
}

// ─── String library (FD-013 Batch 2) ────────────────────────────────────
//
// CoolBasic string functions, ported from ../CBCompiler/Runtime/cb_string.cpp.
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

namespace {

// Count Unicode codepoints in a UTF-8 buffer: every byte that is not a
// continuation byte (0b10xxxxxx) starts a new codepoint.
static std::size_t cp_len(const uint8_t* data, std::size_t byte_len) {
    std::size_t n = 0;
    for (std::size_t i = 0; i < byte_len; ++i) {
        if ((data[i] & 0xC0) != 0x80) ++n;
    }
    return n;
}

// Byte offset of the `cp_index`-th codepoint (0-based), clamped to
// [0, byte_len]. cp_index >= codepoint count returns byte_len.
static std::size_t byte_offset_of_cp(const uint8_t* data, std::size_t byte_len,
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
static std::size_t encode_utf8(int64_t cp, uint8_t out[4]) {
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
static CbString* make_empty() {
    return cb_rt_string_retain(const_cast<CbString*>(&CB_EMPTY_STRING_INSTANCE));
}

// Build an owning CbString from a byte range. Empty range -> sentinel.
static CbString* make_from_bytes(const uint8_t* data, std::size_t len) {
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
    CbString* out = alloc_with_data(len);
    uint8_t* dst = data_of(out);
    for (std::size_t i = 0; i < len; ++i) {
        uint8_t b = src[i];
        dst[i] = (b >= 'a' && b <= 'z') ? static_cast<uint8_t>(b - 32) : b;
    }
    return out;
}

extern "C" CbString* cb_rt_str_lower(const CbString* s) {
    std::size_t len = cb_rt_string_len(s);
    if (len == 0) return make_empty();
    const uint8_t* src = cb_rt_string_data(s);
    CbString* out = alloc_with_data(len);
    uint8_t* dst = data_of(out);
    for (std::size_t i = 0; i < len; ++i) {
        uint8_t b = src[i];
        dst[i] = (b >= 'A' && b <= 'Z') ? static_cast<uint8_t>(b + 32) : b;
    }
    return out;
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

    CbString* out = alloc_with_data(kept);
    uint8_t* dst = data_of(out);
    std::memcpy(dst, data, b_start);
    std::memcpy(dst + b_start, data + b_end, len - b_end);
    return out;
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

// ─── Catalog wiring ─────────────────────────────────────────────────────
//
// The CbStringApi instance is defined in this TU so cb_string.cpp owns
// everything string-related. catalog.cpp just references it when assembling
// the CbCatalog struct.

extern "C" const CbStringApi cb_runtime_string_api = {
    /* retain       */ cb_rt_string_retain,
    /* release      */ cb_rt_string_release,
    /* from_literal */ cb_rt_string_from_literal,
    /* len          */ cb_rt_string_len,
    /* data         */ cb_rt_string_data,
    /* concat       */ cb_rt_string_concat,
    /* empty        */ &CB_EMPTY_STRING_INSTANCE,
};
