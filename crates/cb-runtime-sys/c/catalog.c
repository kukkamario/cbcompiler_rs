#include "cb_runtime.h"

/* Stub implementations (catalog-only milestone, no execution yet) */

void cb_rt_print(const char* text) {
    (void)text;
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

/* Static catalog data */

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
    5,
    catalog_funcs
};

const CbCatalog* cb_runtime_get_catalog(void) {
    return &catalog;
}
