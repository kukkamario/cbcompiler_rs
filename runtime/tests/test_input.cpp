// FD-022: unit tests for the input edge-state machine. Drives the Allegro-free
// state functions in cb_input.cpp with synthetic ALLEGRO_EVENTs — no display,
// no al_init. cb::input::frame_begin/handle_event read event fields and mutate
// file-static state only, so this runs fully headless.

#include "cb_input.h"  // cb::input::frame_begin, cb::input::handle_event, ALLEGRO_EVENT

#include <gtest/gtest.h>

#include <cstring>

// The CB-visible query functions are extern "C" in cb_input.cpp; forward-declare
// the ones we exercise rather than pulling in the whole catalog header.
extern "C" {
int32_t cb_rt_key_down(int32_t scancode);
int32_t cb_rt_key_up(int32_t scancode);
int32_t cb_rt_key_hit(int32_t scancode);
int32_t cb_rt_mouse_down(int32_t button);
int32_t cb_rt_mouse_up(int32_t button);
int32_t cb_rt_mouse_hit(int32_t button);
int32_t cb_rt_mouse_move_x(void);
int32_t cb_rt_mouse_move_y(void);
void cb_rt_clear_keys(void);
void cb_rt_clear_mouse(void);
}

namespace {
// cb_keys.def: CB scancode 30 -> ALLEGRO_KEY_A.
constexpr int CB_SCAN_A = 30;

ALLEGRO_EVENT make_key_event(int type, int keycode) {
    ALLEGRO_EVENT ev;
    std::memset(&ev, 0, sizeof(ev));
    ev.type = type;
    ev.keyboard.keycode = keycode;
    return ev;
}

ALLEGRO_EVENT make_mouse_button_event(int type, int button) {
    ALLEGRO_EVENT ev;
    std::memset(&ev, 0, sizeof(ev));
    ev.type = type;
    ev.mouse.button = button;
    return ev;
}

ALLEGRO_EVENT make_mouse_axes_event(int dx, int dy, int dz) {
    ALLEGRO_EVENT ev;
    std::memset(&ev, 0, sizeof(ev));
    ev.type = ALLEGRO_EVENT_MOUSE_AXES;
    ev.mouse.dx = dx;
    ev.mouse.dy = dy;
    ev.mouse.dz = dz;
    return ev;
}
}  // namespace

// Press → Hit & Down for one frame; Down only while held; Up exactly the frame
// of release. (State is process-global, so reset at the top of every test.)
TEST(Input, KeyPressHoldRelease) {
    cb_rt_clear_keys();      // zero key state
    cb::input::frame_begin();  // clears the ignore flag set by clear_keys

    ALLEGRO_EVENT down = make_key_event(ALLEGRO_EVENT_KEY_DOWN, ALLEGRO_KEY_A);
    cb::input::handle_event(&down);
    EXPECT_EQ(cb_rt_key_hit(CB_SCAN_A), 1);
    EXPECT_EQ(cb_rt_key_down(CB_SCAN_A), 1);
    EXPECT_EQ(cb_rt_key_up(CB_SCAN_A), 0);

    // Next frame, still held: Down stays, Hit clears.
    cb::input::frame_begin();
    EXPECT_EQ(cb_rt_key_hit(CB_SCAN_A), 0);
    EXPECT_EQ(cb_rt_key_down(CB_SCAN_A), 1);
    EXPECT_EQ(cb_rt_key_up(CB_SCAN_A), 0);

    // Release this frame: Up fires once, Down clears.
    cb::input::frame_begin();
    ALLEGRO_EVENT up = make_key_event(ALLEGRO_EVENT_KEY_UP, ALLEGRO_KEY_A);
    cb::input::handle_event(&up);
    EXPECT_EQ(cb_rt_key_hit(CB_SCAN_A), 0);
    EXPECT_EQ(cb_rt_key_down(CB_SCAN_A), 0);
    EXPECT_EQ(cb_rt_key_up(CB_SCAN_A), 1);

    // Following frame: Up clears (lasts exactly one frame).
    cb::input::frame_begin();
    EXPECT_EQ(cb_rt_key_up(CB_SCAN_A), 0);
}

// Auto-repeat KEY_DOWN while already held must not re-trigger Hit.
TEST(Input, KeyRepeatDoesNotRetriggerHit) {
    cb_rt_clear_keys();
    cb::input::frame_begin();
    ALLEGRO_EVENT down = make_key_event(ALLEGRO_EVENT_KEY_DOWN, ALLEGRO_KEY_A);
    cb::input::handle_event(&down);
    EXPECT_EQ(cb_rt_key_hit(CB_SCAN_A), 1);

    cb::input::frame_begin();
    cb::input::handle_event(&down);  // repeat: already down, no transition
    EXPECT_EQ(cb_rt_key_hit(CB_SCAN_A), 0);
    EXPECT_EQ(cb_rt_key_down(CB_SCAN_A), 1);
}

TEST(Input, MouseButtonPressHoldRelease) {
    cb_rt_clear_mouse();
    cb::input::frame_begin();

    ALLEGRO_EVENT down =
        make_mouse_button_event(ALLEGRO_EVENT_MOUSE_BUTTON_DOWN, 1);
    cb::input::handle_event(&down);
    EXPECT_EQ(cb_rt_mouse_hit(1), 1);
    EXPECT_EQ(cb_rt_mouse_down(1), 1);
    EXPECT_EQ(cb_rt_mouse_up(1), 0);

    cb::input::frame_begin();
    ALLEGRO_EVENT up = make_mouse_button_event(ALLEGRO_EVENT_MOUSE_BUTTON_UP, 1);
    cb::input::handle_event(&up);
    EXPECT_EQ(cb_rt_mouse_hit(1), 0);
    EXPECT_EQ(cb_rt_mouse_down(1), 0);
    EXPECT_EQ(cb_rt_mouse_up(1), 1);
}

// Movement deltas accumulate within a frame and zero at the next frame begin.
TEST(Input, MouseMoveAccumulatesPerFrame) {
    cb::input::frame_begin();
    ALLEGRO_EVENT a = make_mouse_axes_event(3, -2, 0);
    ALLEGRO_EVENT b = make_mouse_axes_event(4, 1, 0);
    cb::input::handle_event(&a);
    cb::input::handle_event(&b);
    EXPECT_EQ(cb_rt_mouse_move_x(), 7);
    EXPECT_EQ(cb_rt_mouse_move_y(), -1);

    cb::input::frame_begin();
    EXPECT_EQ(cb_rt_mouse_move_x(), 0);
    EXPECT_EQ(cb_rt_mouse_move_y(), 0);
}
