#ifndef CB_RUNTIME_H
#define CB_RUNTIME_H

#include <stdint.h>

#define CB_CATALOG_VERSION 3

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

/* Opaque handle types. Each runtime-defined type is a forward-declared
   struct that is never defined — only pointers to it ever appear in
   function signatures. The C++ catalog's type_tag<T> is specialized for
   the pointer forms only (`T*` and `const T*`); passing a custom type by
   value is a compile error because the primary template is undefined.

   CONVENTION: Runtime functions MUST take and return custom types via
   pointer:
       MyType* cb_rt_make_thing(void);          // owning return
       double  cb_rt_thing_x(const MyType* t);  // borrowing read-only
       void    cb_rt_set_x(MyType* t, double x);// borrowing mutate
   The runtime is free to encode the handle as a slab index, an actual
   pointer, etc.; only the bit pattern matters. */
typedef struct CbTestHandle CbTestHandle;

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
    /* Statically-linked address of the runtime function. The interpreter
       dispatches through this; the LLVM backend uses `symbol` for
       declare/call emission. The CB_FN macro guarantees `symbol` (via #)
       and `fn_ptr` reference the same identifier. */
    void             (*fn_ptr)(void);
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

#ifdef __cplusplus
extern "C" {
#endif

const CbCatalog* cb_runtime_get_catalog(void);

/* System */
void cb_rt_print(const char* text);
int32_t cb_rt_abs_int(int32_t x);
double cb_rt_abs_float(double x);

/* Math */
double cb_rt_sqrt(double x);

/* Graphics */
void cb_rt_screen(int32_t w, int32_t h);
void cb_rt_drawscreen(void);
void cb_rt_color(int32_t r, int32_t g, int32_t b);
void cb_rt_line(double x1, double y1, double x2, double y2);
int32_t cb_rt_screen_width(void);
int32_t cb_rt_screen_height(void);

/* Input */
int32_t cb_rt_mouse_x(void);
int32_t cb_rt_mouse_y(void);

/* Test handle functions for opaque type testing */
CbTestHandle* cb_rt_create_test_handle(void);
int32_t cb_rt_use_test_handle(const CbTestHandle* handle);

#ifdef __cplusplus
}
#endif

#endif /* CB_RUNTIME_H */
