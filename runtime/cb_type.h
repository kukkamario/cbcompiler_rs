#ifndef CB_TYPE_H
#define CB_TYPE_H

/* CoolBasic user-`Type` instance heap + linked-list helpers.
 *
 * Dedicated INTERNAL header — NOT part of the plugin SDK (cb_runtime_core.h).
 * Only cb_type.cpp (the implementation) and the gtest suite include it; the
 * LLVM backend declares these symbols itself (cb-backend-llvm/.../runtime.rs).
 *
 * These bare extern "C" helpers back the LLVM/AOT backend's user-`Type`
 * lowering. The interpreter keeps its own Rust Slab/TypeList heap
 * (cb-backend-interp); the two backends share a SPECIFICATION enforced by the
 * differential harness, not code. The contract is the interpreter's (the diff
 * oracle, cb-backend-interp/src/{heap,interp}.rs):
 *
 *   - one singly-rooted doubly-linked list per `TypeDefId`, lazily created with
 *     a head SENTINEL node and a cached tail; `New` appends at the tail;
 *   - `First` = sentinel.next or Null; `Last` = tail or Null; `Next` = node.next
 *     or Null (`Next(Null)` = Null); `Previous` = node.prev unless that is the
 *     sentinel (then Null), `Previous(Null)` = Null;
 *   - an LVALUE `Delete v` unlinks+frees and REWINDS the variable to `prev` (the
 *     sentinel if it was first) so an in-flight `For Each` continues; an RVALUE
 *     `Delete` (field/element/`First(T)` operand) unlinks+frees only;
 *   - any fault (null deref, deleted access, double delete, sentinel access)
 *     raises a runtime error through the host channel — which in an AOT
 *     exe writes stderr + exits 1 — and returns a safe default. With no host
 *     (the gtest target) the trap is a no-op and the default is returned.
 *
 * MEMORY: nodes are leaked (malloc, never freed) like arrays;
 * `Delete` sets a per-node `deleted` flag rather than freeing, and any later
 * access to a deleted node traps. (No generation slab — which permits one
 * scoped `Delete x; Delete x` divergence.)
 *
 * NODE LAYOUT: a fixed 32-byte header (CbTypeHeader) followed by the type's
 * inline field storage. The C helper only ever touches the header; the LLVM
 * backend GEPs the fields (LLVM struct element index 5 + field position). The
 * backend passes `cb_rt_type_new` the LLVM `size_of` of the whole node.
 *
 * CORE TU: Allegro-free, compiles clean under -DCB_NO_ALLEGRO. */

#include "cb_runtime_core.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Shared header prefix of every type-instance node (and the per-list sentinel).
 * 32 bytes on LP64/Win64: three pointers + two int32. The backend reserves
 * these five slots as the first five elements of its node LLVM struct
 * ({ptr, ptr, ptr, i32, i32, <fields...>}); fields begin at element index 5. */
typedef struct CbTypeHeader CbTypeHeader;
struct CbTypeHeader {
    void*         list;        /* owning CbTypeList* (opaque to the backend) */
    CbTypeHeader* prev;
    CbTypeHeader* next;
    int32_t       deleted;     /* 0 live, 1 deleted-but-leaked */
    int32_t       is_sentinel; /* 1 for the per-list head sentinel only */
};

/* New <Type>: get/create the list (+ sentinel) for `type_def`, calloc a node of
 * `size` bytes (the backend's node `size_of`), append it at the tail, and return
 * it. Allocation failure traps + returns null. The field region is zero
 * (calloc); the backend overwrites String fields with the empty sentinel. */
void* cb_rt_type_new(int64_t type_def, int64_t size);

/* Validate a node before a field access (`GetField` / field-projection store):
 * null → null-deref trap, `deleted` → deleted-access trap, sentinel → null-deref
 * trap (each traps + returns null); otherwise returns `node`. */
void* cb_rt_type_check(void* node);

/* First(<Type>) / Last(<Type>): the first real node (sentinel.next) / the tail,
 * or null for an uncreated or empty list (never traps, never creates). */
void* cb_rt_type_first(int64_t type_def);
void* cb_rt_type_last(int64_t type_def);

/* Next(node): null → null; `deleted` → deleted-access trap + null; else
 * node->next (null at the tail). The sentinel is a valid argument (a rewound
 * lvalue may hold it): Next(sentinel) yields the first live node. */
void* cb_rt_type_next(void* node);

/* Previous(node): null → null; `deleted` → deleted-access trap + null; else
 * node->prev unless that is the sentinel (→ null). */
void* cb_rt_type_previous(void* node);

/* RVALUE `Delete` (operand is a field / array element / First(T)): null →
 * null-deref trap; `deleted`/sentinel → double-delete trap; else unlink (repair
 * neighbours + the list tail) and set the node's `deleted` flag. No rewind. */
void cb_rt_type_delete_rvalue(void* node);

/* LVALUE `Delete v` (operand is a bare variable): null → null-deref trap + null;
 * `deleted`/sentinel → double-delete trap + null; else capture `prev` (the
 * sentinel if the node was first), unlink + set `deleted`, and RETURN `prev` —
 * the backend stores it back into the variable's slot (the rewind). */
void* cb_rt_type_delete_lvalue(void* node);

#ifdef __cplusplus
}
#endif

#endif /* CB_TYPE_H */
