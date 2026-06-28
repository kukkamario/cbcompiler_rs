// Unit tests for the pure sound helpers in cb_sound.h. No audio device /
// Allegro needed — the header is self-contained (mirrors test_particle.cpp /
// test_map.cpp). Two concerns: the parameter-mapping math (volume→gain,
// balance→pan, frequency→speed-ratio) and the generation-tagged channel pool
// (alloc → reap-bump → stale-reject → slot-reuse), the headless analogue of the
// interpreter's heap.rs Slab tests. The actual play/stop/reap of real Allegro
// instances needs a device → that smoke is deferred.

#include "cb_sound.h"

#include <gtest/gtest.h>

#include <cstdint>

using cb::sound::GenPool;
using cb::sound::PoolHandle;

namespace {
constexpr float kEps = 1e-6f;
}  // namespace

// ── Parameter mapping (unified) ──────────────────────────────────────────────

// gain = volume / 100. 100 → 1.0 (full), 0 → silent, 50 → half.
TEST(SoundGain, VolumeOverHundred) {
    EXPECT_NEAR(cb::sound::gain(100.0f), 1.0f, kEps);
    EXPECT_NEAR(cb::sound::gain(0.0f), 0.0f, kEps);
    EXPECT_NEAR(cb::sound::gain(50.0f), 0.5f, kEps);
    // Above 100 is allowed (amplification); the helper does not clamp gain.
    EXPECT_NEAR(cb::sound::gain(200.0f), 2.0f, kEps);
}

// pan = balance / 100, centered at 0, ±100 → ±1 (full left/right).
TEST(SoundPan, BalanceOverHundred) {
    EXPECT_NEAR(cb::sound::pan(0.0f), 0.0f, kEps);
    EXPECT_NEAR(cb::sound::pan(100.0f), 1.0f, kEps);
    EXPECT_NEAR(cb::sound::pan(-100.0f), -1.0f, kEps);
    EXPECT_NEAR(cb::sound::pan(50.0f), 0.5f, kEps);
}

// An out-of-range balance clamps to [-1, 1] instead of letting Allegro reject the
// pan (out-of-range values are clamped, not rejected).
TEST(SoundPan, OutOfRangeClamps) {
    EXPECT_NEAR(cb::sound::pan(250.0f), 1.0f, kEps);
    EXPECT_NEAR(cb::sound::pan(-250.0f), -1.0f, kEps);
}

// speed = freq / native (a target Hz expressed as a playback-rate ratio).
TEST(SoundSpeed, TargetOverNative) {
    EXPECT_NEAR(cb::sound::speed(44100, 44100), 1.0f, kEps);
    EXPECT_NEAR(cb::sound::speed(22050, 44100), 0.5f, kEps);
    EXPECT_NEAR(cb::sound::speed(88200, 44100), 2.0f, kEps);
}

// freq <= 0 (the default -1) leaves the native rate (ratio 1.0). A zero native
// frequency degrades to 1.0 rather than dividing by zero.
TEST(SoundSpeed, NonPositiveLeavesNative) {
    EXPECT_NEAR(cb::sound::speed(-1, 44100), 1.0f, kEps);
    EXPECT_NEAR(cb::sound::speed(0, 44100), 1.0f, kEps);
    EXPECT_NEAR(cb::sound::speed(44100, 0), 1.0f, kEps);
}

// ── Handle encoding (all-zero reserved for Null) ─────────────────────────────

// A live handle is never the all-zero (Null) bit pattern: the low half is
// index + 1, so even {index 0, generation 0} encodes non-zero.
TEST(SoundHandle, ZeroReservedForNull) {
    PoolHandle decoded;
    EXPECT_FALSE(cb::sound::decode_handle(0, decoded));  // Null

    uint64_t enc = cb::sound::encode_handle(PoolHandle{0, 0});
    EXPECT_NE(enc, 0u);
    ASSERT_TRUE(cb::sound::decode_handle(enc, decoded));
    EXPECT_EQ(decoded.index, 0u);
    EXPECT_EQ(decoded.generation, 0u);
}

// Index and generation survive a round-trip through the packed encoding.
TEST(SoundHandle, RoundTrip) {
    PoolHandle decoded;
    uint64_t enc = cb::sound::encode_handle(PoolHandle{7, 3});
    ASSERT_TRUE(cb::sound::decode_handle(enc, decoded));
    EXPECT_EQ(decoded.index, 7u);
    EXPECT_EQ(decoded.generation, 3u);
}

// ── Generation pool liveness (mirrors heap.rs Slab tests) ────────────────────

// A freshly-allocated handle resolves to its payload.
TEST(SoundPool, AllocThenGetResolves) {
    GenPool<int> pool;
    PoolHandle h = pool.alloc(42);
    int* p = pool.get(h);
    ASSERT_NE(p, nullptr);
    EXPECT_EQ(*p, 42);
}

// A freed handle is rejected: get() returns null (the safe silent-no-op path),
// like heap.rs's freed_handle_is_rejected.
TEST(SoundPool, FreedHandleIsRejected) {
    GenPool<int> pool;
    PoolHandle h = pool.alloc(42);
    pool.free(h);
    EXPECT_EQ(pool.get(h), nullptr);
}

// Freeing the only slot returns it to the free-list; the next alloc reuses the
// same index with a BUMPED generation. The stale handle stays rejected while the
// new handle resolves — heap.rs's slot_reused_with_bumped_generation.
TEST(SoundPool, SlotReusedWithBumpedGeneration) {
    GenPool<int> pool;
    PoolHandle h0 = pool.alloc(1);
    pool.free(h0);
    PoolHandle h1 = pool.alloc(2);

    EXPECT_EQ(h1.index, h0.index);             // same slot
    EXPECT_EQ(h1.generation, h0.generation + 1);  // bumped

    EXPECT_EQ(pool.get(h0), nullptr);          // stale handle rejected
    int* p = pool.get(h1);
    ASSERT_NE(p, nullptr);
    EXPECT_EQ(*p, 2);
}

// Distinct live handles coexist and the free-list reuses slots LIFO.
TEST(SoundPool, DistinctHandlesCoexist) {
    GenPool<int> pool;
    PoolHandle a = pool.alloc(10);
    PoolHandle b = pool.alloc(20);
    EXPECT_NE(a.index, b.index);
    ASSERT_NE(pool.get(a), nullptr);
    ASSERT_NE(pool.get(b), nullptr);
    EXPECT_EQ(*pool.get(a), 10);
    EXPECT_EQ(*pool.get(b), 20);

    // Free b then alloc — LIFO reuse hands back b's slot first.
    pool.free(b);
    PoolHandle c = pool.alloc(30);
    EXPECT_EQ(c.index, b.index);
    EXPECT_EQ(pool.get(b), nullptr);  // b is stale
    EXPECT_NE(pool.get(a), nullptr);  // a untouched
    EXPECT_EQ(*pool.get(a), 10);
}

// A Null-equivalent handle (decode fails) never resolves through the pool: a
// zero-decoded handle would map to index/gen that the empty pool rejects.
TEST(SoundPool, EmptyPoolRejectsAnyHandle) {
    GenPool<int> pool;
    EXPECT_EQ(pool.get(PoolHandle{0, 0}), nullptr);
    EXPECT_EQ(pool.get(PoolHandle{5, 0}), nullptr);
}
