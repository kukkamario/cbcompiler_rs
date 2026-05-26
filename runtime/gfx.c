#include "cb_runtime.h"
#include <allegro5/allegro.h>
#include <allegro5/allegro_primitives.h>
#include <stdlib.h>

static ALLEGRO_DISPLAY *display = NULL;
static ALLEGRO_EVENT_QUEUE *event_queue = NULL;
static ALLEGRO_COLOR draw_color;
static ALLEGRO_COLOR clear_color;
static int screen_w = 0;
static int screen_h = 0;

void cb_rt_screen(int32_t w, int32_t h) {
    if (!al_is_system_installed()) {
        if (!al_init()) return;
    }
    if (!al_is_primitives_addon_initialized()) {
        if (!al_init_primitives_addon()) return;
    }
    if (!al_is_mouse_installed()) {
        al_install_mouse();
    }
    if (!al_is_keyboard_installed()) {
        al_install_keyboard();
    }

    if (display) {
        al_destroy_display(display);
    }

    display = al_create_display(w, h);
    if (!display) return;
    screen_w = w;
    screen_h = h;

    if (event_queue) {
        al_destroy_event_queue(event_queue);
    }
    event_queue = al_create_event_queue();
    al_register_event_source(event_queue, al_get_display_event_source(display));
    al_register_event_source(event_queue, al_get_mouse_event_source());
    al_register_event_source(event_queue, al_get_keyboard_event_source());

    al_set_target_backbuffer(display);
    al_set_blender(ALLEGRO_ADD, ALLEGRO_ONE, ALLEGRO_ZERO);

    draw_color = al_map_rgb(255, 255, 255);
    clear_color = al_map_rgb(0, 0, 0);
    al_clear_to_color(clear_color);
}

void cb_rt_drawscreen(void) {
    if (!display) return;

    al_flip_display();

    ALLEGRO_EVENT ev;
    while (al_get_next_event(event_queue, &ev)) {
        if (ev.type == ALLEGRO_EVENT_DISPLAY_CLOSE) {
            al_destroy_display(display);
            display = NULL;
            exit(0);
        }
    }

    al_set_target_backbuffer(display);
    al_clear_to_color(clear_color);
}

void cb_rt_color(int32_t r, int32_t g, int32_t b) {
    draw_color = al_map_rgb((unsigned char)r, (unsigned char)g, (unsigned char)b);
}

/* CB Float maps to double at the C ABI boundary — the catalog tags this
   parameter as CB_TYPE_FLOAT and the interpreter's libffi dispatch always
   pushes f64, so each function takes double regardless of what Allegro's
   own signature expects. */
void cb_rt_line(double x1, double y1, double x2, double y2) {
    if (!display) return;
    al_draw_line((float)x1, (float)y1, (float)x2, (float)y2, draw_color, 1.0f);
}

int32_t cb_rt_screen_width(void) {
    return screen_w;
}

int32_t cb_rt_screen_height(void) {
    return screen_h;
}
