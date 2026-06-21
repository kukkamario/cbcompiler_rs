// CoolBasic file-I/O runtime (FD-040).
//
// Binary and text file access plus filesystem/directory queries. The CB-visible
// `File` type is the opaque CbFile* handle (tag 16): OpenToRead/Write/Edit
// return it, CloseFile frees it. Allegro-free (uses only <cstdio> + <filesystem>),
// so this TU is part of the SDK-free catalog (FD-033) and runs headless in CI.
//
// Semantics were mined from the cbEnchanted reference (src/fileinterface.cpp)
// and the official CoolBasic Help; the divergences below are all deliberate
// safety/correctness improvements over classic CB (see FD-040):
//
//   * Opaque handle (tag 16, Null default / Null on open failure) instead of
//     classic CB's integer file ids. The FD-018 null-opaque->Value::Null mapping
//     makes "Null on failure" work with zero frontend change.
//
//   * Reads are LENIENT at end-of-data: a read at/past EOF returns a zero value
//     (0 / 0.0 / "") and zero-fills any missing bytes of a multi-byte read, so a
//     `While Not EOF(f) ... Wend` loop never trips on a short tail. Classic CB
//     returns uninitialised garbage here (and ReadByte returns 255); we return a
//     defined default.
//
//   * Genuinely invalid use TRAPS via the FD-015 host channel (exit 1): a null
//     handle, and a wrong-mode op (writing a Read handle / reading a Write
//     handle). Classic CB is permissive (uses the FILE* as-is). With no host
//     connected (the native gtest target) the trap is a no-op and the caller
//     falls through to its safe default — never UB.
//
//   * Multi-byte values are LITTLE-ENDIAN on the wire, assembled byte-by-byte
//     (not by reinterpret-casting host memory), so files are byte-identical
//     across host byte orders and byte-compatible with classic (x86 LE) CB.
//     ReadByte/ReadShort are unsigned (0..255 / 0..65535), ReadInt signed; Float
//     is 32-bit on the wire (WriteFloat narrows the CB f64 -> f32).
//
//   * ReadString reads exactly the prefixed length (preserving embedded NULs)
//     and guards a negative/over-long prefix; classic CB truncates at the first
//     NUL and crashes on a bad length. ReadLine handles LF, CR, and CRLF
//     uniformly; classic CB only breaks on CR or EOF (mis-reading Unix files).
//
//   * On-disk string content is the CbString's raw UTF-8 bytes (our string ABI);
//     classic CB wrote CP1252. Identical for ASCII; non-ASCII content differs.
//
//   * FindFile yields real directory entries only (no "."/".."), "" when done,
//     over a single global cursor on the current directory. CurrentDir keeps a
//     trailing separator. CopyFile refuses to overwrite an existing destination
//     (traps). Execute matches cbEnchanted: system("start"/"xdg-open" + cmd).

#include "cb_runtime.h"

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <string>

// Open mode, recorded so read/write ops can enforce direction and so the
// random-access (OpenToEdit) interleave gets the C-required seek between a read
// and a write on the same stream.
enum class FileMode { Read, Write, Edit };

// Opaque handle, in the global namespace to match the forward declaration in
// cb_runtime_func.h (the CbImage/CbObject/CbMemblock convention). `last_op`
// tracks the previous I/O direction (0 none / 1 read / 2 write) for the Edit
// interleave seek.
struct CbFile {
    std::FILE* fp;
    FileMode   mode;
    int        last_op;
};

namespace {

// Raise an FD-015 runtime error with `msg`, if a host is connected. The host
// callback copies the message synchronously and returns (it never unwinds), so
// the freshly-made CbString is released right after. With no host (gtest) this
// is a no-op and the caller falls through to its safe default.
void trap(const std::string& msg) {
    const CbHostApi* h = cb_host();
    if (!h) return;
    CbString* s = cb_rt_string_from_literal(
        reinterpret_cast<const uint8_t*>(msg.data()), msg.size());
    h->raise_error(s);
    cb_rt_string_release(s);
}

// ─── String / path marshalling ───────────────────────────────────────────

// Raw bytes of a CbString (UTF-8), or empty for null.
std::string utf8_bytes(const CbString* s) {
    if (!s) return {};
    std::size_t n = cb_rt_string_len(s);
    const uint8_t* d = cb_rt_string_data(s);
    return std::string(reinterpret_cast<const char*>(d), n);
}

// A CbString's UTF-8 bytes as a filesystem path. Building from std::u8string
// makes std::filesystem treat the bytes as UTF-8 on every platform (so Windows
// non-ASCII paths work), avoiding the deprecated u8path.
std::filesystem::path to_path(const CbString* s) {
    if (!s) return {};
    std::size_t n = cb_rt_string_len(s);
    const uint8_t* d = cb_rt_string_data(s);
    return std::filesystem::path(std::u8string(reinterpret_cast<const char8_t*>(d), n));
}

// Own a CbString from raw bytes (refcount 1, or the immortal empty sentinel).
CbString* make_string(const std::string& s) {
    return cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(s.data()), s.size());
}

// Open with a UTF-8 path. On Windows, route through _wfopen with the wide path
// so Unicode filenames open correctly (narrow fopen would use the ANSI code
// page); elsewhere the UTF-8 bytes go straight to fopen.
std::FILE* do_open(const CbString* path, const char* mode) {
#ifdef _WIN32
    std::filesystem::path p = to_path(path);
    wchar_t wmode[8] = {0};
    for (std::size_t i = 0; mode[i] && i < 7; ++i) {
        wmode[i] = static_cast<wchar_t>(mode[i]);
    }
    return _wfopen(p.c_str(), wmode);
#else
    std::string b = utf8_bytes(path);
    return std::fopen(b.c_str(), mode);
#endif
}

// ─── Handle / mode validation ────────────────────────────────────────────

bool check_handle(const char* fn, const CbFile* f) {
    if (!f || !f->fp) {
        trap(std::string(fn) + ": invalid file handle");
        return false;
    }
    return true;
}

bool check_read(const char* fn, CbFile* f) {
    if (!check_handle(fn, f)) return false;
    if (f->mode == FileMode::Write) {
        trap(std::string(fn) + ": file is not open for reading");
        return false;
    }
    return true;
}

bool check_write(const char* fn, CbFile* f) {
    if (!check_handle(fn, f)) return false;
    if (f->mode == FileMode::Read) {
        trap(std::string(fn) + ": file is not open for writing");
        return false;
    }
    return true;
}

// C requires a positioning call between a read and a write (and vice versa) on a
// read/write stream. Insert a no-op seek on a direction switch in Edit mode.
void prepare(CbFile* f, int op) {
    if (f->mode == FileMode::Edit && f->last_op != 0 && f->last_op != op) {
        std::fseek(f->fp, 0, SEEK_CUR);
    }
    f->last_op = op;
}

// ─── Little-endian load/store (host-byte-order-independent) ────────────────

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

// Lenient fixed-width read: zero-fill `buf` then read up to `n` bytes. Missing
// bytes stay zero (end-of-data leniency).
void read_le(CbFile* f, uint8_t* buf, std::size_t n) {
    std::memset(buf, 0, n);
    std::fread(buf, 1, n, f->fp);
}

// Global directory-search cursor (StartSearch/FindFile/EndSearch). Single,
// non-reentrant — matches classic CB.
std::filesystem::directory_iterator g_search;

} // namespace

// ─── Open / close / position ───────────────────────────────────────────────

// OpenToRead(path): read-only. Null on failure (missing file / is a directory).
extern "C" CbFile* cb_rt_open_to_read(const CbString* path) {
    std::FILE* fp = do_open(path, "rb");
    if (!fp) return nullptr;
    return new CbFile{fp, FileMode::Read, 0};
}

// OpenToWrite(path): write-only, create or truncate. Null on failure.
extern "C" CbFile* cb_rt_open_to_write(const CbString* path) {
    std::FILE* fp = do_open(path, "wb");
    if (!fp) return nullptr;
    return new CbFile{fp, FileMode::Write, 0};
}

// OpenToEdit(path): read+write random access. "rb+" if the file exists (no
// truncation), else "wb+" to create it empty. Null on failure.
extern "C" CbFile* cb_rt_open_to_edit(const CbString* path) {
    std::error_code ec;
    const bool exists = std::filesystem::exists(to_path(path), ec);
    std::FILE* fp = do_open(path, exists ? "rb+" : "wb+");
    if (!fp) return nullptr;
    return new CbFile{fp, FileMode::Edit, 0};
}

// CloseFile(f): close and free the handle. Like DeleteMEMBlock, this frees the
// handle; using it afterward is a use-after-free (the user's bug), same as
// classic CB. A null handle traps.
extern "C" void cb_rt_close_file(CbFile* f) {
    if (!f) {
        trap("CloseFile: invalid file handle");
        return;
    }
    if (f->fp) std::fclose(f->fp);
    delete f;
}

// SeekFile(f, pos): absolute seek from the start. The seek also satisfies the
// Edit read<->write interleave, so reset last_op.
extern "C" void cb_rt_seek_file(CbFile* f, int32_t pos) {
    if (!check_handle("SeekFile", f)) return;
    std::fseek(f->fp, static_cast<long>(pos), SEEK_SET);
    f->last_op = 0;
}

// FileOffset(f): current absolute byte offset.
extern "C" int32_t cb_rt_file_offset(CbFile* f) {
    if (!check_handle("FileOffset", f)) return 0;
    return static_cast<int32_t>(std::ftell(f->fp));
}

// EOF(f): non-zero when there is nothing more to read. Probe one byte and push
// it back (fseek-free, so it works at the very start of an empty file — a
// classic-CB EOF bug we avoid). Counts as a read for the Edit interleave.
extern "C" int32_t cb_rt_eof(CbFile* f) {
    if (!check_handle("EOF", f)) return 1;
    prepare(f, 1);
    int c = std::fgetc(f->fp);
    if (c == EOF) return 1;
    std::ungetc(c, f->fp);
    return 0;
}

// ─── Binary read ───────────────────────────────────────────────────────────

// ReadByte: 8-bit unsigned (0..255); 0 at end-of-data.
extern "C" int32_t cb_rt_read_byte(CbFile* f) {
    if (!check_read("ReadByte", f)) return 0;
    prepare(f, 1);
    int c = std::fgetc(f->fp);
    return c == EOF ? 0 : (c & 0xFF);
}

// ReadShort: 16-bit unsigned (0..65535), little-endian.
extern "C" int32_t cb_rt_read_short(CbFile* f) {
    if (!check_read("ReadShort", f)) return 0;
    prepare(f, 1);
    uint8_t b[2];
    read_le(f, b, 2);
    return static_cast<int32_t>(load_u16_le(b));
}

// ReadInt: 32-bit signed, little-endian.
extern "C" int32_t cb_rt_read_int(CbFile* f) {
    if (!check_read("ReadInt", f)) return 0;
    prepare(f, 1);
    uint8_t b[4];
    read_le(f, b, 4);
    return static_cast<int32_t>(load_u32_le(b));
}

// ReadFloat: 32-bit IEEE float (little-endian) widened to the CB f64 Float.
extern "C" double cb_rt_read_float(CbFile* f) {
    if (!check_read("ReadFloat", f)) return 0.0;
    prepare(f, 1);
    uint8_t b[4];
    read_le(f, b, 4);
    uint32_t bits = load_u32_le(b);
    float v;
    std::memcpy(&v, &bits, sizeof v);
    return static_cast<double>(v);
}

// ReadString: 32-bit LE length prefix + that many raw bytes. Reads exactly the
// prefixed length, preserving embedded NULs; a negative prefix yields "" and a
// short tail stops at the bytes actually present. The reserve cap keeps a corrupt
// huge prefix from forcing a huge allocation (the loop still stops at EOF).
extern "C" CbString* cb_rt_read_string(CbFile* f) {
    std::string s;
    if (check_read("ReadString", f)) {
        prepare(f, 1);
        uint8_t hdr[4];
        read_le(f, hdr, 4);
        int32_t len = static_cast<int32_t>(load_u32_le(hdr));
        if (len > 0) {
            s.reserve(static_cast<std::size_t>(len < 65536 ? len : 65536));
            for (int32_t i = 0; i < len; ++i) {
                int c = std::fgetc(f->fp);
                if (c == EOF) break;
                s.push_back(static_cast<char>(c));
            }
        }
    }
    return make_string(s);
}

// ReadLine: read to end-of-line, stripping the terminator. Handles LF, CR, and
// CRLF (consuming the LF after a CR); classic CB breaks only on CR or EOF.
extern "C" CbString* cb_rt_read_line(CbFile* f) {
    std::string s;
    if (check_read("ReadLine", f)) {
        prepare(f, 1);
        int c;
        while ((c = std::fgetc(f->fp)) != EOF) {
            if (c == '\n') break;
            if (c == '\r') {
                int n = std::fgetc(f->fp);
                if (n != '\n' && n != EOF) std::ungetc(n, f->fp);
                break;
            }
            s.push_back(static_cast<char>(c));
        }
    }
    return make_string(s);
}

// ─── Binary write ──────────────────────────────────────────────────────────

// WriteByte: low 8 bits of `value`.
extern "C" void cb_rt_write_byte(CbFile* f, int32_t value) {
    if (!check_write("WriteByte", f)) return;
    prepare(f, 2);
    uint8_t b = static_cast<uint8_t>(value & 0xFF);
    std::fwrite(&b, 1, 1, f->fp);
}

// WriteShort: low 16 bits of `value`, little-endian.
extern "C" void cb_rt_write_short(CbFile* f, int32_t value) {
    if (!check_write("WriteShort", f)) return;
    prepare(f, 2);
    uint8_t b[2];
    store_u16_le(b, static_cast<uint16_t>(value & 0xFFFF));
    std::fwrite(b, 1, 2, f->fp);
}

// WriteInt: full 32 bits, little-endian.
extern "C" void cb_rt_write_int(CbFile* f, int32_t value) {
    if (!check_write("WriteInt", f)) return;
    prepare(f, 2);
    uint8_t b[4];
    store_u32_le(b, static_cast<uint32_t>(value));
    std::fwrite(b, 1, 4, f->fp);
}

// WriteFloat: CB f64 narrowed to 32-bit IEEE, little-endian.
extern "C" void cb_rt_write_float(CbFile* f, double value) {
    if (!check_write("WriteFloat", f)) return;
    prepare(f, 2);
    float v = static_cast<float>(value);
    uint32_t bits;
    std::memcpy(&bits, &v, sizeof bits);
    uint8_t b[4];
    store_u32_le(b, bits);
    std::fwrite(b, 1, 4, f->fp);
}

// WriteString: 32-bit LE length prefix + the string's raw UTF-8 bytes (no NUL
// terminator). Round-trips with ReadString.
extern "C" void cb_rt_write_string(CbFile* f, const CbString* s) {
    if (!check_write("WriteString", f)) return;
    prepare(f, 2);
    std::size_t len = s ? cb_rt_string_len(s) : 0;
    uint8_t hdr[4];
    store_u32_le(hdr, static_cast<uint32_t>(len));
    std::fwrite(hdr, 1, 4, f->fp);
    if (len > 0) std::fwrite(cb_rt_string_data(s), 1, len, f->fp);
}

// WriteLine: the string's raw bytes followed by the OS line ending (CRLF on
// Windows, LF elsewhere).
extern "C" void cb_rt_write_line(CbFile* f, const CbString* s) {
    if (!check_write("WriteLine", f)) return;
    prepare(f, 2);
    std::size_t len = s ? cb_rt_string_len(s) : 0;
    if (len > 0) std::fwrite(cb_rt_string_data(s), 1, len, f->fp);
#ifdef _WIN32
    std::fwrite("\r\n", 1, 2, f->fp);
#else
    std::fwrite("\n", 1, 1, f->fp);
#endif
}

// ─── Filesystem & directory ──────────────────────────────────────────────

// FileExists(path): 1 if a file or directory exists at `path`, else 0.
extern "C" int32_t cb_rt_file_exists(const CbString* path) {
    std::error_code ec;
    return std::filesystem::exists(to_path(path), ec) ? 1 : 0;
}

// IsDirectory(path): 1 if `path` is a directory, else 0.
extern "C" int32_t cb_rt_is_directory(const CbString* path) {
    std::error_code ec;
    return std::filesystem::is_directory(to_path(path), ec) ? 1 : 0;
}

// FileSize(path): size in bytes; 0 for a missing path or a directory.
extern "C" int32_t cb_rt_file_size(const CbString* path) {
    std::error_code ec;
    std::filesystem::path p = to_path(path);
    if (!std::filesystem::is_regular_file(p, ec)) return 0;
    auto sz = std::filesystem::file_size(p, ec);
    if (ec) return 0;
    return static_cast<int32_t>(sz);
}

// CurrentDir(): the working directory, with a trailing separator (matches
// cbEnchanted / actual CoolBasic).
extern "C" CbString* cb_rt_current_dir(void) {
    std::error_code ec;
    std::filesystem::path p = std::filesystem::current_path(ec);
    std::u8string u = p.u8string();
    std::string s(reinterpret_cast<const char*>(u.data()), u.size());
    s.push_back(static_cast<char>(std::filesystem::path::preferred_separator));
    return make_string(s);
}

// ChDir(path): change the working directory. Best-effort (failure is non-fatal
// in classic CB; we silently ignore it rather than trap).
extern "C" void cb_rt_chdir(const CbString* path) {
    std::error_code ec;
    std::filesystem::current_path(to_path(path), ec);
}

// MakeDir(path): create a directory. Best-effort (non-fatal on failure).
extern "C" void cb_rt_make_dir(const CbString* path) {
    std::error_code ec;
    std::filesystem::create_directory(to_path(path), ec);
}

// CopyFile(src, dst): copy a file. Refuses to overwrite an existing destination
// (traps), matching classic CB; the copy itself is best-effort.
extern "C" void cb_rt_copy_file(const CbString* src, const CbString* dst) {
    std::error_code ec;
    std::filesystem::path d = to_path(dst);
    if (std::filesystem::exists(d, ec)) {
        trap("CopyFile: destination already exists");
        return;
    }
    std::filesystem::copy_file(to_path(src), d, ec);
}

// DeleteFile(path): remove a file or an empty directory. Best-effort (a
// non-empty directory cannot be removed; failure is non-fatal).
extern "C" void cb_rt_delete_file(const CbString* path) {
    std::error_code ec;
    std::filesystem::remove(to_path(path), ec);
}

// Execute(cmd): launch an external command, matching cbEnchanted — a shell
// `start` (Windows) / `xdg-open` (elsewhere) of `cmd` via system().
extern "C" void cb_rt_execute(const CbString* cmd) {
    std::string c = utf8_bytes(cmd);
#ifdef _WIN32
    std::string full = "start " + c;
#else
    std::string full = "xdg-open " + c;
#endif
    std::system(full.c_str());
}

// StartSearch: begin iterating the current directory (single global cursor).
extern "C" void cb_rt_start_search(void) {
    std::error_code ec;
    std::filesystem::path cwd = std::filesystem::current_path(ec);
    g_search = std::filesystem::directory_iterator(cwd, ec);
}

// FindFile: next entry name (files and directories; std::filesystem omits "."
// and ".."), or "" when the search is exhausted.
extern "C" CbString* cb_rt_find_file(void) {
    static const std::filesystem::directory_iterator end;
    std::string name;
    if (g_search != end) {
        std::u8string u = g_search->path().filename().u8string();
        name.assign(reinterpret_cast<const char*>(u.data()), u.size());
        std::error_code ec;
        g_search.increment(ec);
    }
    return make_string(name);
}

// EndSearch: tear down the global search cursor.
extern "C" void cb_rt_end_search(void) {
    g_search = std::filesystem::directory_iterator();
}
