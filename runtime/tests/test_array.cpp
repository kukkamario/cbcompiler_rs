// FD-049 Phase 2: unit tests for the native array runtime (cb_array.cpp). These
// drive the bare extern "C" cb_rt_array_* helpers directly, like
// test_memblock.cpp. No trap host is connected (cb_runtime_init is never
// called), so cb_host() returns null: a fault is a silent no-op returning the
// safe default (null / 0). That "returns the default, never corrupts/crashes"
// property is what these pin; the end-to-end fault-to-exit-1 behaviour (host
// connected) is covered by the cb-driver array_oob differential fixture.

#include "cb_array.h"

#include <gtest/gtest.h>

#include <cstdint>

TEST(Array, OneDimZeroFilledDefault) {
    int64_t d[1] = {4};
    CbArray* a = cb_rt_array_new(1, d, sizeof(int32_t), 0);
    ASSERT_NE(a, nullptr);
    EXPECT_EQ(cb_rt_array_total_len(a), 4);
    EXPECT_EQ(cb_rt_array_dim_len(a, 0), 4);
    for (int64_t i = 0; i < 4; ++i) {
        int32_t* p = static_cast<int32_t*>(cb_rt_array_elem_addr(a, &i, 1));
        ASSERT_NE(p, nullptr);
        EXPECT_EQ(*p, 0);  // calloc default
    }
}

TEST(Array, OneDimRoundTrip) {
    int64_t d[1] = {3};
    CbArray* a = cb_rt_array_new(1, d, sizeof(int32_t), 0);
    ASSERT_NE(a, nullptr);
    for (int64_t i = 0; i < 3; ++i) {
        *static_cast<int32_t*>(cb_rt_array_elem_addr(a, &i, 1)) =
            static_cast<int32_t>(i * 10);
    }
    for (int64_t i = 0; i < 3; ++i) {
        EXPECT_EQ(*static_cast<int32_t*>(cb_rt_array_elem_addr(a, &i, 1)),
                  static_cast<int32_t>(i * 10));
    }
}

TEST(Array, TwoDimRowMajor) {
    // dims [2,3]; addr(1,2) is flat 1*3 + 2 = 5 (last index fastest).
    int64_t d[2] = {2, 3};
    CbArray* a = cb_rt_array_new(2, d, sizeof(int32_t), 0);
    ASSERT_NE(a, nullptr);
    EXPECT_EQ(cb_rt_array_total_len(a), 6);
    EXPECT_EQ(cb_rt_array_dim_len(a, 0), 2);
    EXPECT_EQ(cb_rt_array_dim_len(a, 1), 3);
    int64_t multi[2] = {1, 2};
    void* via_multi = cb_rt_array_elem_addr(a, multi, 2);
    void* via_flat = cb_rt_array_elem_addr_flat(a, 5);
    ASSERT_NE(via_multi, nullptr);
    EXPECT_EQ(via_multi, via_flat);
}

TEST(Array, DimLenOutOfRangeReturnsZeroNoTrap) {
    int64_t d[2] = {2, 3};
    CbArray* a = cb_rt_array_new(2, d, sizeof(int32_t), 0);
    ASSERT_NE(a, nullptr);
    EXPECT_EQ(cb_rt_array_dim_len(a, 2), 0);   // == rank
    EXPECT_EQ(cb_rt_array_dim_len(a, 99), 0);  // > rank
    EXPECT_EQ(cb_rt_array_dim_len(a, -1), 0);  // negative
}

TEST(Array, OutOfBoundsAndRankMismatchReturnNull) {
    int64_t d[2] = {2, 3};
    CbArray* a = cb_rt_array_new(2, d, sizeof(int32_t), 0);
    ASSERT_NE(a, nullptr);
    int64_t oob[2] = {2, 0};  // axis 0 == dims[0]
    EXPECT_EQ(cb_rt_array_elem_addr(a, oob, 2), nullptr);
    int64_t neg[2] = {0, -1};
    EXPECT_EQ(cb_rt_array_elem_addr(a, neg, 2), nullptr);
    int64_t wrong_rank[1] = {0};
    EXPECT_EQ(cb_rt_array_elem_addr(a, wrong_rank, 1), nullptr);
    EXPECT_EQ(cb_rt_array_elem_addr_flat(a, 6), nullptr);  // == total
    EXPECT_EQ(cb_rt_array_elem_addr_flat(a, -1), nullptr);
}

TEST(Array, NegativeDimensionReturnsNull) {
    int64_t d[1] = {-1};
    EXPECT_EQ(cb_rt_array_new(1, d, sizeof(int32_t), 0), nullptr);
}

TEST(Array, NullHandleQueriesAreSafe) {
    EXPECT_EQ(cb_rt_array_total_len(nullptr), 0);
    EXPECT_EQ(cb_rt_array_dim_len(nullptr, 0), 0);
    int64_t idx = 0;
    EXPECT_EQ(cb_rt_array_elem_addr(nullptr, &idx, 1), nullptr);
    EXPECT_EQ(cb_rt_array_elem_addr_flat(nullptr, 0), nullptr);
}

TEST(Array, StringElementDefaultIsEmptySentinel) {
    int64_t d[1] = {2};
    CbArray* a = cb_rt_array_new(1, d, sizeof(void*), 1);
    ASSERT_NE(a, nullptr);
    for (int64_t i = 0; i < 2; ++i) {
        const CbString** slot =
            static_cast<const CbString**>(cb_rt_array_elem_addr(a, &i, 1));
        ASSERT_NE(slot, nullptr);
        EXPECT_EQ(*slot, cb_runtime_string_api.empty);  // never null
    }
}

TEST(Array, ZeroTotalArray) {
    int64_t d[1] = {0};
    CbArray* a = cb_rt_array_new(1, d, sizeof(int32_t), 0);
    ASSERT_NE(a, nullptr);
    EXPECT_EQ(cb_rt_array_total_len(a), 0);
    EXPECT_EQ(cb_rt_array_dim_len(a, 0), 0);
    int64_t idx = 0;
    EXPECT_EQ(cb_rt_array_elem_addr(a, &idx, 1), nullptr);  // 0 >= total 0
}
