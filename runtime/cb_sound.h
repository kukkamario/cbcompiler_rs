#ifndef CB_SOUND_H
#define CB_SOUND_H

// Pure, Allegro-free sound helpers.
//
// cb_sound.cpp owns the live audio (the loaded samples, the playing channels,
// the Allegro init, and the catalog entry points). THIS header holds only the
// headless-testable parts so they unit-test without an audio device:
//
//   • The parameter-mapping math — CB's 0..100 volume/balance scale and an
//     absolute target-frequency-in-Hz mapped onto Allegro's gain (0..1),
//     pan (-1..1), and speed-ratio knobs.
//   • A generation-tagged handle pool (GenPool) mirroring the interpreter's
//     heap.rs Slab. A finished channel is auto-reaped every frame, so a stale
//     `SoundChannel` handle is the NORMAL case (the sound just ended) — the
//     generation check turns it into a safe silent no-op instead of a
//     use-after-free of a recycled slot.
//
// The math is unified: a single gain(volume) = volume/100 used by both the
// sample and stream paths and by PlaySound and SetSound alike.

#include <cstddef>
#include <cstdint>
#include <vector>

namespace cb::sound {

// ─── Parameter mapping (pure) ─────────────────────────────────────────────

// CB volume (0..100) → Allegro gain. 100 → 1.0.
inline float gain(float volume) { return volume / 100.0f; }

// CB balance (-100..100) → Allegro pan (-1..1). Clamped so an out-of-range
// balance degrades gracefully (Allegro rejects a pan outside [-1,1]; out-of-range
// values are clamped, not rejected).
inline float pan(float balance) {
    float p = balance / 100.0f;
    if (p < -1.0f) p = -1.0f;
    if (p > 1.0f) p = 1.0f;
    return p;
}

// CB frequency (absolute target Hz) → Allegro speed ratio freq / native. A
// non-positive freq (the default -1) leaves the native rate untouched (ratio
// 1.0).
inline float speed(int32_t freq, uint32_t native_freq) {
    if (freq <= 0 || native_freq == 0) return 1.0f;
    return static_cast<float>(freq) / static_cast<float>(native_freq);
}

// ─── Generation-tagged channel pool ───────────────────────────────────────
//
// Mirrors crates/cb-backend-interp/src/heap.rs Slab: parallel entries /
// occupied / generations / free_list vectors. alloc pops a free slot (reusing
// its current generation) or grows; free() empties the slot, BUMPS its
// generation (this is what invalidates every outstanding handle to it), and
// returns it to the free-list; get() rejects a handle whose generation no longer
// matches. No per-play heap allocation and no pointer-reuse hazard.

struct PoolHandle {
    uint32_t index = 0;
    uint32_t generation = 0;
};

// Pack a handle into the pointer-sized opaque `SoundChannel` slot and back. The
// FFI carries opaque handles as a u64 bit pattern (cb_runtime_core.h permits a
// non-pointer encoding). All-zero is reserved for Null, so the low half holds
// index + 1 — a live handle is therefore never 0, and Null/0 decodes as invalid.
inline uint64_t encode_handle(PoolHandle h) {
    return (static_cast<uint64_t>(h.generation) << 32) |
           (static_cast<uint64_t>(h.index) + 1u);
}

inline bool decode_handle(uint64_t bits, PoolHandle& out) {
    uint32_t low = static_cast<uint32_t>(bits & 0xFFFFFFFFu);
    if (low == 0) return false;  // Null / never-assigned
    out.index = low - 1u;
    out.generation = static_cast<uint32_t>(bits >> 32);
    return true;
}

// A generation slab over an arbitrary payload. Templated so the liveness logic
// (alloc → reap-bump → stale-reject → slot-reuse) is unit-testable with a dummy
// payload (GenPool<int>), exactly as heap.rs's slab tests do, without pulling in
// the Allegro-bearing ChannelState.
template <typename Payload>
class GenPool {
public:
    PoolHandle alloc(Payload value) {
        if (!free_list_.empty()) {
            uint32_t idx = free_list_.back();
            free_list_.pop_back();
            entries_[idx] = value;
            occupied_[idx] = true;
            return PoolHandle{idx, generations_[idx]};
        }
        uint32_t idx = static_cast<uint32_t>(entries_.size());
        entries_.push_back(value);
        occupied_.push_back(true);
        generations_.push_back(0);
        return PoolHandle{idx, 0};
    }

    // The live payload for `h`, or nullptr if the slot is empty or `h` is stale
    // (its generation was bumped by a reap) — the safe-no-op path.
    Payload* get(PoolHandle h) {
        if (h.index >= entries_.size()) return nullptr;
        if (!occupied_[h.index]) return nullptr;
        if (generations_[h.index] != h.generation) return nullptr;
        return &entries_[h.index];
    }

    void free(PoolHandle h) {
        if (h.index >= entries_.size()) return;
        if (!occupied_[h.index] || generations_[h.index] != h.generation) return;
        occupied_[h.index] = false;
        generations_[h.index] += 1;  // bump → every old handle now rejected
        entries_[h.index] = Payload{};
        free_list_.push_back(h.index);
    }

    // Iteration support for the per-frame reaper.
    std::size_t capacity() const { return entries_.size(); }
    bool occupied(uint32_t idx) const {
        return idx < occupied_.size() && occupied_[idx];
    }
    Payload& at(uint32_t idx) { return entries_[idx]; }
    PoolHandle handle_at(uint32_t idx) const {
        return PoolHandle{idx, generations_[idx]};
    }

private:
    std::vector<Payload> entries_;
    std::vector<bool> occupied_;
    std::vector<uint32_t> generations_;
    std::vector<uint32_t> free_list_;
};

// Per-frame channel reaper (defined in cb_sound.cpp). Returns finished channels
// to the pool, bumping their generation. Declared here — Allegro-free — so the
// DrawScreen frame hook (cb_gfx.cpp) can call it without including audio headers.
void reap();

// At-exit flush (defined in cb_sound.cpp): destroy every live channel
// unconditionally — the teardown counterpart to reap(). Called from the
// graphics about_to_exit teardown before al_uninstall_system tears audio down.
// Allegro-free declaration so cb_gfx.cpp can call it without audio headers.
void flush_all();

}  // namespace cb::sound

#endif  // CB_SOUND_H
