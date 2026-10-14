// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#pragma once

#include <duckdb/planner/logical_operator.hpp>

namespace sirius::planner {

/// Restore generated delim CTEs before resolving column bindings. Unrecognized shapes are retained.
void invert_delim_ctes(duckdb::unique_ptr<duckdb::LogicalOperator>& plan);

}  // namespace sirius::planner
