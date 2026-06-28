// CoolBasic system / time runtime.
//
// Timer / Wait / MakeError. Two design choices worth calling out:
//   - Timer uses a monotonic WALL clock (std::chrono::steady_clock) rather than
//     CPU time (clock()): milliseconds-since-start is what a game loop expects,
//     and CPU time drifts from wall time under load.
//   - Wait uses std::this_thread::sleep_for rather than Allegro's al_rest, so a
//     pure sleep does not drag in Allegro initialization.
//
// MakeError only writes its message here; program termination is handled by
// the interpreter/IR (an IR `Halt` terminator follows the call), so nothing
// in the runtime calls exit() — the interpreter stops cleanly and returns the
// process exit code.

#include "cb_runtime.h"

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <ctime>
#include <string>
#include <thread>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#else
#include <unistd.h>
#endif

extern "C" int32_t cb_rt_timer(void) {
    // Lazy epoch: the clock starts on the first Timer() call.
    static const auto start = std::chrono::steady_clock::now();
    auto elapsed = std::chrono::steady_clock::now() - start;
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(elapsed).count();
    return static_cast<int32_t>(ms);
}

extern "C" void cb_rt_wait(int32_t ms) {
    if (ms > 0) {
        std::this_thread::sleep_for(std::chrono::milliseconds(ms));
    }
}

extern "C" void cb_rt_make_error(const CbString* msg) {
    if (msg) {
        std::size_t len = cb_rt_string_len(msg);
        if (len > 0) {
            std::fwrite(cb_rt_string_data(msg), 1, len, stderr);
        }
    }
    std::fputc('\n', stderr);
}

// ─── Date / Time ──────────────────────────────────────────────────────

namespace {
CbString* str_from(const std::string& s) {
    return cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(s.data()),
                                     s.size());
}

// localtime into a caller-owned tm (thread-safe variant per platform).
bool local_now(std::tm& out) {
    std::time_t t = std::time(nullptr);
#ifdef _WIN32
    return localtime_s(&out, &t) == 0;
#else
    return localtime_r(&t, &out) != nullptr;
#endif
}
} // namespace

// Current date as "D Mon YYYY" (e.g. "31 May 2026"); the day is unpadded, to
// match CoolBasic's `Date$` formatting.
extern "C" CbString* cb_rt_date(void) {
    std::tm tm{};
    if (!local_now(tm)) return str_from("");
    char mon[8] = {0};
    char yr[8] = {0};
    std::strftime(mon, sizeof mon, "%b", &tm);
    std::strftime(yr, sizeof yr, "%Y", &tm);
    return str_from(std::to_string(tm.tm_mday) + " " + mon + " " + yr);
}

// Current time as "HH:MM:SS".
extern "C" CbString* cb_rt_time(void) {
    std::tm tm{};
    if (!local_now(tm)) return str_from("");
    char buf[16] = {0};
    std::strftime(buf, sizeof buf, "%H:%M:%S", &tm);
    return str_from(buf);
}

// The process command line. For the interpreter this is `cb`'s own command
// line (interpreter + script + any trailing args): the script runs inside the
// interpreter, not as a separately compiled program executable.
extern "C" CbString* cb_rt_command_line(void) {
#ifdef _WIN32
    const char* cl = GetCommandLineA();
    return str_from(cl ? cl : "");
#else
    // /proc/self/cmdline is NUL-separated; join with spaces.
    std::FILE* f = std::fopen("/proc/self/cmdline", "rb");
    if (!f) return str_from("");
    std::string out;
    int c;
    while ((c = std::fgetc(f)) != EOF) {
        out.push_back(c == '\0' ? ' ' : static_cast<char>(c));
    }
    std::fclose(f);
    while (!out.empty() && out.back() == ' ') out.pop_back();
    return str_from(out);
#endif
}

// Absolute path of the running executable (the `cb` interpreter).
extern "C" CbString* cb_rt_get_exe_name(void) {
#ifdef _WIN32
    char path[MAX_PATH] = {0};
    DWORD n = GetModuleFileNameA(nullptr, path, MAX_PATH);
    return cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(path), n);
#else
    char path[4096] = {0};
    ssize_t n = readlink("/proc/self/exe", path, sizeof path);
    if (n < 0) n = 0;
    return cb_rt_string_from_literal(reinterpret_cast<const uint8_t*>(path),
                                     static_cast<std::size_t>(n));
#endif
}
