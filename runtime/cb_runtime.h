#ifndef CB_RUNTIME_H
#define CB_RUNTIME_H

/* Back-compat umbrella (FD-016).
 *
 * The runtime header was split into a plugin-facing CORE ABI
 * (cb_runtime_core.h) and the internal FUNCTIONALITY prototypes
 * (cb_runtime_func.h). This umbrella re-exports both so existing translation
 * units that `#include "cb_runtime.h"` keep compiling unchanged during the
 * transition.
 *
 * New code should include the narrower header it actually needs:
 *   - plugins / core-only code → cb_runtime_core.h
 *   - the bundled functionality TUs → cb_runtime_func.h
 * Once every TU has migrated, this umbrella will be removed. */

#include "cb_runtime_core.h"
#include "cb_runtime_func.h"

#endif /* CB_RUNTIME_H */
