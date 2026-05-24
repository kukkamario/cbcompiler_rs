#ifndef CB_RUNTIME_H
#define CB_RUNTIME_H

#include <stdint.h>

#define CB_CATALOG_VERSION 2

typedef uint32_t CbTypeTag;
#define CB_TYPE_VOID    0
#define CB_TYPE_BYTE    1
#define CB_TYPE_SHORT   2
#define CB_TYPE_INT     3
#define CB_TYPE_UINT    4
#define CB_TYPE_LONG    5
#define CB_TYPE_ULONG   6
#define CB_TYPE_FLOAT   7
#define CB_TYPE_BOOL    8
#define CB_TYPE_STRING  9
/* Tags >= 10 are runtime-defined opaque types (see CbTypeDesc). */

typedef struct {
    const char* name;
    CbTypeTag   tag;
} CbTypeDesc;

typedef struct {
    const char* name;
    CbTypeTag   type;
} CbParamDesc;

typedef struct {
    const char*        name;
    const char*        symbol;
    const CbParamDesc* params;
    uint32_t           param_count;
    CbTypeTag          return_type;
    uint32_t           flags;
} CbFuncDesc;

typedef struct {
    uint32_t            version;
    uint32_t            type_count;
    const CbTypeDesc*   types;
    uint32_t            func_count;
    const CbFuncDesc*   funcs;
} CbCatalog;

const CbCatalog* cb_runtime_get_catalog(void);

/* System */
void cb_rt_print(const char* text);
int32_t cb_rt_abs_int(int32_t x);
double cb_rt_abs_float(double x);

/* Graphics */
void cb_rt_screen(int32_t w, int32_t h);
void cb_rt_drawscreen(void);
void cb_rt_color(int32_t r, int32_t g, int32_t b);
void cb_rt_line(float x1, float y1, float x2, float y2);
int32_t cb_rt_screen_width(void);
int32_t cb_rt_screen_height(void);

/* Input */
int32_t cb_rt_mouse_x(void);
int32_t cb_rt_mouse_y(void);

/* Test handle functions for opaque type testing */
uint64_t cb_rt_create_test_handle(void);
int32_t cb_rt_use_test_handle(uint64_t handle);

#endif /* CB_RUNTIME_H */
