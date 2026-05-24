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
};

static const CbCatalog catalog = {
    CB_CATALOG_VERSION,
    3,
    catalog_funcs
};

const CbCatalog* cb_runtime_get_catalog(void) {
    return &catalog;
}
