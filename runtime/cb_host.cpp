// CoolBasic runtime — host trap-channel handshake.
//
// CORE TU: stores the host API the backend delivers at startup and exposes it
// to the functionality `cb_rt_*` functions via cb_host(). Allegro-free and
// functionality-agnostic, so it belongs in cb_runtime_core — the
// about_to_exit hook it returns dispatches only to callbacks registered via the
// teardown seam below, so core still references nothing outside core.
//
// Each module that links cb_runtime_core (the main runtime, and each plugin
// DLL) gets its own g_host; the driver calls cb_runtime_init once per module.

#include "cb_runtime_core.h"

// The host API, delivered once via cb_runtime_init. Null until then; cb_rt_*
// callers null-check before use.
static const CbHostApi* g_host = nullptr;

// Teardown-registration seam. Functionality modules (e.g. graphics)
// register an at-exit teardown here during their lazy init; about_to_exit
// dispatches to all of them via run_teardowns(). Keeping the array in core
// means about_to_exit never references an Allegro symbol, so the
// cb_runtime_core / functionality split holds and the SDK-free build
// — which registers nothing — gets a clean no-op. Single-threaded by the same
// contract as the trap channel; no synchronization needed.
static constexpr int CB_MAX_TEARDOWNS = 8;
static void (*g_teardowns[CB_MAX_TEARDOWNS])(void) = {};
static int g_teardown_count = 0;
static int g_teardown_runs = 0;

extern "C" void cb_runtime_register_teardown(void (*fn)(void)) {
    if (fn == nullptr) {
        return;
    }
    // De-dupe by pointer so multiple init sites registering the same coarse
    // teardown is harmless; silently drop once the fixed array is full.
    for (int i = 0; i < g_teardown_count; ++i) {
        if (g_teardowns[i] == fn) {
            return;
        }
    }
    if (g_teardown_count < CB_MAX_TEARDOWNS) {
        g_teardowns[g_teardown_count++] = fn;
    }
}

// The about_to_exit hook: fire every registered teardown. The host
// interpreter calls this at most once per run, but each registered callback is
// independently idempotent so the inline window-close path (which exits before
// the hook) can coexist with it.
static void run_teardowns(void) {
    ++g_teardown_runs;
    for (int i = 0; i < g_teardown_count; ++i) {
        g_teardowns[i]();
    }
}

extern "C" int32_t cb_rt_test_teardown_count(void) {
    return g_teardown_runs;
}

// The hook table handed back to the host. about_to_exit dispatches to the
// teardown seam above; in the SDK-free build nothing registers, so it is an
// empty no-op.
static const CbRuntimeHooks g_hooks = {
    /* size          */ sizeof(CbRuntimeHooks),
    /* about_to_exit */ run_teardowns,
};

extern "C" const CbRuntimeHooks* cb_runtime_init(const CbHostApi* host) {
    // Validate the host's ABI guards before trusting its callbacks.
    // A host that is null, too small, or built against a different host ABI is
    // rejected: g_host stays null and the caller gets null back so it can fail
    // loudly instead of dispatching through a stale/garbage table.
    if (host == nullptr
        || host->size < sizeof(CbHostApi)
        || host->abi_version != CB_HOST_ABI_VERSION) {
        return nullptr;
    }
    g_host = host;
    return &g_hooks;
}

extern "C" const CbHostApi* cb_host(void) {
    return g_host;
}
