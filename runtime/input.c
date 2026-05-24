#include "cb_runtime.h"
#include <allegro5/allegro.h>

int32_t cb_rt_mouse_x(void) {
    ALLEGRO_MOUSE_STATE state;
    al_get_mouse_state(&state);
    return state.x;
}

int32_t cb_rt_mouse_y(void) {
    ALLEGRO_MOUSE_STATE state;
    al_get_mouse_state(&state);
    return state.y;
}
