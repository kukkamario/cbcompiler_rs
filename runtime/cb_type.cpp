// CoolBasic user-`Type` instance runtime (FD-049 Phase 3a).
//
// C-ABI heap + linked-list helpers that ONLY the LLVM/AOT backend calls; the
// interpreter keeps its Rust Slab/TypeList heap (preserving its observability).
// The two backends share a SPECIFICATION — the interpreter is the
// differential-test oracle — not code. See cb_type.h for the contract.
//
// CORE TU (FD-016): Allegro-free, includes only cb_runtime_core.h (via
// cb_type.h), part of cb_runtime_core. The trap pattern mirrors cb_array.cpp:
// trap() is a no-op when no host is connected (the gtest path), and in an AOT
// exe the default host's raise_error writes stderr and exits 1
// (cb_standalone.cpp), so every fault collapses to one exit-1 path.
//
// Nodes are conservatively LEAKED (malloc, never freed), exactly like Phase-2
// arrays and Phase-1 string temps; `Delete` only unlinks and sets a per-node
// `deleted` flag (no generation slab). Single-threaded (the runtime contract).

#include "cb_type.h"

#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <vector>

namespace {

// Per-`TypeDefId` list state: the head sentinel and the cached tail. Allocated
// on the heap (NOT stored inline in the vector below) so a node's `list` pointer
// stays valid when the vector grows for a higher TypeDefId.
struct CbTypeList {
    CbTypeHeader* sentinel;
    CbTypeHeader* tail;
};

// One slot per TypeDefId, grown on demand; null until the type's first `New`.
// Lazy creation is observably identical to the interpreter's eager
// per-TypeDefId lists (an uncreated/empty list and an empty list both yield
// Null for First/Last/Each) — the diff harness cannot tell them apart.
std::vector<CbTypeList*> g_lists;

// Raise an FD-015 runtime error with `msg`, if a host is connected. With no host
// (the native gtest target) this is a no-op and the caller falls through to its
// safe default — never UB. Modeled on cb_array.cpp's trap().
void trap(const char* msg) {
    const CbHostApi* h = cb_host();
    if (!h) return;
    CbString* s = cb_rt_string_from_literal(
        reinterpret_cast<const uint8_t*>(msg), std::strlen(msg));
    h->raise_error(s);
    cb_rt_string_release(s);
}

// Get (creating if needed) the list for `type_def`, with its head sentinel.
// Returns null on allocation failure (after trapping).
CbTypeList* ensure_list(int64_t type_def) {
    size_t idx = static_cast<size_t>(type_def);
    if (idx >= g_lists.size()) g_lists.resize(idx + 1, nullptr);
    CbTypeList* list = g_lists[idx];
    if (!list) {
        list = static_cast<CbTypeList*>(std::malloc(sizeof(CbTypeList)));
        CbTypeHeader* sentinel =
            static_cast<CbTypeHeader*>(std::calloc(1, sizeof(CbTypeHeader)));
        if (!list || !sentinel) {
            std::free(list);
            std::free(sentinel);
            trap("New: out of memory");
            return nullptr;
        }
        sentinel->list = list;
        sentinel->is_sentinel = 1;
        list->sentinel = sentinel;
        list->tail = nullptr;
        g_lists[idx] = list;
    }
    return list;
}

// The list for `type_def` if it has been created, else null (no creation).
CbTypeList* peek_list(int64_t type_def) {
    if (type_def < 0) return nullptr;
    size_t idx = static_cast<size_t>(type_def);
    if (idx >= g_lists.size()) return nullptr;
    return g_lists[idx];
}

// Unlink `node` from its list: repair its neighbours and, if it was the tail,
// rewind the cached tail to its predecessor (null if that is the sentinel).
// Mirrors TypeList::unlink (cb-backend-interp/src/heap.rs). The node's own
// prev/next are left intact (the node is leaked + flagged `deleted`; any later
// access traps on the flag before reading them).
void unlink(CbTypeHeader* node) {
    CbTypeHeader* prev = node->prev;
    CbTypeHeader* next = node->next;
    if (prev) prev->next = next;
    if (next) next->prev = prev;
    CbTypeList* list = static_cast<CbTypeList*>(node->list);
    if (list && list->tail == node) {
        list->tail = (prev && !prev->is_sentinel) ? prev : nullptr;
    }
}

} // namespace

extern "C" void* cb_rt_type_new(int64_t type_def, int64_t size) {
    CbTypeList* list = ensure_list(type_def);
    if (!list) return nullptr; // trap already fired
    CbTypeHeader* node =
        static_cast<CbTypeHeader*>(std::calloc(1, static_cast<size_t>(size)));
    if (!node) {
        trap("New: out of memory");
        return nullptr;
    }
    node->list = list;
    // Append at the tail (calloc zeroed deleted/is_sentinel and the fields).
    CbTypeHeader* prev = list->tail ? list->tail : list->sentinel;
    node->prev = prev;
    node->next = nullptr;
    prev->next = node;
    list->tail = node;
    return node;
}

extern "C" void* cb_rt_type_check(void* node_v) {
    CbTypeHeader* node = static_cast<CbTypeHeader*>(node_v);
    if (!node) {
        trap("null pointer dereference");
        return nullptr;
    }
    if (node->deleted) {
        trap("access to deleted object");
        return nullptr;
    }
    if (node->is_sentinel) {
        trap("null pointer dereference");
        return nullptr;
    }
    return node;
}

extern "C" void* cb_rt_type_first(int64_t type_def) {
    CbTypeList* list = peek_list(type_def);
    if (!list) return nullptr;
    return list->sentinel->next; // null for an empty list
}

extern "C" void* cb_rt_type_last(int64_t type_def) {
    CbTypeList* list = peek_list(type_def);
    if (!list) return nullptr;
    return list->tail; // null for an empty list
}

extern "C" void* cb_rt_type_next(void* node_v) {
    CbTypeHeader* node = static_cast<CbTypeHeader*>(node_v);
    if (!node) return nullptr;
    if (node->deleted) {
        trap("access to deleted object");
        return nullptr;
    }
    // No sentinel guard: a rewound lvalue may legitimately hold the sentinel,
    // and Next(sentinel) = sentinel->next = the first live node (or null),
    // exactly matching the interpreter (no sentinel check on Next).
    return node->next;
}

extern "C" void* cb_rt_type_previous(void* node_v) {
    CbTypeHeader* node = static_cast<CbTypeHeader*>(node_v);
    if (!node) return nullptr;
    if (node->deleted) {
        trap("access to deleted object");
        return nullptr;
    }
    CbTypeHeader* prev = node->prev;
    if (!prev || prev->is_sentinel) return nullptr; // hide the head sentinel
    return prev;
}

extern "C" void cb_rt_type_delete_rvalue(void* node_v) {
    CbTypeHeader* node = static_cast<CbTypeHeader*>(node_v);
    if (!node) {
        trap("null pointer dereference");
        return;
    }
    if (node->deleted || node->is_sentinel) {
        trap("double delete");
        return;
    }
    unlink(node);
    node->deleted = 1;
}

extern "C" void* cb_rt_type_delete_lvalue(void* node_v) {
    CbTypeHeader* node = static_cast<CbTypeHeader*>(node_v);
    if (!node) {
        trap("null pointer dereference");
        return nullptr;
    }
    if (node->deleted || node->is_sentinel) {
        trap("double delete");
        return nullptr;
    }
    CbTypeList* list = static_cast<CbTypeList*>(node->list);
    // Rewind target = node->prev, or the sentinel if the node was first.
    CbTypeHeader* prev = node->prev ? node->prev : (list ? list->sentinel : nullptr);
    unlink(node);
    node->deleted = 1;
    return prev;
}
