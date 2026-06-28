// FD-049: unit tests for the standalone (AOT) program lifecycle
// (cb_standalone.cpp) — the entry-point glue the native backend links against.
// These exercise cb_rt_exit and cb_rt_standalone_run, which both terminate the
// process, so they are GoogleTest *death tests*: each statement runs in an
// isolated child whose exit code and stderr are matched. No display / Allegro.

#include "cb_runtime_core.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <cstdio>

namespace {

void noop_main() {}

void stub_main() {
    std::fputs("user_main ran\n", stderr);
    std::fflush(stderr);
}

// A teardown announcing itself (registered via cb_runtime_register_teardown and
// dispatched by the about_to_exit hook cb_rt_exit fires).
void announce_teardown() {
    std::fputs("teardown fired\n", stderr);
    std::fflush(stderr);
}

// A teardown that re-enters cb_rt_exit: the exit latch must absorb the reentry
// (fire about_to_exit at most once) and exit cleanly rather than recurse.
void reentrant_teardown() {
    std::fputs("td\n", stderr);
    std::fflush(stderr);
    cb_rt_exit(0);
}

} // namespace

// cb_rt_exit terminates with the supplied code.
TEST(StandaloneDeathTest, ExitUsesCode) {
    EXPECT_EXIT(cb_rt_exit(7), ::testing::ExitedWithCode(7), "");
    EXPECT_EXIT(cb_rt_exit(0), ::testing::ExitedWithCode(0), "");
}

// cb_rt_standalone_run runs user_main, then exits 0.
TEST(StandaloneDeathTest, RunInvokesUserMainThenExitsZero) {
    EXPECT_EXIT(
        cb_rt_standalone_run(stub_main), ::testing::ExitedWithCode(0), "user_main ran");
}

// The about_to_exit teardown fires on the standalone exit path.
TEST(StandaloneDeathTest, AboutToExitTeardownFires) {
    EXPECT_EXIT(
        {
            cb_runtime_register_teardown(announce_teardown);
            cb_rt_standalone_run(noop_main);
        },
        ::testing::ExitedWithCode(0),
        "teardown fired");
}

// The exit latch makes about_to_exit fire at most once: a teardown that
// re-enters cb_rt_exit still exits cleanly (no recursion / double dispatch).
TEST(StandaloneDeathTest, ExitLatchAbsorbsReentry) {
    EXPECT_EXIT(
        {
            cb_runtime_register_teardown(reentrant_teardown);
            cb_rt_standalone_run(noop_main);
        },
        ::testing::ExitedWithCode(0),
        "td");
}
