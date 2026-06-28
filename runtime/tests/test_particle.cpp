// Unit tests for the pure particle-emitter math in cb_particle.h. No
// display / Allegro needed — the header is self-contained (mirrors
// test_object.cpp / test_collision.cpp). These pin the uniform launch direction,
// the per-frame integrate/accelerate/gravity/cull, the density-
// scheduled spawn (and its non-positive-density guard), and the forward, clamped
// animation-frame selection.

#include "cb_particle.h"

#include <gtest/gtest.h>

#include <cmath>
#include <cstdint>

using cb::particle::CbEmitterState;
using cb::particle::CbParticle;

namespace {
constexpr double kEps = 1e-9;
constexpr double kPi = cb::particle::k_pi;
}  // namespace

// ── Launch direction (uniform over ±spread) ──────────────────

// rand01 == 0.5 aims exactly along the emitter's facing angle (offset 0).
TEST(ParticleLaunch, MidRandomIsFacingAngle) {
    EXPECT_NEAR(cb::particle::particle_launch_rad(0.0, 60.0, 0.5), 0.0, kEps);
    EXPECT_NEAR(cb::particle::particle_launch_rad(90.0, 60.0, 0.5), 90.0 * kPi / 180.0,
                kEps);
}

// The full [0,1) range spans the whole [-spread, +spread] sector: rand01==0 is
// +spread, rand01->1 approaches -spread.
TEST(ParticleLaunch, SpansSector) {
    // rand01 == 0  ->  angle + spread.
    EXPECT_NEAR(cb::particle::particle_launch_rad(0.0, 60.0, 0.0), 60.0 * kPi / 180.0,
                kEps);
    // rand01 == 1  ->  angle - spread (the open end).
    EXPECT_NEAR(cb::particle::particle_launch_rad(0.0, 60.0, 1.0), -60.0 * kPi / 180.0,
                kEps);
}

// Spread 0 always fires straight along the facing angle, for any rand01 (a tight
// stream — the Help's "nolla sinkoaa ... vain määrättyyn suuntaan").
TEST(ParticleLaunch, ZeroSpreadIsDeterministic) {
    EXPECT_NEAR(cb::particle::particle_launch_rad(45.0, 0.0, 0.0), 45.0 * kPi / 180.0,
                kEps);
    EXPECT_NEAR(cb::particle::particle_launch_rad(45.0, 0.0, 0.9), 45.0 * kPi / 180.0,
                kEps);
}

// ── Integrate / accelerate / gravity / cull ──────────────────────────────

// One step moves by velocity, scales velocity by acceleration, and subtracts
// gravity from velY (gravity pulls toward -Y, i.e. down on screen).
TEST(ParticleIntegrate, GravityAndAcceleration) {
    CbEmitterState e;
    e.acceleration = 0.5;
    e.gravity = 2.0;
    CbParticle p;
    p.x = 0.0;
    p.y = 0.0;
    p.velX = 10.0;
    p.velY = 4.0;
    p.lifeTime = 5;
    e.particles.push_back(p);

    cb::particle::integrate_and_cull(e);
    ASSERT_EQ(e.particles.size(), 1u);
    const CbParticle& q = e.particles[0];
    EXPECT_NEAR(q.x, 10.0, kEps);                 // x += velX
    EXPECT_NEAR(q.y, 4.0, kEps);                  // y += velY
    EXPECT_NEAR(q.velX, 10.0 * 0.5, kEps);        // velX *= accel
    EXPECT_NEAR(q.velY, 4.0 * 0.5 - 2.0, kEps);   // velY = velY*accel - gravity
    EXPECT_EQ(q.lifeTime, 4);                     // life--
}

// A particle is culled the step its life decrements below zero (life 0 -> -1).
TEST(ParticleIntegrate, CullsAtEndOfLife) {
    CbEmitterState e;
    CbParticle p;
    p.lifeTime = 0;
    e.particles.push_back(p);
    cb::particle::integrate_and_cull(e);
    EXPECT_TRUE(e.particles.empty());
}

// Compaction preserves order and keeps only the survivors.
TEST(ParticleIntegrate, CompactionKeepsOrder) {
    CbEmitterState e;
    for (int i = 0; i < 4; ++i) {
        CbParticle p;
        p.x = (double)i;          // tag by start x
        p.lifeTime = (i % 2);     // even -> life 0 (culled), odd -> life 1 (kept)
        e.particles.push_back(p);
    }
    cb::particle::integrate_and_cull(e);
    ASSERT_EQ(e.particles.size(), 2u);
    EXPECT_NEAR(e.particles[0].x, 1.0, kEps);
    EXPECT_NEAR(e.particles[1].x, 3.0, kEps);
}

// ── Spawn scheduling ─────────────────────────────────────────────────────

// Each density interval crossed spawns `count` particles. With density 5 and the
// counter bumped to 11, two intervals are due -> 2*count particles.
TEST(ParticleSpawn, DensitySchedule) {
    CbEmitterState e;
    e.density = 5.0;
    e.count = 3;
    e.particleLifeTime = 10;
    e.spawnCounter = 11.0;
    cb::particle::spawn_due(e, 0.0, 0.0, 0.0, [] { return 0.5; });
    EXPECT_EQ(e.particles.size(), 6u);            // 2 emissions * count 3
    EXPECT_NEAR(e.spawnCounter, 1.0, kEps);       // 11 - 5 - 5
    for (const CbParticle& p : e.particles) EXPECT_EQ(p.lifeTime, 10);
}

// Below one interval, nothing spawns.
TEST(ParticleSpawn, NothingBelowDensity) {
    CbEmitterState e;
    e.density = 5.0;
    e.spawnCounter = 4.0;
    cb::particle::spawn_due(e, 0.0, 0.0, 0.0, [] { return 0.5; });
    EXPECT_TRUE(e.particles.empty());
    EXPECT_NEAR(e.spawnCounter, 4.0, kEps);
}

// Spawn velocity follows the launch direction; with rand01 0.5 and angle 0 the
// particle flies straight along +X at `speed`.
TEST(ParticleSpawn, VelocityFromSpeedAndAngle) {
    CbEmitterState e;
    e.density = 1.0;
    e.count = 1;
    e.speed = 7.0;
    e.spread = 90.0;
    e.spawnCounter = 1.5;  // > density, one emission due; remainder 0.5
    cb::particle::spawn_due(e, 100.0, 50.0, 0.0, [] { return 0.5; });
    ASSERT_EQ(e.particles.size(), 1u);
    const CbParticle& p = e.particles[0];
    EXPECT_NEAR(p.velX, 7.0, kEps);                 // cos(0)*speed
    EXPECT_NEAR(p.velY, 0.0, kEps);                 // sin(0)*speed
    EXPECT_NEAR(p.x, 100.0 + 7.0 * 0.5, kEps);      // pos + vel*spawnCounter
    EXPECT_NEAR(p.y, 50.0, kEps);
}

// A non-positive density must not spawn (and must not hang): the guard against
// an infinite `while (counter > density)` loop.
TEST(ParticleSpawn, NonPositiveDensityGuard) {
    CbEmitterState e;
    e.density = 0.0;
    e.count = 4;
    e.spawnCounter = 100.0;
    cb::particle::spawn_due(e, 0.0, 0.0, 0.0, [] { return 0.5; });
    EXPECT_TRUE(e.particles.empty());
}

// ── Animation frame selection (forward, clamped) ─────────────

// Forward: a freshly spawned particle (remaining == total) is on frame 0; it
// advances toward the last frame as it ages.
TEST(ParticleFrame, ForwardOverLife) {
    // total 10, 8 frames.
    EXPECT_EQ(cb::particle::particle_frame(10, 10, 8), 0);  // age 0
    EXPECT_EQ(cb::particle::particle_frame(5, 10, 8), 4);   // age 5 -> 0.5*8
    EXPECT_EQ(cb::particle::particle_frame(1, 10, 8), 7);   // age 9 -> 7.2 -> 7
}

// The index is clamped to frameCount-1: a dead/over-aged particle never reads
// past the strip (classic CB crashes here; we clamp).
TEST(ParticleFrame, ClampsToLastFrame) {
    EXPECT_EQ(cb::particle::particle_frame(0, 10, 8), 7);    // age == total
    EXPECT_EQ(cb::particle::particle_frame(-3, 10, 8), 7);   // over-aged
}

// Degenerate inputs collapse to frame 0 (static particle).
TEST(ParticleFrame, DegenerateIsZero) {
    EXPECT_EQ(cb::particle::particle_frame(5, 10, 1), 0);   // single frame
    EXPECT_EQ(cb::particle::particle_frame(5, 10, 0), 0);   // no animation
    EXPECT_EQ(cb::particle::particle_frame(5, 0, 8), 0);    // zero total life
}
