// Unit tests for the memory-block runtime (cb_memblock.cpp). These
// drive the extern "C" cb_rt_* entry points directly (like test_input.cpp), so
// the target links cb_runtime. No display / Allegro is touched.
//
// No trap host is connected in this target (cb_runtime_init is never called),
// so cb_host() returns null: an out-of-bounds access is a silent no-op that
// returns the safe default (0). That "returns the default, never corrupts/
// crashes" property is what these pin; the end-to-end trap-to-exit-1 behaviour
// (host connected) is covered by the cb-driver cli.rs test.

#include "cb_runtime.h"

#include <gtest/gtest.h>

#include <cstdint>

namespace {

// RAII guard so a failed EXPECT doesn't leak the block.
struct Block {
    CbMemblock* m;
    explicit Block(int32_t size) : m(cb_rt_make_memblock(size)) {}
    ~Block() { cb_rt_delete_memblock(m); }
    Block(const Block&) = delete;
    Block& operator=(const Block&) = delete;
};

} // namespace

TEST(Memblock, MakeIsZeroFilledAndSized) {
    Block b(8);
    ASSERT_NE(b.m, nullptr);
    EXPECT_EQ(cb_rt_memblock_size(b.m), 8);
    for (int32_t i = 0; i < 8; ++i) {
        EXPECT_EQ(cb_rt_peek_byte(b.m, i), 0);
    }
}

TEST(Memblock, MakeZeroSize) {
    Block b(0);
    ASSERT_NE(b.m, nullptr);
    EXPECT_EQ(cb_rt_memblock_size(b.m), 0);
}

TEST(Memblock, MakeNegativeSizeReturnsNull) {
    // Negative size traps (no-op with no host) and returns Null.
    CbMemblock* m = cb_rt_make_memblock(-1);
    EXPECT_EQ(m, nullptr);
}

TEST(Memblock, ByteRoundTripAndUnsigned) {
    Block b(4);
    cb_rt_poke_byte(b.m, 0, 0);
    cb_rt_poke_byte(b.m, 1, 255);
    cb_rt_poke_byte(b.m, 2, 0x100 + 7);  // only low 8 bits stored
    EXPECT_EQ(cb_rt_peek_byte(b.m, 0), 0);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 1), 255);  // unsigned, not -1
    EXPECT_EQ(cb_rt_peek_byte(b.m, 2), 7);
}

TEST(Memblock, ShortRoundTripUnsigned) {
    Block b(4);
    cb_rt_poke_short(b.m, 0, 0xFFFF);
    EXPECT_EQ(cb_rt_peek_short(b.m, 0), 65535);  // unsigned, not -1
    cb_rt_poke_short(b.m, 2, 0x1234);
    EXPECT_EQ(cb_rt_peek_short(b.m, 2), 0x1234);
}

TEST(Memblock, IntRoundTripSigned) {
    Block b(8);
    cb_rt_poke_int(b.m, 0, -1);
    EXPECT_EQ(cb_rt_peek_int(b.m, 0), -1);  // signed reinterpret
    cb_rt_poke_int(b.m, 4, 0x7FABCDEF);
    EXPECT_EQ(cb_rt_peek_int(b.m, 4), 0x7FABCDEF);
}

TEST(Memblock, FloatRoundTrip32Bit) {
    Block b(4);
    cb_rt_poke_float(b.m, 0, 1.5);
    EXPECT_DOUBLE_EQ(cb_rt_peek_float(b.m, 0), 1.5);  // exactly representable in f32
    // A value not representable in f32 round-trips through the f32 rounding,
    // not the original f64.
    cb_rt_poke_float(b.m, 0, 0.1);
    EXPECT_FLOAT_EQ(static_cast<float>(cb_rt_peek_float(b.m, 0)), 0.1f);
}

// The on-wire byte order is little-endian regardless of host architecture:
// PokeInt(0x04030201) lays bytes 01 02 03 04 from low to high address.
TEST(Memblock, LittleEndianOnTheWire) {
    Block b(4);
    cb_rt_poke_int(b.m, 0, 0x04030201);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 0), 0x01);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 1), 0x02);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 2), 0x03);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 3), 0x04);

    // And the reverse: laying individual bytes assembles the LE short.
    Block c(2);
    cb_rt_poke_byte(c.m, 0, 0xCD);
    cb_rt_poke_byte(c.m, 1, 0xAB);
    EXPECT_EQ(cb_rt_peek_short(c.m, 0), 0xABCD);
}

TEST(Memblock, ResizeGrowPreservesAndZeroFills) {
    Block b(4);
    cb_rt_poke_int(b.m, 0, 0x11223344);
    cb_rt_resize_memblock(b.m, 8);
    EXPECT_EQ(cb_rt_memblock_size(b.m), 8);
    EXPECT_EQ(cb_rt_peek_int(b.m, 0), 0x11223344);  // preserved
    EXPECT_EQ(cb_rt_peek_int(b.m, 4), 0);           // growth zero-filled
}

TEST(Memblock, ResizeShrinkKeepsPrefix) {
    Block b(8);
    cb_rt_poke_byte(b.m, 0, 0xAA);
    cb_rt_poke_byte(b.m, 1, 0xBB);
    cb_rt_resize_memblock(b.m, 2);
    EXPECT_EQ(cb_rt_memblock_size(b.m), 2);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 0), 0xAA);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 1), 0xBB);
}

TEST(Memblock, MemCopyBetweenBlocks) {
    Block src(4);
    Block dst(8);
    cb_rt_poke_int(src.m, 0, 0xDEADBEEF);
    cb_rt_mem_copy(src.m, 0, dst.m, 4, 4);
    EXPECT_EQ(cb_rt_peek_int(dst.m, 4), static_cast<int32_t>(0xDEADBEEF));
    EXPECT_EQ(cb_rt_peek_int(dst.m, 0), 0);  // untouched
}

TEST(Memblock, MemCopyOverlappingWithinBlock) {
    Block b(8);
    for (int32_t i = 0; i < 4; ++i) cb_rt_poke_byte(b.m, i, i + 1);  // 1 2 3 4
    // Overlapping forward copy: memmove makes this well-defined.
    cb_rt_mem_copy(b.m, 0, b.m, 2, 4);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 2), 1);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 3), 2);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 4), 3);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 5), 4);
}

// Out-of-bounds reads/writes don't corrupt or crash: with no host the trap is a
// no-op and the read returns the safe default (0); the write does nothing.
TEST(Memblock, OutOfBoundsReadReturnsZero) {
    Block b(4);
    cb_rt_poke_int(b.m, 0, 0x12345678);
    EXPECT_EQ(cb_rt_peek_byte(b.m, 4), 0);    // one past the end
    EXPECT_EQ(cb_rt_peek_byte(b.m, -1), 0);   // negative offset
    EXPECT_EQ(cb_rt_peek_int(b.m, 1), 0);     // would cross the end (1+4 > 4)
    EXPECT_EQ(cb_rt_peek_short(b.m, 3), 0);   // 3+2 > 4
}

TEST(Memblock, OutOfBoundsWriteIsNoOp) {
    Block b(4);
    cb_rt_poke_byte(b.m, 10, 0xFF);   // far past the end — ignored
    cb_rt_poke_int(b.m, 2, 0x11223344);  // 2+4 > 4 — ignored, no corruption
    for (int32_t i = 0; i < 4; ++i) {
        EXPECT_EQ(cb_rt_peek_byte(b.m, i), 0);
    }
}

TEST(Memblock, NullHandleQueriesAreSafe) {
    EXPECT_EQ(cb_rt_memblock_size(nullptr), 0);
    EXPECT_EQ(cb_rt_peek_int(nullptr, 0), 0);
    cb_rt_poke_int(nullptr, 0, 1);  // must not crash
    cb_rt_delete_memblock(nullptr); // must not crash
}
