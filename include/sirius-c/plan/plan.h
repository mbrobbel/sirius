// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
/** @file
 * @brief Immutable plan construction for materialized execution. */
#pragma once
#include <sirius-c/error.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/** An immutable relation. Retain/release each owning reference once.
 * Construction copies names and paths without accessing files or GPUs.
 * Concurrent reads and construction are allowed with live references.
 * Distinct references may be released on any thread; do not release a reference
 * concurrently with its use. Output slots require exclusive access.
 * Factories clear output slots, borrow inputs, and leave inputs intact on failure.
 */
typedef struct sirius_relation sirius_relation;
/** An immutable scoped boolean expression, with the same ownership/thread rules as a relation. */
typedef struct sirius_expression sirius_expression;
/** An immutable single-root plan, independent of any context; the same ownership/thread rules
 * apply. */
typedef struct sirius_plan sirius_plan;

/** One signed 64-bit input field. Name bytes must be nonempty UTF-8 without NUL.
 * Only nullable fields are supported initially; nullable must be 1. Field order must match the
 * Parquet file. */
typedef struct sirius_i64_field {
  const char* name;
  size_t name_size;
  uint32_t nullable;
} sirius_i64_field;

/** Fixed-width comparison selector. SQL null comparisons produce null. */
typedef uint32_t sirius_comparison;
enum {
  SIRIUS_EQUAL         = 0,
  SIRIUS_NOT_EQUAL     = 1,
  SIRIUS_LESS          = 2,
  SIRIUS_LESS_EQUAL    = 3,
  SIRIUS_GREATER       = 4,
  SIRIUS_GREATER_EQUAL = 5
};

/** Describe one local Parquet file with an explicit i64-only schema.
 * path is a nonempty native path without NUL, glob characters, or a URI scheme.
 * No file is opened until execution. The file must remain unchanged during execution.
 * @code{.c}
 * sirius_i64_field field = {"id", 2, 1};
 * sirius_relation *scan = NULL;
 * sirius_status status = sirius_relation_parquet("data.parquet", 12, &field, 1, &scan, NULL);
 * @endcode
 */
SIRIUS_EXPORT sirius_status sirius_relation_parquet(const char* path,
                                                    size_t path_size,
                                                    const sirius_i64_field* fields,
                                                    size_t field_count,
                                                    sirius_relation** out,
                                                    sirius_error** error) SIRIUS_C_NOEXCEPT;

/** Construct a comparison between an input column and an i64 literal.
 * The column is zero-based and scoped to input; no implicit numeric coercion occurs.
 * @code{.c}
 * sirius_expression *predicate = NULL;
 * sirius_status status = sirius_expression_compare_i64(scan, 0, SIRIUS_GREATER, 10, &predicate,
 * NULL);
 * @endcode
 */
SIRIUS_EXPORT sirius_status sirius_expression_compare_i64(const sirius_relation* input,
                                                          size_t column,
                                                          sirius_comparison comparison,
                                                          int64_t literal,
                                                          sirius_expression** out,
                                                          sirius_error** error) SIRIUS_C_NOEXCEPT;

/** Return a filtered relation, preserving column order; only true predicate rows survive.
 * Reject a predicate from another relation's scope.
 * @code{.c}
 * sirius_relation *filtered = NULL;
 * sirius_status status = sirius_relation_filter(scan, predicate, &filtered, NULL);
 * @endcode
 */
SIRIUS_EXPORT sirius_status sirius_relation_filter(const sirius_relation* input,
                                                   const sirius_expression* predicate,
                                                   sirius_relation** out,
                                                   sirius_error** error) SIRIUS_C_NOEXCEPT;

/** Create a complete immutable plan; output names are inherited from the root.
 * @code{.c}
 * sirius_plan *plan = NULL;
 * sirius_status status = sirius_plan_from_root(filtered, &plan, NULL);
 * @endcode
 */
SIRIUS_EXPORT sirius_status sirius_plan_from_root(const sirius_relation* root,
                                                  sirius_plan** out,
                                                  sirius_error** error) SIRIUS_C_NOEXCEPT;

/** Acquire an owning reference; NULL is allowed. @code{.c} sirius_relation_retain(scan); @endcode
 */
SIRIUS_EXPORT void sirius_relation_retain(sirius_relation* value) SIRIUS_C_NOEXCEPT;
/** Release an owning reference; NULL is allowed. @code{.c} sirius_relation_release(scan); @endcode
 */
SIRIUS_EXPORT void sirius_relation_release(sirius_relation* value) SIRIUS_C_NOEXCEPT;
/** Acquire an owning reference; NULL is allowed. @code{.c} sirius_expression_retain(predicate);
 * @endcode */
SIRIUS_EXPORT void sirius_expression_retain(sirius_expression* value) SIRIUS_C_NOEXCEPT;
/** Release an owning reference; NULL is allowed. @code{.c} sirius_expression_release(predicate);
 * @endcode */
SIRIUS_EXPORT void sirius_expression_release(sirius_expression* value) SIRIUS_C_NOEXCEPT;
/** Acquire an owning reference; NULL is allowed. @code{.c} sirius_plan_retain(plan); @endcode */
SIRIUS_EXPORT void sirius_plan_retain(sirius_plan* value) SIRIUS_C_NOEXCEPT;
/** Release an owning reference; NULL is allowed. @code{.c} sirius_plan_release(plan); @endcode */
SIRIUS_EXPORT void sirius_plan_release(sirius_plan* value) SIRIUS_C_NOEXCEPT;
#ifdef __cplusplus
}
#endif
