// CoolBasic runtime — host trap-channel handshake (FD-015).
//
// CORE TU: stores the host API the backend delivers at startup and exposes it
// to the functionality `cb_rt_*` functions via cb_host(). Allegro-free and
// functionality-agnostic, so it belongs in cb_runtime_core (FD-016) — the
// hooks it returns are currently empty, so it references nothing outside core.
//
// Each module that links cb_runtime_core (the main runtime, and each plugin
// DLL) gets its own g_host; the driver calls cb_runtime_init once per module.

#include "cb_runtime_core.h"

// The host API, delivered once via cb_runtime_init. Null until then; cb_rt_*
// callers null-check before use.
static const CbHostApi* g_host = nullptr;

// The hook table handed back to the host. about_to_exit is reserved (null) for
// now — wiring it to a subsystem teardown would pull a functionality symbol
// into core, so window-close tears down its own display inline instead.
static const CbRuntimeHooks g_hooks = {
    /* size          */ sizeof(CbRuntimeHooks),
    /* about_to_exit */ nullptr,
};

extern "C" const CbRuntimeHooks* cb_runtime_init(const CbHostApi* host) {
    // Store unconditionally; the host fills `size`/`abi_version` as ABI guards
    // that a future plugin loader can validate before trusting the table.
    g_host = host;
    return &g_hooks;
}

extern "C" const CbHostApi* cb_host(void) {
    return g_host;
}
