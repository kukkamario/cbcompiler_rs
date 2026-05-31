// CoolBasic input runtime (FD-013 Batch 5).
//
// Keyboard ported 1:1 from the legacy ../CBCompiler/Runtime/cb_input.cpp +
// inputinterface.cpp: the CB DirectInput-style scancode table (`sCBKeyMap`) and
// the 2-bit edge-state machine (bit0 = currently down, bit1 = changed since the
// last frame). Mouse buttons/wheel/movement are new cbcompiler_rs definitions
// (the legacy runtime exposed no mouse-button/wheel functions); they reuse the
// same edge model, backed by Allegro 5's mouse events.
//
// Frame boundary = DrawScreen. cb_gfx.cpp owns the event queue; on each
// DrawScreen it calls cb_input_frame_begin() (clears the per-frame "changed"
// bits and zeroes the movement deltas) and then routes every queued event to
// cb_input_handle_event(). Input state therefore only advances when the program
// pumps DrawScreen — exactly as the legacy window loop behaved. With no display
// open, no events arrive and every query returns 0 (headless-safe).
//
// ABI: CB `Int` arrives/returns as int32_t. EscapeKey is a pure query — unlike
// the legacy "safe exit", pressing Escape does NOT auto-close the program (that
// would need a runtime->interpreter trap channel that does not exist yet, and
// conflicts with Batch 3's clean IR Halt termination).

#include "cb_runtime.h"
#include "cb_input.h"

#include <allegro5/allegro.h>

#include <cstring>
#include <deque>

// ─── Edge-state model ──────────────────────────────────────────────────
//
// Per key/button: bit0 = down now, bit1 = changed since the last frame begin.
// Derived states match the legacy enum: Down = 0b01, Released = 0b10 (was down,
// now up this frame), Pressed = 0b11 (was up, now down this frame).
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
// frame; cb_input_frame_begin clears them (mirrors cbEnchanted's clearKeyboard
// / clearMouse flags — "ignored until the next frame").
bool sIgnoreKeyboard = false;
bool sIgnoreMouse    = false;

// CB DirectInput-style scancode -> Allegro keycode. Ported verbatim from
// legacy inputinterface.cpp. Index is the CB scancode (1..221); value is the
// ALLEGRO_KEY_* constant (0 = unmapped).
constexpr int SCANCODE_MAX = 222;
int sCBKeyMap[SCANCODE_MAX] = {0};
bool sKeyMapInit = false;

void init_key_map() {
    if (sKeyMapInit) return;
    sKeyMapInit = true;
    sCBKeyMap[1]   = ALLEGRO_KEY_ESCAPE;
    sCBKeyMap[2]   = ALLEGRO_KEY_1;
    sCBKeyMap[3]   = ALLEGRO_KEY_2;
    sCBKeyMap[4]   = ALLEGRO_KEY_3;
    sCBKeyMap[5]   = ALLEGRO_KEY_4;
    sCBKeyMap[6]   = ALLEGRO_KEY_5;
    sCBKeyMap[7]   = ALLEGRO_KEY_6;
    sCBKeyMap[8]   = ALLEGRO_KEY_7;
    sCBKeyMap[9]   = ALLEGRO_KEY_8;
    sCBKeyMap[10]  = ALLEGRO_KEY_9;
    sCBKeyMap[11]  = ALLEGRO_KEY_0;
    sCBKeyMap[12]  = ALLEGRO_KEY_EQUALS;
    sCBKeyMap[13]  = ALLEGRO_KEY_OPENBRACE;
    sCBKeyMap[14]  = ALLEGRO_KEY_BACKSPACE;
    sCBKeyMap[15]  = ALLEGRO_KEY_TAB;
    sCBKeyMap[16]  = ALLEGRO_KEY_Q;
    sCBKeyMap[17]  = ALLEGRO_KEY_W;
    sCBKeyMap[18]  = ALLEGRO_KEY_E;
    sCBKeyMap[19]  = ALLEGRO_KEY_R;
    sCBKeyMap[20]  = ALLEGRO_KEY_T;
    sCBKeyMap[21]  = ALLEGRO_KEY_Y;
    sCBKeyMap[22]  = ALLEGRO_KEY_U;
    sCBKeyMap[23]  = ALLEGRO_KEY_I;
    sCBKeyMap[24]  = ALLEGRO_KEY_O;
    sCBKeyMap[25]  = ALLEGRO_KEY_P;
    sCBKeyMap[26]  = ALLEGRO_KEY_CLOSEBRACE;
    sCBKeyMap[27]  = ALLEGRO_KEY_SEMICOLON;
    sCBKeyMap[28]  = ALLEGRO_KEY_ENTER;
    sCBKeyMap[29]  = ALLEGRO_KEY_LCTRL;
    sCBKeyMap[30]  = ALLEGRO_KEY_A;
    sCBKeyMap[31]  = ALLEGRO_KEY_S;
    sCBKeyMap[32]  = ALLEGRO_KEY_D;
    sCBKeyMap[33]  = ALLEGRO_KEY_F;
    sCBKeyMap[34]  = ALLEGRO_KEY_G;
    sCBKeyMap[35]  = ALLEGRO_KEY_H;
    sCBKeyMap[36]  = ALLEGRO_KEY_J;
    sCBKeyMap[37]  = ALLEGRO_KEY_K;
    sCBKeyMap[38]  = ALLEGRO_KEY_L;
    sCBKeyMap[39]  = ALLEGRO_KEY_TILDE;
    sCBKeyMap[40]  = ALLEGRO_KEY_QUOTE;
    sCBKeyMap[41]  = ALLEGRO_KEY_BACKSLASH;
    sCBKeyMap[42]  = ALLEGRO_KEY_LSHIFT;
    sCBKeyMap[43]  = ALLEGRO_KEY_SLASH;
    sCBKeyMap[44]  = ALLEGRO_KEY_Z;
    sCBKeyMap[45]  = ALLEGRO_KEY_X;
    sCBKeyMap[46]  = ALLEGRO_KEY_C;
    sCBKeyMap[47]  = ALLEGRO_KEY_V;
    sCBKeyMap[48]  = ALLEGRO_KEY_B;
    sCBKeyMap[49]  = ALLEGRO_KEY_N;
    sCBKeyMap[50]  = ALLEGRO_KEY_M;
    sCBKeyMap[51]  = ALLEGRO_KEY_COMMA;
    sCBKeyMap[52]  = ALLEGRO_KEY_FULLSTOP;
    sCBKeyMap[53]  = ALLEGRO_KEY_MINUS;
    sCBKeyMap[54]  = ALLEGRO_KEY_RSHIFT;
    sCBKeyMap[55]  = ALLEGRO_KEY_PAD_ASTERISK;
    sCBKeyMap[56]  = ALLEGRO_KEY_ALT;
    sCBKeyMap[57]  = ALLEGRO_KEY_SPACE;
    sCBKeyMap[58]  = ALLEGRO_KEY_CAPSLOCK;
    sCBKeyMap[59]  = ALLEGRO_KEY_F1;
    sCBKeyMap[60]  = ALLEGRO_KEY_F2;
    sCBKeyMap[61]  = ALLEGRO_KEY_F3;
    sCBKeyMap[62]  = ALLEGRO_KEY_F4;
    sCBKeyMap[63]  = ALLEGRO_KEY_F5;
    sCBKeyMap[64]  = ALLEGRO_KEY_F6;
    sCBKeyMap[65]  = ALLEGRO_KEY_F7;
    sCBKeyMap[66]  = ALLEGRO_KEY_F8;
    sCBKeyMap[67]  = ALLEGRO_KEY_F9;
    sCBKeyMap[68]  = ALLEGRO_KEY_F10;
    sCBKeyMap[69]  = ALLEGRO_KEY_PAUSE;
    sCBKeyMap[70]  = ALLEGRO_KEY_SCROLLLOCK;
    sCBKeyMap[71]  = ALLEGRO_KEY_PAD_7;
    sCBKeyMap[72]  = ALLEGRO_KEY_PAD_8;
    sCBKeyMap[73]  = ALLEGRO_KEY_PAD_9;
    sCBKeyMap[74]  = ALLEGRO_KEY_PAD_MINUS;
    sCBKeyMap[75]  = ALLEGRO_KEY_PAD_4;
    sCBKeyMap[76]  = ALLEGRO_KEY_PAD_5;
    sCBKeyMap[77]  = ALLEGRO_KEY_PAD_6;
    sCBKeyMap[78]  = ALLEGRO_KEY_PAD_PLUS;
    sCBKeyMap[79]  = ALLEGRO_KEY_PAD_1;
    sCBKeyMap[80]  = ALLEGRO_KEY_PAD_2;
    sCBKeyMap[81]  = ALLEGRO_KEY_PAD_3;
    sCBKeyMap[82]  = ALLEGRO_KEY_PAD_0;
    sCBKeyMap[83]  = ALLEGRO_KEY_PAD_DELETE;
    sCBKeyMap[86]  = ALLEGRO_KEY_BACKSLASH2;
    sCBKeyMap[87]  = ALLEGRO_KEY_F11;
    sCBKeyMap[88]  = ALLEGRO_KEY_F12;
    sCBKeyMap[156] = ALLEGRO_KEY_PAD_ENTER;
    sCBKeyMap[157] = ALLEGRO_KEY_RCTRL;
    sCBKeyMap[181] = ALLEGRO_KEY_PAD_SLASH;
    sCBKeyMap[183] = ALLEGRO_KEY_PRINTSCREEN;
    sCBKeyMap[184] = ALLEGRO_KEY_ALTGR;
    sCBKeyMap[197] = ALLEGRO_KEY_NUMLOCK;
    sCBKeyMap[199] = ALLEGRO_KEY_HOME;
    sCBKeyMap[200] = ALLEGRO_KEY_UP;
    sCBKeyMap[201] = ALLEGRO_KEY_PGUP;
    sCBKeyMap[203] = ALLEGRO_KEY_LEFT;
    sCBKeyMap[205] = ALLEGRO_KEY_RIGHT;
    sCBKeyMap[207] = ALLEGRO_KEY_END;
    sCBKeyMap[208] = ALLEGRO_KEY_DOWN;
    sCBKeyMap[209] = ALLEGRO_KEY_PGDN;
    sCBKeyMap[210] = ALLEGRO_KEY_INSERT;
    sCBKeyMap[211] = ALLEGRO_KEY_DELETE;
    sCBKeyMap[219] = ALLEGRO_KEY_LWIN;
    sCBKeyMap[220] = ALLEGRO_KEY_RWIN;
    sCBKeyMap[221] = ALLEGRO_KEY_MENU;
}

int scancode_to_allegro_key(int scan) {
    init_key_map();
    if (scan > 0 && scan < SCANCODE_MAX) return sCBKeyMap[scan];
    return 0;
}

// Reverse of scancode_to_allegro_key: an Allegro keycode -> CB scancode, or 0
// if unmapped. Used by WaitKey's function form (mirrors cbEnchanted's loop).
int allegro_key_to_scancode(int keycode) {
    init_key_map();
    if (keycode <= 0) return 0;
    for (int i = 1; i < SCANCODE_MAX; ++i) {
        if (sCBKeyMap[i] == keycode) return i;
    }
    return 0;
}

// Apply a press/release to a 2-bit slot, marking it changed only on a genuine
// transition (mirrors the legacy handleKeyEvent toggle so key-repeat events
// don't re-trigger the "changed" bit).
void apply_transition(unsigned char& slot, bool is_down) {
    if ((slot & DOWN_BIT) != (is_down ? 1 : 0)) {
        slot ^= DOWN_BIT;
        slot |= CHANGED_BIT;
    }
}

} // namespace

// ─── Per-frame hooks (called by cb_gfx.cpp's DrawScreen) ───────────────

extern "C" void cb_input_frame_begin(void) {
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

extern "C" void cb_input_handle_event(const ALLEGRO_EVENT* ev) {
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
    ALLEGRO_EVENT_QUEUE* q = cb_gfx_event_queue();
    if (!q) return 0;
    ALLEGRO_EVENT e;
    while (true) {
        al_wait_for_event(q, &e);
        cb_input_handle_event(&e);
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
    ALLEGRO_EVENT_QUEUE* q = cb_gfx_event_queue();
    if (!q) return 0;
    ALLEGRO_EVENT e;
    while (true) {
        al_wait_for_event(q, &e);
        cb_input_handle_event(&e);
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
    ALLEGRO_DISPLAY* d = cb_gfx_display();
    if (d) al_set_mouse_xy(d, x, y);
}

// Cursor mode: 0=hide, 1=standard cursor. cbEnchanted's `>1`=use image-id form
// has no equivalent here (images are opaque handles, not integer ids), so any
// value > 1 falls back to showing the standard cursor. No-op without a window.
extern "C" void cb_rt_show_mouse(int32_t mode) {
    ALLEGRO_DISPLAY* d = cb_gfx_display();
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
