/*
 * Copyright 2026, Sirius Contributors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// copy_logical_plan clones plans structurally: scans are cloned with FunctionData::Copy()
// instead of the serialize round-trip whose deserialization re-runs the table function's bind
// (for parquet, a full multi-file re-bind that re-opens every file). These tests pin down the
// three contracts the transparent-execution path relies on:
//   1. a parquet scan takes the cheap bind-data-copy path and the clone is serialization-
//      equivalent to the original,
//   2. the original plan (including its pushed table_filters, which create_plan moves out of
//      the plan it consumes) is left intact and never aliased by the clone,
//   3. a scan whose bind data has no usable Copy() (TableFunctionData's default throws) falls
//      back to the per-leaf serialize round-trip instead of failing the whole capture.

#include "transparent/sirius_optimizer_extension.hpp"

#include <catch.hpp>
#include <duckdb.hpp>
#include <duckdb/common/serializer/binary_serializer.hpp>
#include <duckdb/common/serializer/memory_stream.hpp>
#include <duckdb/planner/operator/logical_get.hpp>
#include <utils/parquet_fixture_utils.hpp>

#include <cstddef>
#include <string>
#include <vector>

namespace {

std::vector<duckdb::data_t> serialize_plan(duckdb::LogicalOperator const& plan,
                                           duckdb::ClientContext& context)
{
  duckdb::MemoryStream stream(duckdb::Allocator::Get(context));
  duckdb::SerializationOptions options;
  options.serialization_compatibility = duckdb::SerializationCompatibility::Latest();
  duckdb::BinarySerializer serializer(stream, options);
  serializer.Begin();
  plan.Serialize(serializer);
  serializer.End();
  return {stream.GetData(), stream.GetData() + stream.GetPosition()};
}

duckdb::LogicalGet* find_get(duckdb::LogicalOperator& op)
{
  if (op.type == duckdb::LogicalOperatorType::LOGICAL_GET) {
    return &op.Cast<duckdb::LogicalGet>();
  }
  for (auto& child : op.children) {
    if (auto* get = find_get(*child)) { return get; }
  }
  return nullptr;
}

std::size_t count_nodes(duckdb::LogicalOperator const& op)
{
  std::size_t n = 1;
  for (auto const& child : op.children) {
    n += count_nodes(*child);
  }
  return n;
}

}  // namespace

TEST_CASE("copy_logical_plan clones parquet scans without the serialize round-trip",
          "[transparent][copy_logical_plan]")
{
  // No GPU / SiriusContext needed: copy_logical_plan is pure host-side plan surgery.
  sirius::test::scoped_sirius_disable disable_sirius;
  duckdb::DuckDB db(nullptr);
  duckdb::Connection con(db);

  sirius::test::scratch_dir scratch("copy_logical_plan");
  auto const parquet_path = scratch.file("scan.parquet");
  {
    auto r = con.Query("COPY (SELECT range AS k, range * 2 AS v FROM range(1000)) TO " +
                       sirius::test::sql_literal(parquet_path) + " (FORMAT PARQUET);");
    REQUIRE_FALSE(r->HasError());
  }

  // The WHERE clause is pushed into the scan's table_filters by the optimizer.
  auto plan = con.ExtractPlan("SELECT k FROM read_parquet(" +
                              sirius::test::sql_literal(parquet_path) + ") WHERE k > 10;");
  REQUIRE(plan != nullptr);
  auto* original_get = find_get(*plan);
  REQUIRE(original_get != nullptr);
  REQUIRE_FALSE(original_get->table_filters.filters.empty());

  auto const nodes_before = count_nodes(*plan);
  auto const bytes_before = serialize_plan(*plan, *con.context);

  sirius::transparent::plan_copy_stats stats;
  auto clone = sirius::transparent::copy_logical_plan(*plan, *con.context, &stats);
  REQUIRE(clone != nullptr);

  // The parquet scan took the cheap path.
  REQUIRE(stats.nodes == nodes_before);
  REQUIRE(stats.bind_copied_gets == 1);
  REQUIRE(stats.serialized_gets == 0);

  // The original plan is intact (children reattached, nothing moved out of it) and the clone is
  // serialization-equivalent to it.
  REQUIRE(count_nodes(*plan) == nodes_before);
  REQUIRE(serialize_plan(*plan, *con.context) == bytes_before);
  REQUIRE(serialize_plan(*clone, *con.context) == bytes_before);

  // table_filters is a deep copy: create_plan moves filters out of the plan it consumes, so
  // gutting the clone's scan must leave the original untouched.
  auto* cloned_get = find_get(*clone);
  REQUIRE(cloned_get != nullptr);
  REQUIRE_FALSE(cloned_get->table_filters.filters.empty());
  cloned_get->table_filters.filters.clear();
  cloned_get->bind_data.reset();
  REQUIRE_FALSE(original_get->table_filters.filters.empty());
  REQUIRE(original_get->bind_data != nullptr);
}

TEST_CASE("copy_logical_plan falls back to the serialize round-trip for non-copyable bind data",
          "[transparent][copy_logical_plan]")
{
  sirius::test::scoped_sirius_disable disable_sirius;
  duckdb::DuckDB db(nullptr);
  duckdb::Connection con(db);

  // range()'s bind data derives from TableFunctionData without overriding Copy(), whose default
  // throws — the per-leaf serialize fallback must kick in (range() has no serializer either, so
  // deserialization re-binds from the recorded parameters).
  auto plan = con.ExtractPlan("SELECT * FROM range(100) WHERE range > 5;");
  REQUIRE(plan != nullptr);
  REQUIRE(find_get(*plan) != nullptr);

  auto const nodes_before = count_nodes(*plan);
  auto const bytes_before = serialize_plan(*plan, *con.context);

  // The serialize fallback re-binds the function on deserialization, which needs an active
  // transaction — the real call sites (optimizer hook, OnFinalizePrepare, execute) always run
  // inside one.
  con.BeginTransaction();
  sirius::transparent::plan_copy_stats stats;
  auto clone = sirius::transparent::copy_logical_plan(*plan, *con.context, &stats);
  con.Commit();
  REQUIRE(clone != nullptr);

  REQUIRE(stats.serialized_gets == 1);
  REQUIRE(stats.bind_copied_gets == 0);

  REQUIRE(count_nodes(*clone) == nodes_before);
  REQUIRE(serialize_plan(*plan, *con.context) == bytes_before);
  REQUIRE(serialize_plan(*clone, *con.context) == bytes_before);
}
