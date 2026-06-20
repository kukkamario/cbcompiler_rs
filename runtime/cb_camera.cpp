// CoolBasic camera runtime (FD-036 Phase 2).
//
// The world<->screen camera: a world position, two independent angle fields, and
// zoom, plus the DrawToWorld flags. Ported from cbEnchanted's CameraInterface
// (src/camerainterface.cpp). The transform arithmetic lives in the Allegro-free
// cb_camera_math.h so it can be unit-tested without a display; this TU adds the
// live state, the catalog entry points, and the cb_gfx glue.
//
// ABI (see cb_runtime.h / the catalog DSL): CB Float args arrive as `double`,
// Int as `int32_t`; Float-returning funcs return `double`.
//
// Dual-angle model (FD-036, faithful to cbEnchanted): `camera_angle` (degrees)
// is what CameraAngle() reports and what MoveCamera's heading uses;
// `camera_rad_angle` (radians) feeds the world matrix. RotateCamera/TurnCamera
// set them from separate args, so they may intentionally diverge — do not
// collapse them to one field.

#include "cb_camera.h"
#include "cb_camera_math.h"

#include <allegro5/allegro.h>

#include <cmath>

namespace {

constexpr double kMinZoom = 0.00001;
constexpr double kPi = 3.14159265358979323846;

// ─── Camera state ──────────────────────────────────────────────────────
double camera_x = 0.0;
double camera_y = 0.0;
double camera_angle = 0.0;      // degrees — CameraAngle(), MoveCamera heading
double camera_rad_angle = 0.0;  // radians — world matrix rotation
double camera_zoom = 1.0;

// DrawToWorld flags (cbEnchanted gfxinterface): nonzero = draw that category in
// world (camera) space rather than screen space.
int draw_cmd_to_world = 0;
int draw_image_to_world = 0;
int draw_text_to_world = 0;

// Wrap to [0, 360) degrees (cbEnchanted MathInterface::wrapAngle).
double wrap_deg(double a) {
    while (a > 360.0) a -= 360.0;
    while (a < 0.0) a += 360.0;
    return a;
}

CbAffine current_world_affine() {
    int dw = 0, dh = 0;
    cb_gfx_design_size(&dw, &dh);
    return cb_build_world_transform(camera_x, camera_y, camera_rad_angle,
                                    camera_zoom, dw, dh);
}

}  // namespace

// ─── Position / movement ───────────────────────────────────────────────

// Absolute position; zoom is set only when above the floor (cbEnchanted clamps
// PositionCamera by ignoring a too-small zoom rather than clamping it).
extern "C" void cb_rt_position_camera(double x, double y, double zoom) {
    camera_x = x;
    camera_y = y;
    if (zoom > kMinZoom) camera_zoom = zoom;
}

// Relative move along the camera's heading. The heading combines BOTH angle
// fields (cbEnchanted camerainterface.cpp:92); `side` advances perpendicular.
extern "C" void cb_rt_move_camera(double forward, double side, double dzoom) {
    camera_zoom += dzoom;
    if (camera_zoom < kMinZoom) camera_zoom = kMinZoom;
    double move_angle = (camera_angle / 180.0) * kPi + camera_rad_angle;
    camera_x += std::cos(move_angle) * forward;
    camera_y += std::sin(move_angle) * forward;
    camera_x += std::cos(move_angle + kPi * 0.5) * side;
    camera_y += std::sin(move_angle + kPi * 0.5) * side;
}

// Relative move in absolute world space.
extern "C" void cb_rt_translate_camera(double dx, double dy, double dzoom) {
    camera_zoom += dzoom;
    if (camera_zoom < kMinZoom) camera_zoom = kMinZoom;
    camera_x += dx;
    camera_y += dy;
}

// ─── Rotation (dual-angle, faithful) ───────────────────────────────────

// Absolute rotation. `logical` sets the reported/heading angle (degrees);
// `render` sets the world-matrix rotation (stored in radians). The two are
// independent and may diverge (cbEnchanted commandRotateCamera).
extern "C" void cb_rt_rotate_camera(double logical, double render) {
    camera_angle = wrap_deg(logical);
    camera_rad_angle = (wrap_deg(render) / 180.0) * kPi;
}

// Relative rotation, mirroring RotateCamera's two-field split (cbEnchanted
// commandTurnCamera): the logical angle wraps in degrees, the render angle
// accumulates in radians and wraps to [0, 2*pi).
extern "C" void cb_rt_turn_camera(double d_logical, double d_render) {
    camera_angle = wrap_deg(camera_angle + d_logical);
    camera_rad_angle += (d_render / 180.0) * kPi;
    while (camera_rad_angle < 0.0) camera_rad_angle += 2.0 * kPi;
    while (camera_rad_angle > 2.0 * kPi) camera_rad_angle -= 2.0 * kPi;
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

// ─── DrawToWorld ───────────────────────────────────────────────────────

// Toggle world-space rendering for the three user-draw categories independently
// (cbEnchanted commandDrawToWorld). cb_gfx.cpp consults the flag getters below
// per draw command.
extern "C" void cb_rt_draw_to_world(int32_t draw_commands, int32_t draw_images,
                                    int32_t draw_text) {
    draw_cmd_to_world = draw_commands ? 1 : 0;
    draw_image_to_world = draw_images ? 1 : 0;
    draw_text_to_world = draw_text ? 1 : 0;
}

// ─── Glue for cb_gfx.cpp (see cb_camera.h) ──────────────────────────────

extern "C" const ALLEGRO_TRANSFORM* cb_camera_render_transform(void) {
    int dw = 0, dh = 0;
    cb_gfx_design_size(&dw, &dh);
    CbAffine r = cb_build_render_transform(camera_x, camera_y, camera_rad_angle,
                                           camera_zoom, dw, dh);
    static ALLEGRO_TRANSFORM t;
    al_identity_transform(&t);
    t.m[0][0] = (float)r.a;  t.m[0][1] = (float)r.b;
    t.m[1][0] = (float)r.c;  t.m[1][1] = (float)r.d;
    t.m[3][0] = (float)r.tx; t.m[3][1] = (float)r.ty;
    return &t;
}

// The plain world transform (cbEnchanted CameraInterface::getWorldTransform),
// with NO folded Y-flip — used by the tilemap render pass, which flips each
// tile anchor's Y itself (mirroring RenderTarget::convertCoords) so tiles stay
// upright.
extern "C" const ALLEGRO_TRANSFORM* cb_camera_world_transform(void) {
    int dw = 0, dh = 0;
    cb_gfx_design_size(&dw, &dh);
    CbAffine w = cb_build_world_transform(camera_x, camera_y, camera_rad_angle,
                                          camera_zoom, dw, dh);
    static ALLEGRO_TRANSFORM t;
    al_identity_transform(&t);
    t.m[0][0] = (float)w.a;  t.m[0][1] = (float)w.b;
    t.m[1][0] = (float)w.c;  t.m[1][1] = (float)w.d;
    t.m[3][0] = (float)w.tx; t.m[3][1] = (float)w.ty;
    return &t;
}

extern "C" int cb_camera_draw_cmd_to_world(void) { return draw_cmd_to_world; }
extern "C" int cb_camera_image_to_world(void) { return draw_image_to_world; }
extern "C" int cb_camera_text_to_world(void) { return draw_text_to_world; }

extern "C" double cb_camera_zoom(void) { return camera_zoom; }

// The world-space draw area (cbEnchanted CameraInterface::getDrawAreaWidth/
// Height): the rotated extent of the design resolution, divided by zoom. Used by
// the floor-object tiling fill (cb_object.cpp). Visual-only — not golden-tested.
extern "C" void cb_camera_draw_area(double* w, double* h) {
    int dw = 0, dh = 0;
    cb_gfx_design_size(&dw, &dh);
    double c = std::fabs(std::cos(camera_rad_angle));
    double s = std::fabs(std::sin(camera_rad_angle));
    double inv = 1.0 / (camera_zoom > kMinZoom ? camera_zoom : kMinZoom);
    if (w) *w = (c * dw + s * dh) * inv;
    if (h) *h = (c * dh + s * dw) * inv;
}
