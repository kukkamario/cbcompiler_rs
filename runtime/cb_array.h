#ifndef CB_ARRAY_H
#define CB_ARRAY_H

/* CoolBasic native-array heap helpers.
 *
 * Dedicated INTERNAL header — NOT part of the plugin SDK (cb_runtime_core.h).
 * Only cb_array.cpp (the implementation) and the gtest suite include it; the
 * LLVM backend declares these symbols itself (cb-backend-llvm/.../runtime.rs).
 *
 * These bare extern "C" helpers back the LLVM/AOT backend's array lowering.
 * The interpreter keeps its own Rust ArrayObj heap (cb-backend-interp); the two
 * backends share a SPECIFICATION enforced by the differential harness, not
 * code. The contract is the interpreter's (the diff oracle):
 *   - dims are element COUNTS; allocate product(counts) elements, laid out
 *     row-major (last index varies fastest);
 *   - any fault (negative dim, OOB/negative index, rank mismatch, null handle)
 *     raises a runtime error through the host channel — which in an AOT
 *     exe writes stderr + exits 1 — and returns a safe default. With no host
 *     (the gtest target) the trap is a no-op and the default is returned;
 *   - Len(arr, dim) with `dim` out of range returns 0 WITHOUT trapping.
 *
 * CORE TU: Allegro-free, compiles clean under -DCB_NO_ALLEGRO. */

#include "cb_runtime_core.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Heap array: a rank, per-axis element counts, the flat element total, the
 * element size in bytes, a ref-element flag (String slots default to the
 * immortal empty sentinel, never null), and the calloc'd, zero-initialised
 * element store. */
typedef struct CbArray {
    int64_t  rank;
    int64_t* dims;
    int64_t  total;
    int64_t  elem_size;
    int32_t  elem_is_ref;
    uint8_t* data;
} CbArray;

/* Allocate a rank-`rank` array of `dims[i]` elements per axis (each >= 0).
 * Traps + returns null on a negative dimension or an overflowing element/byte
 * product. String element slots (elem_is_ref != 0) are initialised to the
 * empty-string sentinel; every other element type is zero. */
CbArray* cb_rt_array_new(int64_t rank, const int64_t* dims, int64_t elem_size,
                         int32_t elem_is_ref);

/* Address of the element at the multi-axis `indices` (one per axis, row-major).
 * Traps + returns null on a null handle, a rank mismatch, or any axis index
 * < 0 or >= dims[axis]. */
void* cb_rt_array_elem_addr(CbArray* a, const int64_t* indices, int64_t rank);

/* Address of the element at flat row-major position `index`. Traps + returns
 * null on a null handle or `index` < 0 or >= total. */
void* cb_rt_array_elem_addr_flat(CbArray* a, int64_t index);

/* Total element count (product of every axis). Null handle traps + returns 0. */
int64_t cb_rt_array_total_len(const CbArray* a);

/* Length of axis `dim`. Null handle traps + returns 0; a `dim` outside
 * [0, rank) returns 0 WITHOUT trapping (matches the interpreter's Len). */
int64_t cb_rt_array_dim_len(const CbArray* a, int64_t dim);

#ifdef __cplusplus
}
#endif

#endif /* CB_ARRAY_H */
