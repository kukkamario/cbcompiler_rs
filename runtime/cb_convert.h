#ifndef CB_CONVERT_H
#define CB_CONVERT_H

/* String<->number conversion primitives.
 *
 * Bare exported symbols — like the core cb_rt_string_* primitives, these are
 * NOT CB-visible catalog functions (no CB_FN row, not in CbCatalog, not in
 * CB_CATALOG_VERSION). They exist solely to service the IR Convert /
 * ConvertExplicit opcodes, giving the interpreter and a future native/LLVM
 * backend ONE shared implementation of every conversion that crosses the
 * String type — most importantly the float->string formatter, the sole
 * conversion where the interpreter and a native build could otherwise diverge
 * silently. Numeric<->numeric casts and Hex$/Bin$/Chr$/Asc are deliberately
 * NOT here.
 *
 * Allegro-free and outside any CB_NO_ALLEGRO guard: ships in both the SDK-free
 * and full runtime builds and is headless gtest-able. Returned CbString*
 * follow the core ownership rule — refcount 1, caller owns. */

#include <stdint.h>

#include "cb_runtime_core.h" /* CbString + the cb_rt_string_* primitives */

#ifdef __cplusplus
extern "C" {
#endif

/* number -> String. Byte/Short widen to int32 at the call site (lossless,
   matching the interpreter). */
CbString* cb_rt_int_to_string(int32_t v);  /* base-10, '-' sign, no leading space */
CbString* cb_rt_long_to_string(int64_t v); /* base-10 */
CbString* cb_rt_float_to_string(double v); /* 6-significant-digit CB float format */

/* String -> number. */
int64_t cb_rt_string_to_long(const CbString* s);  /* lenient leading-int, saturating, 0 on none */
double  cb_rt_string_to_float(const CbString* s); /* lenient strtod-style prefix parse, 0.0 on none */

#ifdef __cplusplus
}
#endif

#endif /* CB_CONVERT_H */
