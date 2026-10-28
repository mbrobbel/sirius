// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
/** @file
 * @brief Blocking materialized plan execution on a Sirius context. */
#pragma once
#include <sirius-c/context/context.h>
#include <sirius-c/plan/plan.h>
#ifdef __cplusplus
extern "C" {
#endif
struct ArrowArrayStream;
/** Prepare and execute an immutable plan, returning completed CPU Arrow data.
 * Blocks until GPU work completes. Does not perform CPU query fallback.
 * Borrow context and plan for the call; neither is consumed on failure.
 * out must point to an empty ArrowArrayStream. It is cleared before work begins.
 * On success, release its owned stream with its release callback. The stream
 * retains runtime resources, so release it before creating another context.
 * On failure, out->release is NULL. Diagnostics follow the common error contract.
 *
 * @par Thread safety
 * Call on the context's owning thread, with no concurrent context calls. Read and
 * release the returned stream on that thread, without concurrent stream calls.
 * The plan may be borrowed concurrently by other construction operations.
 *
 * @code{.c}
 * struct ArrowArrayStream result = {0}; // Include the Arrow C ABI definitions.
 * sirius_status status = sirius_context_execute_plan(context, plan, &result, NULL);
 * // Consume completed batches, then release the stream.
 * if (result.release) { result.release(&result); }
 * @endcode
 */
SIRIUS_EXPORT sirius_status sirius_context_execute_plan(sirius_context* context,
                                                        const sirius_plan* plan,
                                                        struct ArrowArrayStream* out,
                                                        sirius_error** error) SIRIUS_C_NOEXCEPT;
#ifdef __cplusplus
}
#endif
