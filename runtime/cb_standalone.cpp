// CoolBasic runtime — standalone (AOT) program lifecycle (FD-049 decision A).
//
// CORE TU: provides the entry-point glue the native/LLVM backend links against
// when it emits a self-contained executable. The backend emits a trivial
//   int main() { cb_rt_standalone_run(cb_user_main); return 0; }
// plus `cb_user_main` (the lowered top-level body). This TU supplies the default
// host (the FD-015 trap channel) and the clean-exit path.
//
// Allegro-free and functionality-agnostic, so it belongs in cb_runtime_core
// (FD-016): it references only the core handshake (cb_runtime_init) and the
// core string primitives (for raise_error's message). It carries NO `main`, so
// it never collides with the interpreter binary — which statically links the
// runtime and drives cb_runtime_init through its own host (cb-backend-interp).
// There these symbols are simply never called.

#include "cb_runtime_core.h"

#include <cstdint>
#include <cstdio>
#include <cstdlib>

#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#endif

// Hooks returned by cb_runtime_init, stashed by cb_rt_standalone_run so
// cb_rt_exit can fire about_to_exit. Single-threaded by the same contract as
// the trap channel; no synchronization needed.
static const CbRuntimeHooks* g_standalone_hooks = nullptr;
static int g_exit_latched = 0;

extern "C" void cb_rt_exit(int32_t code) {
    // Fire the teardown hook exactly once, even if cb_rt_exit is re-entered
    // (e.g. raise_error → cb_rt_exit while a teardown is running).
    if (!g_exit_latched) {
        g_exit_latched = 1;
        if (g_standalone_hooks && g_standalone_hooks->about_to_exit) {
            g_standalone_hooks->about_to_exit();
        }
    }
    // libc exit() flushes buffered stdio — required so a piped stdout (the test
    // harness) is not truncated. _exit() would lose it.
    std::exit(code);
}

// Default host callbacks (FD-015). Plain file-static functions assigned to the
// CbHostApi function-pointer fields, mirroring cb_host.cpp's run_teardowns.
static void default_request_exit(int32_t code) {
    cb_rt_exit(code);
}

static void default_raise_error(const CbString* msg) {
    // Mirror cb_system.cpp's MakeError: write the message to stderr, then exit
    // with code 1 (a fatal runtime error).
    if (msg) {
        std::size_t len = cb_rt_string_len(msg);
        if (len > 0) {
            std::fwrite(cb_rt_string_data(msg), 1, len, stderr);
        }
    }
    std::fputc('\n', stderr);
    cb_rt_exit(1);
}

extern "C" int32_t cb_rt_standalone_run(void (*user_main)(void)) {
#ifdef _WIN32
    // Put stdout in binary mode so cb_rt_print's '\n' is not translated to
    // CRLF (FD-049 review F12). The interpreter writes raw bytes through Rust's
    // stdout, so this makes the native exe byte-identical — an embedded CR+LF
    // (Chr$(13)+Chr$(10)) no longer becomes "\r\r\n". No-op on Unix.
    _setmode(_fileno(stdout), _O_BINARY);
#endif
    // The host outlives every runtime call (whole program), so make it static.
    static const CbHostApi host = {
        /* size         */ static_cast<uint32_t>(sizeof(CbHostApi)),
        /* abi_version  */ CB_HOST_ABI_VERSION,
        /* request_exit */ default_request_exit,
        /* raise_error  */ default_raise_error,
    };
    const CbRuntimeHooks* hooks = cb_runtime_init(&host);
    if (!hooks) {
        std::fputs("CoolBasic runtime: init handshake failed\n", stderr);
        std::exit(1);
    }
    g_standalone_hooks = hooks;

    if (user_main) {
        user_main();
    }
    cb_rt_exit(0);
    return 0;  // unreachable (cb_rt_exit does not return)
}
