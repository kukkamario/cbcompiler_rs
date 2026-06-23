// CoolBasic sound runtime (FD-041).
//
// The sample-based audio surface: load a file into memory (LoadSound), play a
// sample or stream a file (PlaySound), tweak a playing channel live (SetSound),
// stop it (StopSound), query it (SoundPlaying), and free a sample (DeleteSound).
// Two CB-visible opaque types: `Sound` (CbSound* handle, tag 17) and
// `SoundChannel` (tag 18). Allegro-dependent (allegro_audio + allegro_acodec),
// so this TU lives behind the CB_NO_ALLEGRO guard and is absent from the
// SDK-free catalog (FD-033) — the FD-018 Text/Font pattern, not the Allegro-free
// Memblock/File one.
//
// Semantics mined from cbEnchanted (src/soundinterface.cpp, cbsound.cpp,
// cbchannel.cpp) and the official CoolBasic Help. Deliberate divergences (FD-041):
//
//   * Strict opaque `Sound`/`SoundChannel` types (Null default / Null on load
//     failure) instead of classic CB's plain int32 ids — consistent with
//     Object/Map/Memblock/File. The FD-018 null-opaque→Value::Null mapping makes
//     "Null on failure" work with zero frontend change.
//
//   * `SoundChannel` is a generation-tagged pool handle (cb_sound.h GenPool), not
//     a raw pointer. cbEnchanted reaps a finished channel every frame, so a stale
//     handle is the NORMAL case (the sound ended on its own), not a program bug —
//     the generation check makes SetSound/StopSound/SoundPlaying on it a SAFE
//     silent no-op, never a use-after-free of a recycled slot. `Sound` stays a
//     plain new/delete pointer like every other handle (it isn't auto-reaped, so
//     a stale `Sound` is a genuine bug → trap, matching cbEnchanted's getSound).
//
//   * Graceful audio-less degradation (best-effort init, never abort), unlike
//     cbEnchanted's fatal initializeSounds — the cb_rt_screen `if (!g_display)`
//     headless pattern. On an audio-less host LoadSound/PlaySound return Null,
//     SoundPlaying returns 0, Set/StopSound no-op, and the null-`Sound` trap is
//     SUPPRESSED (a Null-ignoring program runs silently instead of exit-1-ing on
//     a silent CI box).
//
//   * Unified gain = volume/100 for sample and stream (drop cbEnchanted's
//     stream-only `* streamGain`, always ×1.0). No CD-track form. Channels attach
//     directly to al_get_default_mixer() (no shared mixer until Video — OQ7).

#include "cb_runtime.h"
#include "cb_sound.h"

#include <allegro5/allegro.h>
#include <allegro5/allegro_acodec.h>
#include <allegro5/allegro_audio.h>

#include <cstdint>
#include <string>

// ─── Opaque Sound handle (tag 17) ─────────────────────────────────────────
//
// In the global namespace to match the forward declaration in cb_runtime_func.h
// (the CbImage/CbObject/CbFile convention). A loaded sample plus its cached
// native frequency (so the PlaySound pitch→speed-ratio math needs no second
// Allegro query). Created by LoadSound, freed by DeleteSound. `SoundChannel`
// (struct CbChannel) is never defined — it is only ever a packed pool handle
// (a bit pattern the runtime owns), never a real object.
struct CbSound {
    ALLEGRO_SAMPLE* sample;
    uint32_t native_freq;
};

namespace cb::sound {
namespace {

// A playing channel: a one-shot sample instance (PlaySound of a preloaded
// `Sound`) or a streamed file (PlaySound of a filename String). Trivially
// default-constructible so GenPool can reset a reaped slot; the Allegro object
// is destroyed by the reaper before the slot is freed. Mirrors cbchannel.h's
// sample/stream discriminated union, kept as two plain pointers + a tag for
// observability (CLAUDE.md) — the extra pointer per slot is negligible.
enum class PlayType { Sample, Stream };
struct ChannelState {
    PlayType play_type = PlayType::Sample;
    ALLEGRO_SAMPLE_INSTANCE* instance = nullptr;
    ALLEGRO_AUDIO_STREAM* stream = nullptr;
};

GenPool<ChannelState> g_channels;
bool g_audio_ok = false;
bool g_audio_tried = false;

// Best-effort audio init; mirrors cb_gfx.cpp's ensure_init (each step guarded so
// re-entry is free). On any failure g_audio_ok stays false and every entry point
// degrades — FD-041 OQ2, NOT cbEnchanted's fatal abort. Tried once: a host that
// can't init audio won't start succeeding later.
bool ensure_audio_init() {
    if (g_audio_tried) return g_audio_ok;
    g_audio_tried = true;
    if (!al_is_system_installed() && !al_init()) return false;
    if (!al_install_audio()) return false;
    if (!al_init_acodec_addon()) return false;
    if (!al_reserve_samples(512)) return false;  // the 512 cap is cbEnchanted's
    g_audio_ok = true;
    return true;
}

// Raise an FD-015 runtime error with `msg`, if a host is connected. The host
// copies the message synchronously and returns (it never unwinds), so the
// freshly-made CbString is released right after. With no host (gtest) this is a
// no-op and the caller falls through to its safe default.
void trap(const std::string& msg) {
    const CbHostApi* h = cb_host();
    if (!h) return;
    CbString* s = cb_rt_string_from_literal(
        reinterpret_cast<const uint8_t*>(msg.data()), msg.size());
    h->raise_error(s);
    cb_rt_string_release(s);
}

// Raw UTF-8 bytes of a CbString as a C string for Allegro's file loaders (which
// take UTF-8 on every platform). Empty for null.
std::string utf8_bytes(const CbString* s) {
    if (!s) return {};
    std::size_t n = cb_rt_string_len(s);
    const uint8_t* d = cb_rt_string_data(s);
    return std::string(reinterpret_cast<const char*>(d), n);
}

// SoundChannel <-> opaque pointer slot. The runtime owns the bit pattern
// (cb_runtime_core.h), so the packed {index, generation} handle rides in the
// `CbChannel*` rather than being a real pointer.
CbChannel* to_handle(PoolHandle h) {
    return reinterpret_cast<CbChannel*>(
        static_cast<uintptr_t>(encode_handle(h)));
}
ChannelState* lookup(CbChannel* ch) {
    PoolHandle h;
    if (!decode_handle(static_cast<uint64_t>(reinterpret_cast<uintptr_t>(ch)),
                       h)) {
        return nullptr;
    }
    return g_channels.get(h);
}

bool channel_playing(const ChannelState& c) {
    if (c.play_type == PlayType::Sample) {
        return c.instance && al_get_sample_instance_playing(c.instance);
    }
    return c.stream && al_get_audio_stream_playing(c.stream);
}

void destroy_channel(ChannelState& c) {
    if (c.play_type == PlayType::Sample) {
        if (c.instance) al_destroy_sample_instance(c.instance);
    } else {
        if (c.stream) al_destroy_audio_stream(c.stream);
    }
    c.instance = nullptr;
    c.stream = nullptr;
}

}  // namespace

// Per-frame reaper (declared in cb_sound.h, called from cb_gfx.cpp's DrawScreen).
// Frees every channel that has stopped playing and bumps its slot generation, so
// a CB program still holding the handle gets a safe no-op. Mirrors cbEnchanted's
// updateAudio (soundinterface.cpp:146-161).
void reap() {
    for (uint32_t idx = 0; idx < g_channels.capacity(); ++idx) {
        if (!g_channels.occupied(idx)) continue;
        ChannelState& c = g_channels.at(idx);
        if (!channel_playing(c)) {
            destroy_channel(c);
            g_channels.free(g_channels.handle_at(idx));
        }
    }
}

}  // namespace cb::sound

using namespace cb::sound;

// ─── LoadSound / DeleteSound (the `Sound` sample) ─────────────────────────

// LoadSound(path$): load a file fully into memory as a sample. Null on failure
// (missing/undecodable file) or when audio is unavailable. Caches the sample's
// native frequency for the speed math.
extern "C" CbSound* cb_rt_load_sound(const CbString* path) {
    if (!ensure_audio_init()) return nullptr;
    std::string file = utf8_bytes(path);
    ALLEGRO_SAMPLE* sample = al_load_sample(file.c_str());
    if (!sample) return nullptr;
    return new CbSound{sample, al_get_sample_frequency(sample)};
}

// DeleteSound(sound): free a loaded sample. A null/never-loaded `Sound` traps
// (matching cbEnchanted's "Sound access violation") — UNLESS audio is
// unavailable, in which case the trap is suppressed (LoadSound returned Null, so
// a degrade-gracefully program reaches here with Null through no fault of its
// own). Use-after-DeleteSound is UB, like every other raw-pointer handle.
extern "C" void cb_rt_delete_sound(CbSound* sound) {
    bool audio = ensure_audio_init();
    if (!sound) {
        if (audio) trap("DeleteSound: invalid sound handle");
        return;
    }
    if (sound->sample) al_destroy_sample(sound->sample);
    delete sound;
}

// ─── PlaySound — preloaded `Sound` (one-shot sample instance) ─────────────

// Full 4-arg form. Returns a SoundChannel, or Null on any failure / when audio
// is unavailable (the trap on a null Sound is suppressed in that case — see
// DeleteSound). volume/balance/frequency map through cb_sound.h.
extern "C" CbChannel* cb_rt_play_sound4(CbSound* sound, double volume,
                                        double balance, int32_t frequency) {
    if (!ensure_audio_init()) return nullptr;
    if (!sound || !sound->sample) {
        trap("PlaySound: invalid sound handle");
        return nullptr;
    }
    ALLEGRO_SAMPLE_INSTANCE* inst = al_create_sample_instance(sound->sample);
    if (!inst) return nullptr;
    if (!al_attach_sample_instance_to_mixer(inst, al_get_default_mixer())) {
        al_destroy_sample_instance(inst);
        return nullptr;
    }
    al_set_sample_instance_gain(inst, gain(static_cast<float>(volume)));
    al_set_sample_instance_pan(inst, pan(static_cast<float>(balance)));
    if (frequency > 0) {
        al_set_sample_instance_speed(inst, speed(frequency, sound->native_freq));
    }
    al_play_sample_instance(inst);
    ChannelState c;
    c.play_type = PlayType::Sample;
    c.instance = inst;
    return to_handle(g_channels.alloc(c));
}

// Arity overloads → defaults volume=100, balance=0, frequency=-1 (cbchannel.h).
extern "C" CbChannel* cb_rt_play_sound(CbSound* sound) {
    return cb_rt_play_sound4(sound, 100.0, 0.0, -1);
}
extern "C" CbChannel* cb_rt_play_sound2(CbSound* sound, double volume) {
    return cb_rt_play_sound4(sound, volume, 0.0, -1);
}
extern "C" CbChannel* cb_rt_play_sound3(CbSound* sound, double volume,
                                        double balance) {
    return cb_rt_play_sound4(sound, volume, balance, -1);
}

// ─── PlaySound — filename String (streamed file, the "music" path) ────────

// Full 4-arg form. Streams the file (3 buffers × 8192 samples, cbchannel.cpp:130)
// rather than loading it whole. A load failure returns Null with no trap (it is
// a load failure, like LoadSound — not a bad handle).
extern "C" CbChannel* cb_rt_play_sound_file4(const CbString* path, double volume,
                                             double balance, int32_t frequency) {
    if (!ensure_audio_init()) return nullptr;
    std::string file = utf8_bytes(path);
    ALLEGRO_AUDIO_STREAM* stream = al_load_audio_stream(file.c_str(), 3, 8192);
    if (!stream) return nullptr;
    if (!al_attach_audio_stream_to_mixer(stream, al_get_default_mixer())) {
        al_destroy_audio_stream(stream);
        return nullptr;
    }
    uint32_t native = al_get_audio_stream_frequency(stream);
    al_set_audio_stream_gain(stream, gain(static_cast<float>(volume)));
    al_set_audio_stream_pan(stream, pan(static_cast<float>(balance)));
    if (frequency > 0) {
        al_set_audio_stream_speed(stream, speed(frequency, native));
    }
    al_set_audio_stream_playing(stream, true);
    ChannelState c;
    c.play_type = PlayType::Stream;
    c.stream = stream;
    return to_handle(g_channels.alloc(c));
}

extern "C" CbChannel* cb_rt_play_sound_file(const CbString* path) {
    return cb_rt_play_sound_file4(path, 100.0, 0.0, -1);
}
extern "C" CbChannel* cb_rt_play_sound_file2(const CbString* path,
                                             double volume) {
    return cb_rt_play_sound_file4(path, volume, 0.0, -1);
}
extern "C" CbChannel* cb_rt_play_sound_file3(const CbString* path, double volume,
                                             double balance) {
    return cb_rt_play_sound_file4(path, volume, balance, -1);
}

// ─── SetSound / StopSound / SoundPlaying (operate on a `SoundChannel`) ─────

// SetSound(channel, looping, volume, balance, frequency): mutate a playing
// channel live. A stale/finished/null channel is a silent no-op (the generation
// pool rejects it safely) — cbEnchanted's deliberate non-erroring getChannel.
// The CB compiler always supplies the optional args, so the lower-arity forms
// reset volume/balance to their defaults (faithful to classic CB always pushing
// all five) while frequency=-1 leaves the native pitch.
extern "C" void cb_rt_set_sound5(CbChannel* channel, int32_t looping,
                                 double volume, double balance,
                                 int32_t frequency) {
    ChannelState* c = lookup(channel);
    if (!c) return;
    if (c->play_type == PlayType::Sample) {
        if (frequency > 0) {
            uint32_t native = al_get_sample_instance_frequency(c->instance);
            al_set_sample_instance_speed(c->instance, speed(frequency, native));
        }
        al_set_sample_instance_gain(c->instance, gain(static_cast<float>(volume)));
        al_set_sample_instance_pan(c->instance, pan(static_cast<float>(balance)));
        al_set_sample_instance_playmode(
            c->instance, looping ? ALLEGRO_PLAYMODE_LOOP : ALLEGRO_PLAYMODE_ONCE);
    } else {
        if (frequency > 0) {
            uint32_t native = al_get_audio_stream_frequency(c->stream);
            al_set_audio_stream_speed(c->stream, speed(frequency, native));
        }
        al_set_audio_stream_gain(c->stream, gain(static_cast<float>(volume)));
        al_set_audio_stream_pan(c->stream, pan(static_cast<float>(balance)));
        al_set_audio_stream_playmode(
            c->stream, looping ? ALLEGRO_PLAYMODE_LOOP : ALLEGRO_PLAYMODE_ONCE);
    }
}

extern "C" void cb_rt_set_sound(CbChannel* channel, int32_t looping) {
    cb_rt_set_sound5(channel, looping, 100.0, 0.0, -1);
}
extern "C" void cb_rt_set_sound3(CbChannel* channel, int32_t looping,
                                 double volume) {
    cb_rt_set_sound5(channel, looping, volume, 0.0, -1);
}
extern "C" void cb_rt_set_sound4(CbChannel* channel, int32_t looping,
                                 double volume, double balance) {
    cb_rt_set_sound5(channel, looping, volume, balance, -1);
}

// StopSound(channel): stop a playing channel. The reaper collects it next frame.
// Stale/null channel → silent no-op.
extern "C" void cb_rt_stop_sound(CbChannel* channel) {
    ChannelState* c = lookup(channel);
    if (!c) return;
    if (c->play_type == PlayType::Sample) {
        if (c->instance) al_set_sample_instance_playing(c->instance, false);
    } else {
        if (c->stream) al_set_audio_stream_playing(c->stream, false);
    }
}

// SoundPlaying(channel): 1 while the channel is still playing, else 0 (incl. a
// stopped/finished/reaped/null channel).
extern "C" int32_t cb_rt_sound_playing(CbChannel* channel) {
    ChannelState* c = lookup(channel);
    if (!c) return 0;
    return channel_playing(*c) ? 1 : 0;
}
