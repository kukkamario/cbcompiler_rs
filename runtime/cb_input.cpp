// CoolBasic input runtime (FD-013 Batch 5).
//
// The CB DirectInput-style scancode table (`sCBKeyMap`) and the 2-bit edge-state
// machine (bit0 = currently down, bit1 = changed since the last frame). Mouse
// buttons/wheel/movement reuse the same edge model, backed by Allegro 5's mouse
// events (CoolBasic exposed no mouse-button/wheel functions; these are new).
//
// Frame boundary = DrawScreen. cb_gfx.cpp owns the event queue; on each
// DrawScreen it calls cb::input::frame_begin() (clears the per-frame "changed"
// bits and zeroes the movement deltas) and then routes every queued event to
// cb::input::handle_event(). Input state therefore only advances when the
// program pumps DrawScreen — as a windowed game loop expects. With no display
// open, no events arrive and every query returns 0 (headless-safe).
//
// ABI: CB `Int` arrives/returns as int32_t. EscapeKey is a pure query — unlike
// CoolBasic's "safe exit", pressing Escape does NOT auto-close the program (that
// would need a runtime->interpreter trap channel that does not exist yet, and
// conflicts with Batch 3's clean IR Halt termination).

#include "cb_runtime.h"
#include "cb_input.h"
#include "cb_gfx.h"  // cb::gfx::display / event_queue

#include <allegro5/allegro.h>

#include <cstring>
#include <deque>

// ─── Edge-state model ──────────────────────────────────────────────────
//
// Per key/button: bit0 = down now, bit1 = changed since the last frame begin.
// Derived states: Down = 0b01, Released = 0b10 (was down,
// now up this frame), Pressed = 0b11 (was up, now down this frame).
namespace cb::input {

namespace {

constexpr unsigned char DOWN_BIT    = 1;
constexpr unsigned char CHANGED_BIT = 2;
constexpr unsigned char RELEASED    = 2; // CHANGED_BIT, DOWN_BIT clear
constexpr unsigned char PRESSED     = 3; // CHANGED_BIT | DOWN_BIT

// Keyboard state indexed by Allegro keycode.
unsigned char sKeyStates[ALLEGRO_KEY_MAX] = {0};

// Mouse-button state indexed by Allegro button number (1=left, 2=right,
// 3=middle). Index 0 is unused.
constexpr int MOUSE_BUTTON_COUNT = 8;
unsigned char sMouseButtons[MOUSE_BUTTON_COUNT] = {0};

// Wheel position (absolute) and per-frame movement deltas.
int32_t sMouseZ  = 0;
int32_t sMouseDX = 0;
int32_t sMouseDY = 0;
int32_t sMouseDZ = 0;

// FIFO queues for GetKey/GetMouse: typed character codepoints (KEY_CHAR) and
// button-down button numbers (MOUSE_BUTTON_DOWN), accumulated as DrawScreen
// drains events and consumed one per call.
std::deque<int32_t> sCharQueue;
std::deque<int32_t> sMouseDownQueue;

// ClearKeys/ClearMouse set these to swallow input events for the rest of the
// frame; frame_begin clears them (CoolBasic's clearKeyboard / clearMouse flags —
// "ignored until the next frame").
bool sIgnoreKeyboard = false;
bool sIgnoreMouse    = false;

// CB DirectInput-style scancode -> Allegro keycode. Index is the CB scancode
// (1..221); value is the ALLEGRO_KEY_* constant (0 = unmapped).
constexpr int SCANCODE_MAX = 222;
int sCBKeyMap[SCANCODE_MAX] = {0};
bool sKeyMapInit = false;

void init_key_map() {
    if (sKeyMapInit) return;
    sKeyMapInit = true;
    // Populated from the shared X-macro table (FD-029); the SAME table drives
    // the public `cbKey*` constants in catalog.cpp, so a scancode can never
    // drift between the lookup table and the symbolic name.
#define CB_KEY(name, scan, al) sCBKeyMap[scan] = al;
#define CB_KEY_RAW(scan, al)   sCBKeyMap[scan] = al;
#include "cb_keys.def"
#undef CB_KEY
#undef CB_KEY_RAW
}

int scancode_to_allegro_key(int scan) {
    init_key_map();
    if (scan > 0 && scan < SCANCODE_MAX) return sCBKeyMap[scan];
    return 0;
}

// Reverse of scancode_to_allegro_key: an Allegro keycode -> CB scancode, or 0
// if unmapped. Used by WaitKey's function form.
int allegro_key_to_scancode(int keycode) {
    init_key_map();
    if (keycode <= 0) return 0;
    for (int i = 1; i < SCANCODE_MAX; ++i) {
        if (sCBKeyMap[i] == keycode) return i;
    }
    return 0;
}

// Apply a press/release to a 2-bit slot, marking it changed only on a genuine
// transition (so key-repeat events don't re-trigger the "changed" bit).
void apply_transition(unsigned char& slot, bool is_down) {
    if ((slot & DOWN_BIT) != (is_down ? 1 : 0)) {
        slot ^= DOWN_BIT;
        slot |= CHANGED_BIT;
    }
}

} // namespace

// ─── Per-frame hooks (called by cb_gfx.cpp's DrawScreen) ───────────────

void frame_begin(void) {
    // Clear the "changed" bit on every key/button so Pressed/Released last
    // exactly one frame; zero the movement deltas (they accumulate per frame).
    for (int i = 0; i < ALLEGRO_KEY_MAX; i++) sKeyStates[i] &= DOWN_BIT;
    for (int i = 0; i < MOUSE_BUTTON_COUNT; i++) sMouseButtons[i] &= DOWN_BIT;
    sMouseDX = 0;
    sMouseDY = 0;
    sMouseDZ = 0;
    // A ClearKeys/ClearMouse swallow lasts only until the next frame begins.
    sIgnoreKeyboard = false;
    sIgnoreMouse    = false;
}

void handle_event(const ALLEGRO_EVENT* ev) {
    if (!ev) return;
    switch (ev->type) {
        case ALLEGRO_EVENT_KEY_DOWN: {
            if (sIgnoreKeyboard) break;
            int kc = ev->keyboard.keycode;
            if (kc > 0 && kc < ALLEGRO_KEY_MAX) apply_transition(sKeyStates[kc], true);
            break;
        }
        case ALLEGRO_EVENT_KEY_UP: {
            if (sIgnoreKeyboard) break;
            int kc = ev->keyboard.keycode;
            if (kc > 0 && kc < ALLEGRO_KEY_MAX) apply_transition(sKeyStates[kc], false);
            break;
        }
        case ALLEGRO_EVENT_KEY_CHAR: {
            // Queue typed characters for GetKey (codepoint; ASCII == CP-1252).
            // Skip auto-repeat so GetKey yields one code per physical press.
            if (sIgnoreKeyboard || ev->keyboard.repeat) break;
            sCharQueue.push_back(ev->keyboard.unichar);
            break;
        }
        case ALLEGRO_EVENT_MOUSE_BUTTON_DOWN: {
            if (sIgnoreMouse) break;
            unsigned b = ev->mouse.button;
            if (b > 0 && b < (unsigned)MOUSE_BUTTON_COUNT) {
                apply_transition(sMouseButtons[b], true);
                sMouseDownQueue.push_back((int32_t)b);
            }
            break;
        }
        case ALLEGRO_EVENT_MOUSE_BUTTON_UP: {
            if (sIgnoreMouse) break;
            unsigned b = ev->mouse.button;
            if (b > 0 && b < (unsigned)MOUSE_BUTTON_COUNT) apply_transition(sMouseButtons[b], false);
            break;
        }
        case ALLEGRO_EVENT_MOUSE_AXES: {
            sMouseDX += ev->mouse.dx;
            sMouseDY += ev->mouse.dy;
            sMouseDZ += ev->mouse.dz;
            sMouseZ   = ev->mouse.z;
            break;
        }
        default:
            break;
    }
}

// ─── Keyboard queries ──────────────────────────────────────────────────

extern "C" int32_t cb_rt_key_down(int32_t scancode) {
    int kc = scancode_to_allegro_key(scancode);
    if (kc <= 0) return 0;
    return (sKeyStates[kc] & DOWN_BIT) ? 1 : 0;
}

extern "C" int32_t cb_rt_key_up(int32_t scancode) {
    int kc = scancode_to_allegro_key(scancode);
    if (kc <= 0) return 0;
    return (sKeyStates[kc] == RELEASED) ? 1 : 0;
}

extern "C" int32_t cb_rt_key_hit(int32_t scancode) {
    int kc = scancode_to_allegro_key(scancode);
    if (kc <= 0) return 0;
    return (sKeyStates[kc] == PRESSED) ? 1 : 0;
}

extern "C" int32_t cb_rt_escape_key(void) {
    return (sKeyStates[ALLEGRO_KEY_ESCAPE] & DOWN_BIT) ? 1 : 0;
}

// ─── Mouse queries ─────────────────────────────────────────────────────

extern "C" int32_t cb_rt_mouse_x(void) {
    ALLEGRO_MOUSE_STATE state;
    al_get_mouse_state(&state);
    return state.x;
}

extern "C" int32_t cb_rt_mouse_y(void) {
    ALLEGRO_MOUSE_STATE state;
    al_get_mouse_state(&state);
    return state.y;
}

extern "C" int32_t cb_rt_mouse_down(int32_t button) {
    if (button <= 0 || button >= MOUSE_BUTTON_COUNT) return 0;
    return (sMouseButtons[button] & DOWN_BIT) ? 1 : 0;
}

extern "C" int32_t cb_rt_mouse_hit(int32_t button) {
    if (button <= 0 || button >= MOUSE_BUTTON_COUNT) return 0;
    return (sMouseButtons[button] == PRESSED) ? 1 : 0;
}

extern "C" int32_t cb_rt_mouse_up(int32_t button) {
    if (button <= 0 || button >= MOUSE_BUTTON_COUNT) return 0;
    return (sMouseButtons[button] == RELEASED) ? 1 : 0;
}

extern "C" int32_t cb_rt_mouse_z(void)      { return sMouseZ; }
extern "C" int32_t cb_rt_mouse_move_x(void) { return sMouseDX; }
extern "C" int32_t cb_rt_mouse_move_y(void) { return sMouseDY; }
extern "C" int32_t cb_rt_mouse_move_z(void) { return sMouseDZ; }

// ─── FD-017 keyboard additions ─────────────────────────────────────────
//
// GetKey returns the next queued typed character (codepoint; ASCII == CP-1252),
// or 0 if none queued. Like the rest of the input model, the queue only fills
// while the program pumps DrawScreen; headless it stays empty (returns 0).
extern "C" int32_t cb_rt_get_key(void) {
    if (sCharQueue.empty()) return 0;
    int32_t c = sCharQueue.front();
    sCharQueue.pop_front();
    return c;
}

// Arrow-key level queries.
extern "C" int32_t cb_rt_left_key(void) {
    return (sKeyStates[ALLEGRO_KEY_LEFT] & DOWN_BIT) ? 1 : 0;
}
extern "C" int32_t cb_rt_right_key(void) {
    return (sKeyStates[ALLEGRO_KEY_RIGHT] & DOWN_BIT) ? 1 : 0;
}
extern "C" int32_t cb_rt_up_key(void) {
    return (sKeyStates[ALLEGRO_KEY_UP] & DOWN_BIT) ? 1 : 0;
}
extern "C" int32_t cb_rt_down_key(void) {
    return (sKeyStates[ALLEGRO_KEY_DOWN] & DOWN_BIT) ? 1 : 0;
}

// Clears key states and the typed-char queue, and swallows keyboard events for
// the rest of the frame (until the next DrawScreen).
extern "C" void cb_rt_clear_keys(void) {
    std::memset(sKeyStates, 0, sizeof(sKeyStates));
    sCharQueue.clear();
    sIgnoreKeyboard = true;
}

// Blocks until a key is pressed; returns its CB scancode (0 on window close).
// With no window open there is no event queue to wait on, so it returns 0
// immediately rather than hang — the headless-safe degenerate behaviour.
extern "C" int32_t cb_rt_wait_key(void) {
    ALLEGRO_EVENT_QUEUE* q = cb::gfx::event_queue();
    if (!q) return 0;
    ALLEGRO_EVENT e;
    while (true) {
        al_wait_for_event(q, &e);
        handle_event(&e);
        if (e.type == ALLEGRO_EVENT_KEY_DOWN) {
            return allegro_key_to_scancode(e.keyboard.keycode);
        }
        if (e.type == ALLEGRO_EVENT_DISPLAY_CLOSE) {
            const CbHostApi* h = cb_host();
            if (h) h->request_exit(0);
            return 0;
        }
    }
}

// ─── FD-017 mouse additions ────────────────────────────────────────────
//
// GetMouse returns the next queued button-down (button number), or 0 if none.
extern "C" int32_t cb_rt_get_mouse(void) {
    if (sMouseDownQueue.empty()) return 0;
    int32_t b = sMouseDownQueue.front();
    sMouseDownQueue.pop_front();
    return b;
}

// Blocks until a mouse button is pressed; returns the button (0 on window
// close). Headless (no event queue) returns 0 immediately rather than hang.
extern "C" int32_t cb_rt_wait_mouse(void) {
    ALLEGRO_EVENT_QUEUE* q = cb::gfx::event_queue();
    if (!q) return 0;
    ALLEGRO_EVENT e;
    while (true) {
        al_wait_for_event(q, &e);
        handle_event(&e);
        if (e.type == ALLEGRO_EVENT_MOUSE_BUTTON_DOWN) {
            return (int32_t)e.mouse.button;
        }
        if (e.type == ALLEGRO_EVENT_DISPLAY_CLOSE) {
            const CbHostApi* h = cb_host();
            if (h) h->request_exit(0);
            return 0;
        }
    }
}

// Moves the cursor to screen coordinates. No-op without a window.
extern "C" void cb_rt_position_mouse(int32_t x, int32_t y) {
    ALLEGRO_DISPLAY* d = cb::gfx::display();
    if (d) al_set_mouse_xy(d, x, y);
}

// Cursor mode: 0=hide, 1=standard cursor. CoolBasic's `>1`=use image-id form
// has no equivalent here (images are opaque handles, not integer ids), so any
// value > 1 falls back to showing the standard cursor. No-op without a window.
extern "C" void cb_rt_show_mouse(int32_t mode) {
    ALLEGRO_DISPLAY* d = cb::gfx::display();
    if (!d) return;
    if (mode == 0) {
        al_hide_mouse_cursor(d);
    } else {
        al_show_mouse_cursor(d);
    }
}

// Clears mouse button states and the button-down queue, and swallows mouse
// events for the rest of the frame (until the next DrawScreen).
extern "C" void cb_rt_clear_mouse(void) {
    std::memset(sMouseButtons, 0, sizeof(sMouseButtons));
    sMouseDownQueue.clear();
    sIgnoreMouse = true;
}

}  // namespace cb::input
