// CoolBasic memory-block runtime (FD-039).
//
// Raw, manually-managed byte buffers for byte-level data manipulation. The
// CB-visible `Memblock` type is the opaque CbMemblock* handle (tag 15):
// MakeMEMBlock returns it, DeleteMEMBlock frees it. Allegro-free, so this TU
// is part of the SDK-free catalog (FD-033) and runs headless in CI.
//
// Two deliberate divergences from classic CoolBasic, both for safety and
// portability:
//
//   * Bounds/handle checking TRAPS. Classic CB Peek/Poke blind-cast and walk
//     off the end of the buffer on a bad offset (undefined behaviour, silent
//     memory corruption). Every access here validates the handle and the
//     offset+width against the block size and, on failure, raises a runtime
//     error through the FD-015 host channel (cb_host()->raise_error) and
//     returns a safe default. A negative MakeMEMBlock/ResizeMEMBlock size and
//     a bad MemCopy range trap the same way.
//
//   * Multi-byte values are LITTLE-ENDIAN on the wire, assembled byte-by-byte
//     rather than by reinterpret-casting host memory, so a memblock's contents
//     are identical regardless of the host architecture's byte order (classic
//     CB ran x86-only and relied on native little-endian layout).
//
// PeekByte/PeekShort return UNSIGNED values (0..255 / 0..65535); PeekInt is a
// signed 32-bit reinterpret. Float is 32-bit on the wire: PokeFloat narrows
// the CB f64 to f32 and PeekFloat widens back.

#include "cb_runtime.h"

#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

// Opaque handle. Kept in the global namespace to match the forward
// declaration in cb_runtime_func.h (the same convention as CbImage/CbObject).
// The byte storage grows and shrinks via ResizeMEMBlock; std::vector gives the
// preserve-existing-bytes + zero-fill-growth semantics MEMBlocks require for
// free.
struct CbMemblock {
    std::vector<uint8_t> bytes;
};

namespace {

// Raise an FD-015 runtime error with `msg`, if a host is connected. The host
// callback copies the message synchronously and returns (it never unwinds),
// so the freshly-made CbString is released right after. With no host (e.g. the
// native gtest target) this is a no-op and the caller falls through to its
// safe default — never UB.
void trap(const std::string& msg) {
    const CbHostApi* h = cb_host();
    if (!h) return;
    CbString* s = cb_rt_string_from_literal(
        reinterpret_cast<const uint8_t*>(msg.data()), msg.size());
    h->raise_error(s);
    cb_rt_string_release(s);
}

// Validate a `width`-byte access at `offset` on `m`. Traps and returns false on
// a null handle, a negative offset, or a range that would cross the block end.
// The `offset < 0` check guards the size_t cast that follows.
bool access_ok(const char* fn, const CbMemblock* m, int32_t offset, size_t width) {
    if (!m) {
        trap(std::string(fn) + ": null memblock handle");
        return false;
    }
    if (offset < 0 || static_cast<size_t>(offset) + width > m->bytes.size()) {
        trap(std::string(fn) + ": offset " + std::to_string(offset) +
             " out of bounds (memblock size " + std::to_string(m->bytes.size()) + ")");
        return false;
    }
    return true;
}

// Little-endian load/store on a validated pointer. Explicit byte assembly so
// the encoding does not depend on the host's native byte order.
uint16_t load_u16_le(const uint8_t* p) {
    return static_cast<uint16_t>(static_cast<uint16_t>(p[0]) |
                                 (static_cast<uint16_t>(p[1]) << 8));
}
uint32_t load_u32_le(const uint8_t* p) {
    return static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) |
           (static_cast<uint32_t>(p[2]) << 16) | (static_cast<uint32_t>(p[3]) << 24);
}
void store_u16_le(uint8_t* p, uint16_t v) {
    p[0] = static_cast<uint8_t>(v & 0xFF);
    p[1] = static_cast<uint8_t>((v >> 8) & 0xFF);
}
void store_u32_le(uint8_t* p, uint32_t v) {
    p[0] = static_cast<uint8_t>(v & 0xFF);
    p[1] = static_cast<uint8_t>((v >> 8) & 0xFF);
    p[2] = static_cast<uint8_t>((v >> 16) & 0xFF);
    p[3] = static_cast<uint8_t>((v >> 24) & 0xFF);
}

} // namespace

// ─── Allocation / lifecycle ───────────────────────────────────────────────

// MakeMEMBlock(size): allocate a zero-filled block of `size` bytes. A negative
// size traps and returns Null.
extern "C" CbMemblock* cb_rt_make_memblock(int32_t size) {
    if (size < 0) {
        trap("MakeMEMBlock: negative size " + std::to_string(size));
        return nullptr;
    }
    return new CbMemblock{std::vector<uint8_t>(static_cast<size_t>(size), 0)};
}

// DeleteMEMBlock(mem): free the block. Null-safe (delete nullptr is a no-op).
extern "C" void cb_rt_delete_memblock(CbMemblock* m) {
    delete m;
}

// ResizeMEMBlock(mem, size): grow or shrink, preserving existing bytes and
// zero-filling any growth. A negative size traps.
extern "C" void cb_rt_resize_memblock(CbMemblock* m, int32_t size) {
    if (!m) {
        trap("ResizeMEMBlock: null memblock handle");
        return;
    }
    if (size < 0) {
        trap("ResizeMEMBlock: negative size " + std::to_string(size));
        return;
    }
    m->bytes.resize(static_cast<size_t>(size), 0);
}

// MEMBlockSize(mem): size in bytes. A null handle traps and returns 0.
extern "C" int32_t cb_rt_memblock_size(const CbMemblock* m) {
    if (!m) {
        trap("MEMBlockSize: null memblock handle");
        return 0;
    }
    return static_cast<int32_t>(m->bytes.size());
}

// MemCopy(srcMem, srcOff, dstMem, dstOff, len): copy `len` bytes between blocks
// (or within one). Traps on a null handle, negative length, or an out-of-range
// source/destination span. memmove (not memcpy) so an in-block copy with
// overlapping ranges is well-defined.
extern "C" void cb_rt_mem_copy(const CbMemblock* src, int32_t src_off,
                               CbMemblock* dst, int32_t dst_off, int32_t len) {
    if (!src || !dst) {
        trap("MemCopy: null memblock handle");
        return;
    }
    if (len < 0) {
        trap("MemCopy: negative length " + std::to_string(len));
        return;
    }
    size_t n = static_cast<size_t>(len);
    if (src_off < 0 || static_cast<size_t>(src_off) + n > src->bytes.size()) {
        trap("MemCopy: source range out of bounds (offset " + std::to_string(src_off) +
             ", length " + std::to_string(len) + ", size " +
             std::to_string(src->bytes.size()) + ")");
        return;
    }
    if (dst_off < 0 || static_cast<size_t>(dst_off) + n > dst->bytes.size()) {
        trap("MemCopy: destination range out of bounds (offset " + std::to_string(dst_off) +
             ", length " + std::to_string(len) + ", size " +
             std::to_string(dst->bytes.size()) + ")");
        return;
    }
    std::memmove(dst->bytes.data() + dst_off, src->bytes.data() + src_off, n);
}

// ─── Peek (read) ───────────────────────────────────────────────────────────

// PeekByte: 8-bit unsigned (0..255).
extern "C" int32_t cb_rt_peek_byte(const CbMemblock* m, int32_t offset) {
    if (!access_ok("PeekByte", m, offset, 1)) return 0;
    return static_cast<int32_t>(m->bytes[static_cast<size_t>(offset)]);
}

// PeekShort: 16-bit unsigned (0..65535), little-endian.
extern "C" int32_t cb_rt_peek_short(const CbMemblock* m, int32_t offset) {
    if (!access_ok("PeekShort", m, offset, 2)) return 0;
    return static_cast<int32_t>(load_u16_le(m->bytes.data() + offset));
}

// PeekInt: 32-bit signed, little-endian.
extern "C" int32_t cb_rt_peek_int(const CbMemblock* m, int32_t offset) {
    if (!access_ok("PeekInt", m, offset, 4)) return 0;
    return static_cast<int32_t>(load_u32_le(m->bytes.data() + offset));
}

// PeekFloat: 32-bit IEEE float (little-endian) widened to the CB f64 Float.
extern "C" double cb_rt_peek_float(const CbMemblock* m, int32_t offset) {
    if (!access_ok("PeekFloat", m, offset, 4)) return 0.0;
    uint32_t bits = load_u32_le(m->bytes.data() + offset);
    float f;
    std::memcpy(&f, &bits, sizeof f);
    return static_cast<double>(f);
}

// ─── Poke (write) ────────────────────────────────────────────────────────

// PokeByte: low 8 bits of `value`.
extern "C" void cb_rt_poke_byte(CbMemblock* m, int32_t offset, int32_t value) {
    if (!access_ok("PokeByte", m, offset, 1)) return;
    m->bytes[static_cast<size_t>(offset)] = static_cast<uint8_t>(value & 0xFF);
}

// PokeShort: low 16 bits of `value`, little-endian.
extern "C" void cb_rt_poke_short(CbMemblock* m, int32_t offset, int32_t value) {
    if (!access_ok("PokeShort", m, offset, 2)) return;
    store_u16_le(m->bytes.data() + offset, static_cast<uint16_t>(value & 0xFFFF));
}

// PokeInt: full 32 bits of `value`, little-endian.
extern "C" void cb_rt_poke_int(CbMemblock* m, int32_t offset, int32_t value) {
    if (!access_ok("PokeInt", m, offset, 4)) return;
    store_u32_le(m->bytes.data() + offset, static_cast<uint32_t>(value));
}

// PokeFloat: CB f64 Float narrowed to 32-bit IEEE, little-endian.
extern "C" void cb_rt_poke_float(CbMemblock* m, int32_t offset, double value) {
    if (!access_ok("PokeFloat", m, offset, 4)) return;
    float f = static_cast<float>(value);
    uint32_t bits;
    std::memcpy(&bits, &f, sizeof bits);
    store_u32_le(m->bytes.data() + offset, bits);
}
