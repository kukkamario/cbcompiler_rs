#ifndef CB_PARTICLE_H
#define CB_PARTICLE_H

// Pure, Allegro-free particle-emitter simulation (FD-038).
//
// A CoolBasic particle emitter IS an Object: MakeEmitter returns the `Object`
// opaque type (tag 13), and the emitter is moved/rotated/deleted with the
// ordinary object commands. cb_object.cpp owns the live emitter (the registry,
// the texture, the render pass, and the catalog entry points), distinguishing
// it from a plain object by a non-null `emitter` payload on CbObject. THIS
// header holds only the headless-testable state + math — the spawn direction,
// the gravity/acceleration integration, the cull, and the animation-frame
// selection — so they unit-test without a display (the cb_*_data.h pattern).
//
// Faithful to cbEnchanted's CBParticleEmitter::updateObject, with two
// deliberate divergences (FD-038 ledger):
//   • Animation plays FORWARD (frame 0 at spawn → last at death), per the
//     CoolBasic Help ("kuvasarja jakautuu nollasta ... kun elämä loppuu");
//     cbEnchanted played it in reverse (it fed remaining life straight in).
//   • Spawning is guarded against a non-positive density (cbEnchanted would
//     spin forever: spawnCounter never falls back below a density <= 0).

#include <cmath>
#include <cstdint>
#include <vector>

namespace cb::particle {

inline constexpr double k_pi = 3.14159265358979323846;

// One live particle. Coordinates are world-space, the same space as objects
// (+Y up; the renderer applies the -Y flip). `lifeTime` is frames remaining and
// is culled once it drops below zero.
struct CbParticle {
    double x = 0.0, y = 0.0;
    double velX = 0.0, velY = 0.0;
    int32_t lifeTime = 0;
};

// All emitter state beyond the inherited CbObject fields (posX/posY/angle/life).
// Defaults mirror CBParticleEmitter's constructor.
struct CbEmitterState {
    // create() — MakeEmitter(image, lifeTime)
    int32_t particleLifeTime = 0;  // per-PARTICLE life, in frames
    // setParticleMovement(speed, gravity[, accel])
    double speed = 0.0;
    double gravity = 0.0;
    double acceleration = 1.0;     // per-frame velocity scale; 1.0 = constant
    // setParticleEmission(density, count, spread)
    double density = 1.0;          // frames between emissions (smaller = denser)
    int32_t count = 1;             // particles spawned per emission
    double spread = 0.0;           // ± sector half-angle in degrees (0..180)
    // setParticleAnimation(frameCount)
    int32_t frameCount = 0;        // animation strip length (0/1 = static)
    // live state
    double spawnCounter = 0.0;
    bool stop = false;             // set by DeleteObject; drains, then frees
    std::vector<CbParticle> particles;
};

// Launch direction for one particle, in radians. Uniform over [-spread, +spread]
// around the emitter facing angle (FD-038 OQ4): for rand01 in [0,1),
//   pa = (angle + spread - rand01*spread*2) * π/180.
// rand01 == 0.5 yields exactly the facing direction; the full [0,1) range spans
// the whole sector. Matches cbEnchanted.
inline double particle_launch_rad(double emitter_angle_deg, double spread_deg,
                                  double rand01) {
    return (emitter_angle_deg + spread_deg - rand01 * spread_deg * 2.0) * k_pi /
           180.0;
}

// Advance + cull every live particle by one frame:
//   x += velX; y += velY; velX *= accel; velY = velY*accel - gravity; lifeTime--.
// A particle is removed once its (decremented) life drops below zero. Order is
// preserved (a stable compaction, equivalent to cbEnchanted's iterator erase).
inline void integrate_and_cull(CbEmitterState& e) {
    std::size_t w = 0;
    for (std::size_t r = 0; r < e.particles.size(); ++r) {
        CbParticle p = e.particles[r];
        p.x += p.velX;
        p.y += p.velY;
        p.velX = p.velX * e.acceleration;
        p.velY = p.velY * e.acceleration - e.gravity;
        p.lifeTime -= 1;
        if (p.lifeTime >= 0) e.particles[w++] = p;
    }
    e.particles.resize(w);
}

// Spawn every emission due this frame. The caller bumps spawnCounter by 1 per
// frame; each emission of `count` particles consumes one `density` interval.
// `rand01()` must yield a fresh uniform [0,1) per particle. A non-positive
// density spawns nothing (guard against the cbEnchanted infinite loop). New
// particles are seeded at pos + vel*spawnCounter (the sub-frame offset).
template <typename Rand>
inline void spawn_due(CbEmitterState& e, double posX, double posY,
                      double emitter_angle_deg, Rand&& rand01) {
    if (e.density <= 0.0) return;
    while (e.spawnCounter > e.density) {
        e.spawnCounter -= e.density;
        for (int32_t i = 0; i < e.count; ++i) {
            CbParticle p;
            p.lifeTime = e.particleLifeTime;
            double pa = particle_launch_rad(emitter_angle_deg, e.spread, rand01());
            p.velX = std::cos(pa) * e.speed;
            p.velY = std::sin(pa) * e.speed;
            p.x = posX + p.velX * e.spawnCounter;
            p.y = posY + p.velY * e.spawnCounter;
            e.particles.push_back(p);
        }
    }
}

// Animation frame index for a particle, clamped to [0, frameCount-1] (FD-038
// OQ5: classic CB crashes on an over-running index; we clamp). Plays forward:
// age = total - remaining, frame = floor(age/total * frameCount).
inline int32_t particle_frame(int32_t remaining_life, int32_t total_life,
                              int32_t frame_count) {
    if (frame_count <= 1 || total_life <= 0) return 0;
    int32_t age = total_life - remaining_life;
    if (age < 0) age = 0;
    int32_t f = (int32_t)((double)age / (double)total_life * (double)frame_count);
    if (f < 0) f = 0;
    if (f > frame_count - 1) f = frame_count - 1;
    return f;
}

}  // namespace cb::particle

#endif  // CB_PARTICLE_H
