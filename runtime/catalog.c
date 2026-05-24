#include "cb_runtime.h"
#include <stdio.h>
#include <stdlib.h>

/* System function implementations */

void cb_rt_print(const char* text) {
    if (text) {
        printf("%s\n", text);
    } else {
        printf("\n");
    }
}

int32_t cb_rt_abs_int(int32_t x) {
    return x < 0 ? -x : x;
}

double cb_rt_abs_float(double x) {
    return x < 0.0 ? -x : x;
}

uint64_t cb_rt_create_test_handle(void) {
    return 42;
}

int32_t cb_rt_use_test_handle(uint64_t handle) {
    return (int32_t)handle;
}

/* Runtime-defined opaque types */

#define CB_TYPE_TEST_HANDLE 10

static const CbTypeDesc catalog_types[] = {
    { "TestHandle", CB_TYPE_TEST_HANDLE },
};

/* Static catalog data — parameter descriptors */

static const CbParamDesc print_params[] = {
    { "text", CB_TYPE_STRING }
};

static const CbParamDesc abs_int_params[] = {
    { "value", CB_TYPE_INT }
};

static const CbParamDesc abs_float_params[] = {
    { "value", CB_TYPE_FLOAT }
};

static const CbParamDesc use_test_handle_params[] = {
    { "handle", CB_TYPE_TEST_HANDLE }
};

static const CbParamDesc screen_params[] = {
    { "width", CB_TYPE_INT },
    { "height", CB_TYPE_INT }
};

static const CbParamDesc color_params[] = {
    { "r", CB_TYPE_INT },
    { "g", CB_TYPE_INT },
    { "b", CB_TYPE_INT }
};

static const CbParamDesc line_params[] = {
    { "x1", CB_TYPE_FLOAT },
    { "y1", CB_TYPE_FLOAT },
    { "x2", CB_TYPE_FLOAT },
    { "y2", CB_TYPE_FLOAT }
};

/* Function catalog */

static const CbFuncDesc catalog_funcs[] = {
    {
        "print",
        "cb_rt_print",
        print_params,
        1,
        CB_TYPE_VOID,
        0
    },
    {
        "abs",
        "cb_rt_abs_int",
        abs_int_params,
        1,
        CB_TYPE_INT,
        0
    },
    {
        "abs",
        "cb_rt_abs_float",
        abs_float_params,
        1,
        CB_TYPE_FLOAT,
        0
    },
    {
        "screen",
        "cb_rt_screen",
        screen_params,
        2,
        CB_TYPE_VOID,
        0
    },
    {
        "drawscreen",
        "cb_rt_drawscreen",
        NULL,
        0,
        CB_TYPE_VOID,
        0
    },
    {
        "color",
        "cb_rt_color",
        color_params,
        3,
        CB_TYPE_VOID,
        0
    },
    {
        "line",
        "cb_rt_line",
        line_params,
        4,
        CB_TYPE_VOID,
        0
    },
    {
        "screenwidth",
        "cb_rt_screen_width",
        NULL,
        0,
        CB_TYPE_INT,
        0
    },
    {
        "screenheight",
        "cb_rt_screen_height",
        NULL,
        0,
        CB_TYPE_INT,
        0
    },
    {
        "mousex",
        "cb_rt_mouse_x",
        NULL,
        0,
        CB_TYPE_INT,
        0
    },
    {
        "mousey",
        "cb_rt_mouse_y",
        NULL,
        0,
        CB_TYPE_INT,
        0
    },
    {
        "createtesthandle",
        "cb_rt_create_test_handle",
        NULL,
        0,
        CB_TYPE_TEST_HANDLE,
        0
    },
    {
        "usetesthandle",
        "cb_rt_use_test_handle",
        use_test_handle_params,
        1,
        CB_TYPE_INT,
        0
    },
};

static const CbCatalog catalog = {
    CB_CATALOG_VERSION,
    1,
    catalog_types,
    13,
    catalog_funcs
};

const CbCatalog* cb_runtime_get_catalog(void) {
    return &catalog;
}
