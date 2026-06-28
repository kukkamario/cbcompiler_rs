// CoolBasic native-array runtime.
//
// C-ABI heap helpers that ONLY the LLVM/AOT backend calls; the interpreter
// keeps its Rust ArrayObj heap (preserving its observability). The two backends
// share a SPECIFICATION — the interpreter is the differential-test oracle — not
// code. See cb_array.h for the contract.
//
// CORE TU: Allegro-free, includes only cb_runtime_core.h (via
// cb_array.h), part of cb_runtime_core. The trap pattern mirrors
// cb_memblock.cpp: trap() is a no-op when no host is connected (the gtest
// path), and in an AOT exe the default host's raise_error writes stderr and
// exits 1 (cb_standalone.cpp), so neg-dim / OOB / rank-mismatch / null-handle
// faults all collapse to one exit-1 path.
//
// Arrays are conservatively LEAKED (no free / refcount), like string temps;
// Redim replaces a slot's handle and the old array leaks. Array deletion is
// not yet implemented.

#include "cb_array.h"

#include <cstdint>
#include <cstdlib>
#include <cstring>

namespace {

// Raise a runtime error with `msg`, if a host is connected. The host
// callback records the intent and returns (it never unwinds), so the freshly
// made CbString is released right after. With no host (the native gtest target)
// this is a no-op and the caller falls through to its safe default — never UB.
// Modeled on cb_memblock.cpp's trap().
void trap(const char* msg) {
    const CbHostApi* h = cb_host();
    if (!h) return;
    CbString* s = cb_rt_string_from_literal(
        reinterpret_cast<const uint8_t*>(msg), std::strlen(msg));
    h->raise_error(s);
    cb_rt_string_release(s);
}

// Overflow-checked multiply of two non-negative i64s. Uses the compiler builtin
// where available (clang/gcc); MSVC cl.exe has no such builtin, so fall back to
// a manual check — both operands are guaranteed >= 0 at every call site. Returns
// true on overflow (and leaves *out unspecified). Mirrors the interpreter's
// checked_mul guard so an over-large request is a clean error, not an abort.
bool mul_ov(int64_t a, int64_t b, int64_t* out) {
#if defined(__clang__) || defined(__GNUC__)
    return __builtin_mul_overflow(a, b, out);
#else
    if (a != 0 && b > INT64_MAX / a) return true;
    *out = a * b;
    return false;
#endif
}

} // namespace

// New T[d0, d1, ...] / Dim a(...): allocate product(dims) zero-initialised
// elements. A negative dimension or an overflowing element/byte product traps
// and returns null (the interp turns the same into a clean RuntimeError).
extern "C" CbArray* cb_rt_array_new(int64_t rank, const int64_t* dims,
                                    int64_t elem_size, int32_t elem_is_ref) {
    int64_t total = 1;
    for (int64_t i = 0; i < rank; ++i) {
        if (dims[i] < 0) {
            trap("New array: negative dimension");
            return nullptr;
        }
        if (mul_ov(total, dims[i], &total)) {
            trap("New array: dimension product overflow");
            return nullptr;
        }
    }
    int64_t bytes;
    if (mul_ov(total, elem_size, &bytes)) {
        trap("New array: size overflow");
        return nullptr;
    }

    CbArray* a = static_cast<CbArray*>(std::malloc(sizeof(CbArray)));
    int64_t* dim_copy = static_cast<int64_t*>(
        std::malloc(static_cast<size_t>(rank) * sizeof(int64_t)));
    // calloc zero-fills (the numeric / null default). A zero-total array gets a
    // null store that is never dereferenced — every index is out of bounds.
    uint8_t* data = (bytes > 0)
        ? static_cast<uint8_t*>(std::calloc(static_cast<size_t>(total),
                                            static_cast<size_t>(elem_size)))
        : nullptr;
    if (!a || !dim_copy || (bytes > 0 && !data)) {
        std::free(a);
        std::free(dim_copy);
        std::free(data);
        trap("New array: out of memory");
        return nullptr;
    }
    for (int64_t i = 0; i < rank; ++i) dim_copy[i] = dims[i];

    a->rank = rank;
    a->dims = dim_copy;
    a->total = total;
    a->elem_size = elem_size;
    a->elem_is_ref = elem_is_ref;
    a->data = data;

    // String element slots default to the immortal empty sentinel (refcount<0,
    // so retain/release are no-ops) — never null (the cb_runtime_core.h
    // invariant), matching the interpreter's String-element default.
    if (elem_is_ref && total > 0) {
        const CbString** slots = reinterpret_cast<const CbString**>(data);
        for (int64_t i = 0; i < total; ++i) {
            slots[i] = cb_runtime_string_api.empty;
        }
    }
    return a;
}

extern "C" void* cb_rt_array_elem_addr(CbArray* a, const int64_t* indices,
                                       int64_t rank) {
    if (!a) {
        trap("Array index: null array");
        return nullptr;
    }
    if (rank != a->rank) {
        trap("Array index: rank mismatch");
        return nullptr;
    }
    // Horner row-major fold (last index fastest), identical to the interp's
    // flat_index: flat = ((i0*d1)+i1)*d2 + ...
    int64_t flat = 0;
    for (int64_t i = 0; i < rank; ++i) {
        int64_t idx = indices[i];
        if (idx < 0 || idx >= a->dims[i]) {
            trap("Array index out of bounds");
            return nullptr;
        }
        flat = flat * a->dims[i] + idx;
    }
    return a->data + flat * a->elem_size;
}

extern "C" void* cb_rt_array_elem_addr_flat(CbArray* a, int64_t index) {
    if (!a) {
        trap("Array index: null array");
        return nullptr;
    }
    if (index < 0 || index >= a->total) {
        trap("Array index out of bounds");
        return nullptr;
    }
    return a->data + index * a->elem_size;
}

extern "C" int64_t cb_rt_array_total_len(const CbArray* a) {
    if (!a) {
        trap("ArrayTotalLen: null array");
        return 0;
    }
    return a->total;
}

extern "C" int64_t cb_rt_array_dim_len(const CbArray* a, int64_t dim) {
    if (!a) {
        trap("Len: null array");
        return 0;
    }
    // An out-of-range axis returns 0 WITHOUT trapping (interp: dim_len → None →
    // unwrap_or(0)).
    if (dim < 0 || dim >= a->rank) return 0;
    return a->dims[dim];
}
