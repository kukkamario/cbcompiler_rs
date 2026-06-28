// Unit tests for the native user-`Type` runtime (cb_type.cpp).
// These drive the bare extern "C" cb_rt_type_* helpers directly, like
// test_array.cpp. No trap host is connected (cb_runtime_init is never called),
// so cb_host() returns null: a fault is a silent no-op returning the safe
// default (null). That "returns the default, never corrupts/crashes" property is
// what these pin; the end-to-end fault-to-exit-1 behaviour (host connected) is
// covered by the cb-driver type_* differential fixtures.
//
// The list state is process-global (one per TypeDefId), so each test uses a
// DISTINCT type_def id to stay independent of the others.

#include "cb_type.h"

#include <gtest/gtest.h>

#include <cstdint>

namespace {

// A node with the 32-byte header followed by one inline Int field, mirroring the
// backend's {ptr,ptr,ptr,i32,i32, i32} node struct (field at offset 32).
struct NodeI {
    CbTypeHeader hdr;
    int32_t      v;
};

CbTypeHeader* make(int64_t type_def, int32_t v) {
    auto* n = static_cast<NodeI*>(cb_rt_type_new(type_def, sizeof(NodeI)));
    if (n) n->v = v;
    return &n->hdr;
}

int32_t val(void* node) { return reinterpret_cast<NodeI*>(node)->v; }

} // namespace

TEST(Type, NewAppendsAndFieldRegionIsZero) {
    int64_t td = 0;
    auto* a = static_cast<NodeI*>(cb_rt_type_new(td, sizeof(NodeI)));
    ASSERT_NE(a, nullptr);
    EXPECT_EQ(a->v, 0);            // calloc-zeroed field region
    EXPECT_EQ(a->hdr.deleted, 0);
    EXPECT_EQ(a->hdr.is_sentinel, 0);
    // First == the freshly appended node; Last == it too (single element).
    EXPECT_EQ(cb_rt_type_first(td), &a->hdr);
    EXPECT_EQ(cb_rt_type_last(td), &a->hdr);
}

TEST(Type, FirstNextLastWalk) {
    int64_t td = 1;
    CbTypeHeader* a = make(td, 1);
    CbTypeHeader* b = make(td, 2);
    CbTypeHeader* c = make(td, 3);
    EXPECT_EQ(cb_rt_type_first(td), a);
    EXPECT_EQ(cb_rt_type_last(td), c);
    EXPECT_EQ(cb_rt_type_next(a), b);
    EXPECT_EQ(cb_rt_type_next(b), c);
    EXPECT_EQ(cb_rt_type_next(c), nullptr); // tail
    EXPECT_EQ(val(a), 1);
    EXPECT_EQ(val(c), 3);
}

TEST(Type, PreviousHidesSentinel) {
    int64_t td = 2;
    CbTypeHeader* a = make(td, 1);
    CbTypeHeader* b = make(td, 2);
    EXPECT_EQ(cb_rt_type_previous(b), a);
    EXPECT_EQ(cb_rt_type_previous(a), nullptr); // first node's prev is the sentinel
}

TEST(Type, FirstLastEmptyAndUncreatedAreNull) {
    EXPECT_EQ(cb_rt_type_first(50), nullptr); // never created
    EXPECT_EQ(cb_rt_type_last(50), nullptr);
}

TEST(Type, DeleteRvalueUnlinksAndFlags) {
    int64_t td = 3;
    CbTypeHeader* a = make(td, 1);
    CbTypeHeader* b = make(td, 2);
    CbTypeHeader* c = make(td, 3);
    cb_rt_type_delete_rvalue(b);
    EXPECT_EQ(b->deleted, 1);
    // List repaired: a -> c, c is the tail.
    EXPECT_EQ(cb_rt_type_next(a), c);
    EXPECT_EQ(cb_rt_type_previous(c), a);
    EXPECT_EQ(cb_rt_type_last(td), c);
    // Accessing the deleted node traps (no-op host) and returns null.
    EXPECT_EQ(cb_rt_type_check(b), nullptr);
    EXPECT_EQ(cb_rt_type_next(b), nullptr);
    EXPECT_EQ(cb_rt_type_previous(b), nullptr);
}

TEST(Type, DeleteRvalueTailUpdatesLast) {
    int64_t td = 4;
    CbTypeHeader* a = make(td, 1);
    CbTypeHeader* b = make(td, 2);
    cb_rt_type_delete_rvalue(b); // delete the tail
    EXPECT_EQ(cb_rt_type_last(td), a);
    EXPECT_EQ(cb_rt_type_next(a), nullptr);
}

TEST(Type, DeleteLvalueReturnsPrev) {
    int64_t td = 5;
    CbTypeHeader* a = make(td, 1);
    CbTypeHeader* b = make(td, 2);
    CbTypeHeader* c = make(td, 3);
    void* prev = cb_rt_type_delete_lvalue(b); // middle node
    EXPECT_EQ(prev, a);                       // rewind target = predecessor
    EXPECT_EQ(b->deleted, 1);
    // Next(prev) yields the node that was after the deleted one (the For Each
    // rewind contract).
    EXPECT_EQ(cb_rt_type_next(prev), c);
}

TEST(Type, DeleteLvalueFirstReturnsSentinelAndNextResumes) {
    int64_t td = 6;
    CbTypeHeader* a = make(td, 1);
    CbTypeHeader* b = make(td, 2);
    void* prev = cb_rt_type_delete_lvalue(a); // first node
    ASSERT_NE(prev, nullptr);                 // the head sentinel
    EXPECT_EQ(reinterpret_cast<CbTypeHeader*>(prev)->is_sentinel, 1);
    // Next(sentinel) resumes at the next live node — how `For Each` continues
    // after deleting the first element.
    EXPECT_EQ(cb_rt_type_next(prev), b);
    EXPECT_EQ(cb_rt_type_first(td), b);
}

TEST(Type, NextNullAndPreviousNullAreNull) {
    EXPECT_EQ(cb_rt_type_next(nullptr), nullptr);
    EXPECT_EQ(cb_rt_type_previous(nullptr), nullptr);
}

TEST(Type, CheckNullAndSentinelTrapToNull) {
    int64_t td = 7;
    CbTypeHeader* a = make(td, 1);
    (void)a;
    EXPECT_EQ(cb_rt_type_check(nullptr), nullptr);
    // The sentinel is never a valid field-access target.
    void* prev = cb_rt_type_delete_lvalue(a); // a was first -> prev is sentinel
    EXPECT_EQ(cb_rt_type_check(prev), nullptr);
}

TEST(Type, DoubleDeleteTrapsToNull) {
    int64_t td = 8;
    CbTypeHeader* a = make(td, 1);
    CbTypeHeader* b = make(td, 2);
    cb_rt_type_delete_rvalue(b);
    EXPECT_EQ(b->deleted, 1);
    // Second rvalue delete sees the flag, traps (no-op host), state intact.
    cb_rt_type_delete_rvalue(b);
    EXPECT_EQ(b->deleted, 1);
    EXPECT_EQ(cb_rt_type_last(td), a);
    // An already-deleted lvalue delete traps to null too.
    EXPECT_EQ(cb_rt_type_delete_lvalue(b), nullptr);
    // Lvalue delete of the first node returns the head sentinel; deleting THAT
    // (the divergent double-delete-of-first case) hits the sentinel guard and
    // traps to null.
    void* sentinel = cb_rt_type_delete_lvalue(a);
    ASSERT_NE(sentinel, nullptr);
    EXPECT_EQ(cb_rt_type_delete_lvalue(sentinel), nullptr);
}
