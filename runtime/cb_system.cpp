// CoolBasic system / time runtime (FD-013 Batch 3).
//
// Timer / Wait / MakeError. Two deliberate departures from the legacy
// implementation (../CBCompiler/Runtime/cb_system.cpp):
//   - Timer uses a monotonic WALL clock (std::chrono::steady_clock), not the
//     legacy clock() (which measures CPU time). Milliseconds-since-start is
//     what a game loop expects; CPU time drifts from wall time under load.
//   - Wait uses std::this_thread::sleep_for rather than Allegro's al_rest, so
//     a pure sleep does not drag in Allegro initialization.
//
// MakeError only writes its message here; program termination is handled by
// the interpreter/IR (an IR `Halt` terminator follows the call), so nothing
// in the runtime calls exit() — the interpreter stops cleanly and returns the
// process exit code.

#include "cb_runtime.h"

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <thread>

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
