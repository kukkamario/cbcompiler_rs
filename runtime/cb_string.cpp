// CoolBasic runtime — refcounted opaque string implementation.
//
// CbString is a single heap block: a small header followed by inline UTF-8
// bytes. The design choices, and why:
//   - UTF-8 internally (not UTF-32). CB §3.1 mandates UTF-8 and §5.3 forbids
//     `[]` indexing on strings, so codepoint-indexed access never needs to be
//     O(1) — the string library pays the walk cost on demand.
//   - No separate decoded buffer — the inline bytes are already UTF-8, so
//     there is nothing to cache or keep in sync.
//   - No C++ smart-pointer wrapper on this side. Ownership/RAII lives
//     Rust-side (`CbStringHandle` in cb-backend-interp); across the FFI
//     boundary a string is a bare `CbString*`.
//   - The static-data sentinel is a signed `refcount < 0`, which a backend can
//     emit directly as a constant initializer rather than having to encode it
//     in the data offset.
//
// All consumers (Rust, future LLVM IR emission) see CbString only through
// the primitives below, exposed via CbStringApi on the catalog. The struct
// layout is private to this TU.
//
// FD-016: this is the CORE string implementation — the only TU in the
// Allegro-free `cb_runtime_core` library. The CB-visible string LIBRARY
// (Upper/Left/InStr/…) lives in the functionality TU cb_strfuncs.cpp, which
// reaches these primitives through the public surface only.

#include "cb_runtime_core.h"

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
static const CbString k_empty_string_instance = {
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

// A statically-allocated immortal CbString carrying the out-of-memory message,
// built as one object (header + inline bytes) so the OOM path touches no heap.
// `offset` points past the header to `bytes`, exactly like a heap CbString, so
// data_of() resolves to the message. refcount = -1 marks it immortal.
namespace {
struct OomMsg {
    CbString hdr;
    char     bytes[33];
};
const OomMsg k_oom = {
    {/*refcount*/ -1, /*pad*/ 0, /*byte_len*/ 32, /*capacity*/ 32,
     /*offset*/ offsetof(OomMsg, bytes)},
    "CoolBasic runtime: out of memory",  // 32 bytes (NUL not counted)
};
}  // namespace

// Single-block allocation: header + inline data. Refcount starts at 1;
// caller is responsible for filling the data region. On allocation failure we
// best-effort surface the condition through the FD-015 trap channel, then
// abort. raise_error only RECORDS the intent and returns (cb_runtime_core.h),
// so it cannot rescue an in-flight allocation that has no valid pointer to hand
// back — std::abort() remains the hard stop (FD-022).
static CbString* alloc_with_data(std::size_t len) {
    CbString* s = static_cast<CbString*>(std::malloc(sizeof(CbString) + len));
    if (!s) {
        const CbHostApi* h = cb_host();
        if (h && h->raise_error) h->raise_error(&k_oom.hdr);
        std::abort();
    }
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
    // Empty-operand fast paths: when one side is empty the result is just the
    // other side, so retain it and skip the allocation; two empties yield the
    // shared sentinel.
    std::size_t la = cb_rt_string_len(a);
    std::size_t lb = cb_rt_string_len(b);
    if (la == 0 && lb == 0) {
        return cb_rt_string_retain(const_cast<CbString*>(&k_empty_string_instance));
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
    /* empty        */ &k_empty_string_instance,
};
