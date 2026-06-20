// CoolBasic camera runtime (FD-036 Phase 2).
//
// The world<->screen camera: a world position, two independent angle fields, and
// zoom, plus the DrawToWorld flags. The transform arithmetic lives in the
// Allegro-free cb_camera_math.h so it can be unit-tested without a display; this
// TU adds the live state, the catalog entry points, and the cb::gfx glue.
//
// ABI (see cb_runtime.h / the catalog DSL): CB Float args arrive as `double`,
// Int as `int32_t`; Float-returning funcs return `double`.
//
// Dual-angle model (FD-036, a CoolBasic quirk): `camera_angle` (degrees)
// is what CameraAngle() reports and what MoveCamera's heading uses;
// `camera_rad_angle` (radians) feeds the world matrix. RotateCamera/TurnCamera
// set them from separate args, so they may intentionally diverge — do not
// collapse them to one field.

#include "cb_camera.h"
#include "cb_camera_math.h"
#include "cb_gfx.h"           // cb::gfx::design_size / window_size
#include "cb_object.h"        // cb::object::pick_at (CameraPick funnel)
#include "cb_runtime_func.h"  // CbObject + cb_rt_object_x/y/angle accessors

#include <allegro5/allegro.h>

#include <cmath>

namespace cb::camera {

namespace {

constexpr double k_min_zoom = 0.00001;
constexpr double k_pi = 3.14159265358979323846;

// ─── Camera state ──────────────────────────────────────────────────────
double camera_x = 0.0;
double camera_y = 0.0;
double camera_angle = 0.0;      // degrees — CameraAngle(), MoveCamera heading
double camera_rad_angle = 0.0;  // radians — world matrix rotation
double camera_zoom = 1.0;

// DrawToWorld flags: nonzero = draw that category in
// world (camera) space rather than screen space.
int g_draw_cmd_to_world = 0;
int g_draw_image_to_world = 0;
int g_draw_text_to_world = 0;

// CameraFollow state (FD-036 Phase 5). When following, the camera steps toward
// the target once per frame (cb::camera::update_follow, called from DrawScreen).
bool is_following = false;
int follow_style = 0;
double follow_setting = 0.0;
const CbObject* follow_target = nullptr;

// Wrap to [0, 360) degrees.
double wrap_deg(double a) {
    while (a > 360.0) a -= 360.0;
    while (a < 0.0) a += 360.0;
    return a;
}

CbAffine current_world_affine() {
    int dw = 0, dh = 0;
    cb::gfx::design_size(&dw, &dh);
    return cb_build_world_transform(camera_x, camera_y, camera_rad_angle,
                                    camera_zoom, dw, dh);
}

}  // namespace

// ─── Position / movement ───────────────────────────────────────────────

// Absolute position; zoom is set only when above the floor (CoolBasic ignores a
// too-small zoom in PositionCamera rather than clamping it).
extern "C" void cb_rt_position_camera(double x, double y, double zoom) {
    camera_x = x;
    camera_y = y;
    if (zoom > k_min_zoom) camera_zoom = zoom;
}

// Relative move along the camera's heading. The heading combines BOTH angle
// fields; `side` advances perpendicular.
extern "C" void cb_rt_move_camera(double forward, double side, double dzoom) {
    camera_zoom += dzoom;
    if (camera_zoom < k_min_zoom) camera_zoom = k_min_zoom;
    double move_angle = (camera_angle / 180.0) * k_pi + camera_rad_angle;
    camera_x += std::cos(move_angle) * forward;
    camera_y += std::sin(move_angle) * forward;
    camera_x += std::cos(move_angle + k_pi * 0.5) * side;
    camera_y += std::sin(move_angle + k_pi * 0.5) * side;
}

// Relative move in absolute world space.
extern "C" void cb_rt_translate_camera(double dx, double dy, double dzoom) {
    camera_zoom += dzoom;
    if (camera_zoom < k_min_zoom) camera_zoom = k_min_zoom;
    camera_x += dx;
    camera_y += dy;
}

// ─── Rotation (dual-angle, faithful) ───────────────────────────────────

// Absolute rotation. `logical` sets the reported/heading angle (degrees);
// `render` sets the world-matrix rotation (stored in radians). The two are
// independent and may diverge (CoolBasic's RotateCamera).
extern "C" void cb_rt_rotate_camera(double logical, double render) {
    camera_angle = wrap_deg(logical);
    camera_rad_angle = (wrap_deg(render) / 180.0) * k_pi;
}

// Relative rotation, mirroring RotateCamera's two-field split: the logical angle
// wraps in degrees, the render angle accumulates in radians and wraps to
// [0, 2*pi).
extern "C" void cb_rt_turn_camera(double d_logical, double d_render) {
    camera_angle = wrap_deg(camera_angle + d_logical);
    camera_rad_angle += (d_render / 180.0) * k_pi;
    while (camera_rad_angle < 0.0) camera_rad_angle += 2.0 * k_pi;
    while (camera_rad_angle > 2.0 * k_pi) camera_rad_angle -= 2.0 * k_pi;
}

// ─── Queries ───────────────────────────────────────────────────────────

extern "C" double cb_rt_camera_x(void) { return camera_x; }
extern "C" double cb_rt_camera_y(void) { return camera_y; }
extern "C" double cb_rt_camera_angle(void) { return camera_angle; }

// Mouse position converted to world coordinates through the current camera. The
// transform math is identical to MouseX/MouseY fed through screen->world.
extern "C" double cb_rt_mouse_wx(void) {
    ALLEGRO_MOUSE_STATE st;
    al_get_mouse_state(&st);
    double x = st.x, y = st.y;
    cb_screen_to_world(current_world_affine(), x, y);
    return x;
}

extern "C" double cb_rt_mouse_wy(void) {
    ALLEGRO_MOUSE_STATE st;
    al_get_mouse_state(&st);
    double x = st.x, y = st.y;
    cb_screen_to_world(current_world_affine(), x, y);
    return y;
}

// ─── Object-aware camera (FD-036 Phase 5) ───────────────────────────────

// PointCamera(obj): rotate the logical/reported camera angle to face the object.
// Bug fix: the reference passed obj.Y for BOTH atan2 args; we use X then Y. Sets
// only camera_angle (the logical field) — the world matrix angle
// (camera_rad_angle) stays independent, matching CoolBasic.
extern "C" void cb_rt_point_camera(const CbObject* obj) {
    if (!obj) return;
    double ox = cb_rt_object_x(obj);
    double oy = cb_rt_object_y(obj);
    camera_angle = (k_pi - std::atan2(camera_y - oy, camera_x - ox)) / k_pi * 180.0;
}

// CameraFollow(obj, style, setting): follow an object. style 1 = smooth lerp,
// 2 = margin deadzone, 3 = orbit. The step runs once per frame in DrawScreen
// (cb_camera_update_follow).
extern "C" void cb_rt_camera_follow(const CbObject* obj, int32_t style, double setting) {
    is_following = true;
    follow_setting = setting;
    follow_style = style;
    follow_target = obj;
}

// CloneCameraPosition(obj): snap the camera to the object's position; stop following.
extern "C" void cb_rt_clone_camera_position(const CbObject* obj) {
    if (!obj) return;
    is_following = false;
    camera_x = cb_rt_object_x(obj);
    camera_y = cb_rt_object_y(obj);
}

// CloneCameraOrientation(obj): snap the camera angle to the object's. Bug #4 fix:
// set BOTH the logical angle and the render (matrix) angle so the view actually
// rotates (setting only the logical field leaves the matrix desynced).
extern "C" void cb_rt_clone_camera_orientation(const CbObject* obj) {
    if (!obj) return;
    double a = wrap_deg(cb_rt_object_angle(obj));
    camera_angle = a;
    camera_rad_angle = (a / 180.0) * k_pi;
}

// CameraPick(sx, sy): pick the object under a screen coordinate (screen→world,
// then the point-in-shape test). Sets PickedObject.
extern "C" void cb_rt_camera_pick(double sx, double sy) {
    screen_to_world(&sx, &sy);
    cb::object::pick_at(sx, sy);
}

// ─── DrawToWorld ───────────────────────────────────────────────────────

// Toggle world-space rendering for the three user-draw categories independently
// (CoolBasic's DrawToWorld). cb_gfx.cpp consults the flag getters below
// per draw command.
extern "C" void cb_rt_draw_to_world(int32_t draw_commands, int32_t draw_images,
                                    int32_t draw_text) {
    g_draw_cmd_to_world = draw_commands ? 1 : 0;
    g_draw_image_to_world = draw_images ? 1 : 0;
    g_draw_text_to_world = draw_text ? 1 : 0;
}

// ─── Glue for cb_gfx.cpp (see cb_camera.h) ──────────────────────────────

const ALLEGRO_TRANSFORM* render_transform(void) {
    int dw = 0, dh = 0;
    cb::gfx::design_size(&dw, &dh);
    CbAffine r = cb_build_render_transform(camera_x, camera_y, camera_rad_angle,
                                           camera_zoom, dw, dh);
    static ALLEGRO_TRANSFORM t;
    al_identity_transform(&t);
    t.m[0][0] = (float)r.a;  t.m[0][1] = (float)r.b;
    t.m[1][0] = (float)r.c;  t.m[1][1] = (float)r.d;
    t.m[3][0] = (float)r.tx; t.m[3][1] = (float)r.ty;
    return &t;
}

// The plain world transform with NO folded Y-flip — used by the tilemap render
// pass, which flips each tile anchor's Y itself so tiles stay upright.
const ALLEGRO_TRANSFORM* world_transform(void) {
    int dw = 0, dh = 0;
    cb::gfx::design_size(&dw, &dh);
    CbAffine w = cb_build_world_transform(camera_x, camera_y, camera_rad_angle,
                                          camera_zoom, dw, dh);
    static ALLEGRO_TRANSFORM t;
    al_identity_transform(&t);
    t.m[0][0] = (float)w.a;  t.m[0][1] = (float)w.b;
    t.m[1][0] = (float)w.c;  t.m[1][1] = (float)w.d;
    t.m[3][0] = (float)w.tx; t.m[3][1] = (float)w.ty;
    return &t;
}

int draw_cmd_to_world(void) { return g_draw_cmd_to_world; }
int image_to_world(void) { return g_draw_image_to_world; }
int text_to_world(void) { return g_draw_text_to_world; }

double zoom(void) { return camera_zoom; }

// The world-space draw area: the rotated extent of the *physical window* size,
// divided by zoom (the window size, NOT the design resolution the transform
// centers on). Used by the floor-object tiling fill (cb_object.cpp). Visual-only
// — not golden-tested.
void draw_area(double* w, double* h) {
    int ww = 0, wh = 0;
    cb::gfx::window_size(&ww, &wh);
    double c = std::fabs(std::cos(camera_rad_angle));
    double s = std::fabs(std::sin(camera_rad_angle));
    double inv = 1.0 / (camera_zoom > k_min_zoom ? camera_zoom : k_min_zoom);
    if (w) *w = (c * ww + s * wh) * inv;
    if (h) *h = (c * wh + s * ww) * inv;
}

// Screen → world through the live camera (the inverse world transform, then
// Y-flip). Same path the MouseWX/WY funcs use; shared by ScreenPositionObject
// (cb_object.cpp) and CameraPick.
void screen_to_world(double* x, double* y) {
    if (!x || !y) return;
    cb_screen_to_world(current_world_affine(), *x, *y);
}

// Step the camera toward its follow target once. Called per frame from
// DrawScreen (Phase 5c). No-op when not following.
void update_follow(void) {
    if (!is_following || !follow_target) return;
    double tx = cb_rt_object_x(follow_target);
    double ty = cb_rt_object_y(follow_target);
    switch (follow_style) {
        case 1: {  // smooth lerp — larger setting = slower approach
            camera_x += (tx - camera_x) / follow_setting;
            camera_y += (ty - camera_y) / follow_setting;
            break;
        }
        case 2: {  // margin deadzone, measured against the PHYSICAL window size
            int ww = 0, wh = 0;
            cb::gfx::window_size(&ww, &wh);
            double half_w = ww / 2.0, half_h = wh / 2.0;
            if (tx < camera_x - half_w + follow_setting)
                camera_x += tx - (camera_x - half_w + follow_setting);
            if (tx > camera_x + half_w - follow_setting)
                camera_x += tx - (camera_x + half_w - follow_setting);
            if (ty < camera_y - half_h + follow_setting)
                camera_y += ty - (camera_y - half_h + follow_setting);
            if (ty > camera_y + half_h - follow_setting)
                camera_y += ty - (camera_y + half_h - follow_setting);
            break;
        }
        case 3: {  // orbit at follow_setting distance around the target's angle
            double rad = cb_rt_object_angle(follow_target) / 180.0 * k_pi;
            camera_x = tx + std::cos(rad) * follow_setting;
            camera_y = ty + std::sin(rad) * follow_setting;
            break;
        }
    }
}

}  // namespace cb::camera
