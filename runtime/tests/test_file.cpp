// Unit tests for the file-I/O runtime (cb_file.cpp). These drive the
// extern "C" cb_rt_* entry points directly (like test_memblock.cpp), so the
// target links cb_runtime. No display / Allegro is touched; the tests work
// against real files in a per-test temp directory.
//
// No trap host is connected in this target (cb_runtime_init is never called),
// so cb_host() returns null: an invalid handle / wrong-mode op is a silent
// no-op that returns the safe default (0 / "") rather than aborting. That
// "returns the default, never corrupts/crashes" property is what these pin; the
// end-to-end trap-to-exit-1 behaviour (host connected) is covered by cli.rs.

#include "cb_runtime.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <filesystem>
#include <fstream>
#include <set>
#include <string>

namespace {

// UTF-8 bytes of a path, the form cb_rt_* expect (via a CbString).
std::string path_str(const std::filesystem::path& p) {
    std::u8string u = p.u8string();
    return std::string(reinterpret_cast<const char*>(u.data()), u.size());
}

// RAII CbString from raw bytes (so embedded NULs survive).
struct Str {
    CbString* s;
    explicit Str(const std::string& v)
        : s(cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(v.data()), v.size())) {}
    ~Str() { cb_rt_string_release(s); }
    Str(const Str&) = delete;
    Str& operator=(const Str&) = delete;
    operator const CbString*() const { return s; }
};

// Consume an owned CbString return value into a std::string (releases it).
std::string take(CbString* s) {
    std::string r(reinterpret_cast<const char*>(cb_rt_string_data(s)), cb_rt_string_len(s));
    cb_rt_string_release(s);
    return r;
}

// Per-test scratch directory; cwd is saved/restored since some tests ChDir.
class FileTest : public ::testing::Test {
protected:
    std::filesystem::path dir;
    std::filesystem::path orig_cwd;

    void SetUp() override {
        std::error_code ec;
        orig_cwd = std::filesystem::current_path(ec);
        const auto* info = ::testing::UnitTest::GetInstance()->current_test_info();
        dir = std::filesystem::temp_directory_path(ec) / (std::string("cb_file_test_") + info->name());
        std::filesystem::remove_all(dir, ec);
        std::filesystem::create_directories(dir, ec);
    }
    void TearDown() override {
        std::error_code ec;
        std::filesystem::current_path(orig_cwd, ec); // leave the dir before removing it
        std::filesystem::remove_all(dir, ec);
    }

    // A CbString path inside the scratch dir. Returned by value; wrap in Str.
    std::string at(const char* name) { return path_str(dir / name); }
};

} // namespace

TEST_F(FileTest, RoundTripPrimitives) {
    Str path(at("data.dat"));
    CbFile* w = cb_rt_open_to_write(path);
    ASSERT_NE(w, nullptr);
    cb_rt_write_byte(w, 65);
    cb_rt_write_byte(w, 300);   // only the low 8 bits (44) are stored
    cb_rt_write_short(w, 65535);
    cb_rt_write_int(w, -1);
    cb_rt_write_float(w, 1.5);
    cb_rt_close_file(w);

    CbFile* r = cb_rt_open_to_read(path);
    ASSERT_NE(r, nullptr);
    EXPECT_EQ(cb_rt_read_byte(r), 65);
    EXPECT_EQ(cb_rt_read_byte(r), 44);
    EXPECT_EQ(cb_rt_read_short(r), 65535);  // unsigned, not -1
    EXPECT_EQ(cb_rt_read_int(r), -1);       // signed
    EXPECT_DOUBLE_EQ(cb_rt_read_float(r), 1.5);
    cb_rt_close_file(r);
}

// PokeInt-style: bytes are laid little-endian regardless of host order.
TEST_F(FileTest, LittleEndianOnTheWire) {
    Str path(at("le.dat"));
    CbFile* w = cb_rt_open_to_write(path);
    ASSERT_NE(w, nullptr);
    cb_rt_write_int(w, 0x04030201);
    cb_rt_close_file(w);

    CbFile* r = cb_rt_open_to_read(path);
    ASSERT_NE(r, nullptr);
    EXPECT_EQ(cb_rt_read_byte(r), 0x01);
    EXPECT_EQ(cb_rt_read_byte(r), 0x02);
    EXPECT_EQ(cb_rt_read_byte(r), 0x03);
    EXPECT_EQ(cb_rt_read_byte(r), 0x04);
    cb_rt_close_file(r);
}

TEST_F(FileTest, FloatIs32BitOnDisk) {
    Str path(at("f.dat"));
    CbFile* w = cb_rt_open_to_write(path);
    cb_rt_write_float(w, 0.1);  // not representable in f32
    cb_rt_close_file(w);
    // The file holds exactly 4 bytes.
    EXPECT_EQ(cb_rt_file_size(path), 4);

    CbFile* r = cb_rt_open_to_read(path);
    EXPECT_FLOAT_EQ(static_cast<float>(cb_rt_read_float(r)), 0.1f);
    cb_rt_close_file(r);
}

TEST_F(FileTest, StringRoundTripWithEmbeddedNul) {
    std::string content;
    content.push_back('a');
    content.push_back('\0');
    content.push_back('b');  // length 3, embedded NUL
    Str path(at("s.dat"));
    Str payload(content);

    CbFile* w = cb_rt_open_to_write(path);
    cb_rt_write_string(w, payload);
    cb_rt_close_file(w);
    // 4-byte length prefix + 3 bytes.
    EXPECT_EQ(cb_rt_file_size(path), 7);

    CbFile* r = cb_rt_open_to_read(path);
    CbString* got = cb_rt_read_string(r);
    ASSERT_EQ(cb_rt_string_len(got), 3u);
    const uint8_t* d = cb_rt_string_data(got);
    EXPECT_EQ(d[0], 'a');
    EXPECT_EQ(d[1], 0);
    EXPECT_EQ(d[2], 'b');
    cb_rt_string_release(got);
    cb_rt_close_file(r);
}

// ReadLine stops on LF, CR, and CRLF (consuming the LF after a CR), unlike
// classic CB which only breaks on CR or EOF.
TEST_F(FileTest, ReadLineHandlesAllEndings) {
    std::filesystem::path file = dir / "lines.dat";
    {
        std::ofstream os(file, std::ios::binary);
        os << "a\nb\r\nc\rd";  // LF, CRLF, CR, then "d" with no terminator
    }
    Str path(path_str(file));
    CbFile* r = cb_rt_open_to_read(path);
    ASSERT_NE(r, nullptr);
    EXPECT_EQ(take(cb_rt_read_line(r)), "a");
    EXPECT_EQ(take(cb_rt_read_line(r)), "b");
    EXPECT_EQ(take(cb_rt_read_line(r)), "c");
    EXPECT_EQ(take(cb_rt_read_line(r)), "d");
    EXPECT_NE(cb_rt_eof(r), 0);
    cb_rt_close_file(r);
}

TEST_F(FileTest, WriteLineThenReadLine) {
    Str path(at("wl.dat"));
    Str hello("hello");
    CbFile* w = cb_rt_open_to_write(path);
    cb_rt_write_line(w, hello);
    cb_rt_close_file(w);

    CbFile* r = cb_rt_open_to_read(path);
    EXPECT_EQ(take(cb_rt_read_line(r)), "hello");
    EXPECT_NE(cb_rt_eof(r), 0);  // terminator fully consumed
    cb_rt_close_file(r);
}

TEST_F(FileTest, SeekAndOffsetOnEdit) {
    Str path(at("seek.dat"));
    CbFile* w = cb_rt_open_to_write(path);
    cb_rt_write_int(w, 10);
    cb_rt_write_int(w, 20);
    cb_rt_write_int(w, 30);
    cb_rt_close_file(w);

    CbFile* e = cb_rt_open_to_edit(path);
    ASSERT_NE(e, nullptr);
    cb_rt_seek_file(e, 4);  // second int
    EXPECT_EQ(cb_rt_file_offset(e), 4);
    EXPECT_EQ(cb_rt_read_int(e), 20);
    EXPECT_EQ(cb_rt_file_offset(e), 8);
    cb_rt_close_file(e);
}

// OpenToEdit allows interleaved write-then-read (the prepare() seek handles the
// C r+ direction switch).
TEST_F(FileTest, EditInterleaveWriteThenRead) {
    Str path(at("edit.dat"));
    CbFile* w = cb_rt_open_to_write(path);
    cb_rt_write_int(w, 111);
    cb_rt_write_int(w, 222);
    cb_rt_close_file(w);

    CbFile* e = cb_rt_open_to_edit(path);
    cb_rt_seek_file(e, 0);
    cb_rt_write_int(e, 999);  // overwrite first int
    // Read the second int back without an explicit seek between write and read.
    EXPECT_EQ(cb_rt_read_int(e), 222);
    cb_rt_seek_file(e, 0);
    EXPECT_EQ(cb_rt_read_int(e), 999);
    cb_rt_close_file(e);
}

TEST_F(FileTest, EofOnEmptyFile) {
    Str path(at("empty.dat"));
    CbFile* w = cb_rt_open_to_write(path);
    cb_rt_close_file(w);  // creates an empty file

    CbFile* r = cb_rt_open_to_read(path);
    ASSERT_NE(r, nullptr);
    EXPECT_NE(cb_rt_eof(r), 0);       // EOF true immediately, no crash
    EXPECT_EQ(cb_rt_read_byte(r), 0); // lenient
    cb_rt_close_file(r);
}

// Reads past end return zero values and zero-fill partial multi-byte reads.
TEST_F(FileTest, LenientPastEnd) {
    Str path(at("one.dat"));
    CbFile* w = cb_rt_open_to_write(path);
    cb_rt_write_byte(w, 0x7F);
    cb_rt_close_file(w);

    CbFile* r = cb_rt_open_to_read(path);
    EXPECT_EQ(cb_rt_read_byte(r), 0x7F);
    EXPECT_EQ(cb_rt_read_int(r), 0);       // only 0 bytes left → zero-filled
    EXPECT_EQ(cb_rt_read_byte(r), 0);
    EXPECT_EQ(take(cb_rt_read_string(r)), "");
    EXPECT_EQ(take(cb_rt_read_line(r)), "");
    cb_rt_close_file(r);
}

TEST_F(FileTest, OpenMissingReturnsNull) {
    Str path(at("does_not_exist.dat"));
    EXPECT_EQ(cb_rt_open_to_read(path), nullptr);
}

TEST_F(FileTest, FilesystemQueries) {
    Str file(at("q.dat"));
    CbFile* w = cb_rt_open_to_write(file);
    cb_rt_write_int(w, 0);
    cb_rt_write_int(w, 0);  // 8 bytes
    cb_rt_close_file(w);

    EXPECT_NE(cb_rt_file_exists(file), 0);
    EXPECT_EQ(cb_rt_is_directory(file), 0);
    EXPECT_EQ(cb_rt_file_size(file), 8);

    Str sub(at("sub"));
    cb_rt_make_dir(sub);
    EXPECT_NE(cb_rt_file_exists(sub), 0);
    EXPECT_NE(cb_rt_is_directory(sub), 0);
    EXPECT_EQ(cb_rt_file_size(sub), 0);  // directories report 0

    cb_rt_delete_file(file);
    EXPECT_EQ(cb_rt_file_exists(file), 0);
}

TEST_F(FileTest, CopyFileRefusesOverwrite) {
    Str src(at("src.dat"));
    CbFile* w = cb_rt_open_to_write(src);
    cb_rt_write_int(w, 0);  // 4 bytes
    cb_rt_close_file(w);

    Str dst(at("dst.dat"));
    cb_rt_copy_file(src, dst);
    EXPECT_NE(cb_rt_file_exists(dst), 0);
    EXPECT_EQ(cb_rt_file_size(dst), 4);

    // Make dst a different size, then a second copy must refuse to overwrite it
    // (no host → the trap is a no-op, so the copy simply does not happen).
    CbFile* w2 = cb_rt_open_to_write(dst);
    cb_rt_write_int(w2, 0);
    cb_rt_write_int(w2, 0);  // now 8 bytes
    cb_rt_close_file(w2);
    cb_rt_copy_file(src, dst);
    EXPECT_EQ(cb_rt_file_size(dst), 8);  // unchanged — not overwritten
}

// StartSearch/FindFile/EndSearch over the (ch-dir'd) scratch directory. Returns
// real entries only — no "." / ".." — and "" when exhausted.
TEST_F(FileTest, DirectorySearch) {
    {
        std::ofstream(dir / "a.dat", std::ios::binary) << "x";
        std::ofstream(dir / "b.dat", std::ios::binary) << "y";
    }
    std::error_code ec;
    std::filesystem::create_directory(dir / "d", ec);

    Str here(path_str(dir));
    cb_rt_chdir(here);  // search operates on the current directory

    cb_rt_start_search();
    std::set<std::string> found;
    for (;;) {
        std::string name = take(cb_rt_find_file());
        if (name.empty()) break;
        found.insert(name);
    }
    cb_rt_end_search();

    EXPECT_EQ(found, (std::set<std::string>{"a.dat", "b.dat", "d"}));
    EXPECT_EQ(found.count("."), 0u);
    EXPECT_EQ(found.count(".."), 0u);
}

TEST_F(FileTest, CurrentDirHasTrailingSeparator) {
    Str here(path_str(dir));
    cb_rt_chdir(here);
    std::string cd = take(cb_rt_current_dir());
    ASSERT_FALSE(cd.empty());
    EXPECT_EQ(static_cast<char>(cd.back()),
              static_cast<char>(std::filesystem::path::preferred_separator));
}

TEST_F(FileTest, NullHandleIsSafe) {
    EXPECT_EQ(cb_rt_read_int(nullptr), 0);
    EXPECT_EQ(cb_rt_read_byte(nullptr), 0);
    EXPECT_NE(cb_rt_eof(nullptr), 0);          // nothing to read
    EXPECT_EQ(cb_rt_file_offset(nullptr), 0);
    EXPECT_EQ(take(cb_rt_read_string(nullptr)), "");
    cb_rt_write_int(nullptr, 1);   // must not crash
    cb_rt_seek_file(nullptr, 0);   // must not crash
    cb_rt_close_file(nullptr);     // must not crash
}

// Wrong-mode ops trap (no-op with no host) and return the safe default.
TEST_F(FileTest, WrongModeReturnsDefault) {
    Str path(at("wm.dat"));
    CbFile* w = cb_rt_open_to_write(path);
    ASSERT_NE(w, nullptr);
    EXPECT_EQ(cb_rt_read_byte(w), 0);  // reading a write-only handle: default
    cb_rt_close_file(w);

    CbFile* r = cb_rt_open_to_read(path);
    ASSERT_NE(r, nullptr);
    cb_rt_write_byte(r, 0x42);  // writing a read-only handle: no-op, no crash
    cb_rt_close_file(r);
}
