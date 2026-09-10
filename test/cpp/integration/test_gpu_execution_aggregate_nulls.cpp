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

// GPU-vs-CPU correctness for NULL handling in aggregates (issue #1095):
// COUNT(*) vs COUNT(col), NULL-skipping SUM/AVG/MIN/MAX, all-NULL inputs and
// groups, GROUP BY on a NULL key, and COUNT(DISTINCT) with NULLs.
//
// Every query goes through the shared file-backed GpuExecutionFixture, which
// runs it once on the GPU (asserting a real GPU execution with no fallback) and
// once on DuckDB CPU, then compares the results. The comparator is order-
// insensitive, which suits GROUP BY output.

#include <catch.hpp>
#include <duckdb.hpp>
#include <utils/gpu_execution_fixture.hpp>

#include <string>

namespace {

// `g` is the group key and deliberately contains a NULL group; `v`/`d`/`f` are
// nullable value columns; `allnull` is entirely NULL. Group 3 has only NULL
// values (an all-NULL group), and one group key is NULL.
class AggNullFixture : public sirius::test::GpuExecutionFixture {
 public:
  AggNullFixture()
  {
    run_ok(
      "CREATE TABLE agg_n ("
      "  g       INTEGER,"
      "  v       INTEGER,"
      "  d       DECIMAL(10,2),"
      "  f       DOUBLE,"
      "  allnull INTEGER);");
    run_ok(
      "INSERT INTO agg_n VALUES "
      "(1,    10,   1.00,  1.5,  NULL),"
      "(1,    20,   2.00,  NULL, NULL),"
      "(1,    NULL, NULL,  2.5,  NULL),"
      "(2,    5,    5.00,  5.0,  NULL),"
      "(2,    NULL, NULL,  NULL, NULL),"
      "(NULL, 100,  10.00, 10.0, NULL),"  // NULL group key
      "(NULL, 200,  NULL,  NULL, NULL),"  // NULL group key
      "(3,    NULL, NULL,  NULL, NULL);"  // all-NULL group
    );
    run_ok("CHECKPOINT;");
  }
};

}  // namespace

//===----------------------------------------------------------------------===//
// Verified-correct coverage
//===----------------------------------------------------------------------===//

TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution COUNT(*) vs COUNT(col) with NULLs",
                 "[integration][gpu_execution][aggregate][nulls]")
{
  // COUNT(*) counts rows; COUNT(col) skips NULLs.
  compare_gpu_vs_cpu("SELECT COUNT(*), COUNT(v), COUNT(d), COUNT(f) FROM agg_n");
}

TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution ungrouped SUM/MIN/MAX skip NULLs",
                 "[integration][gpu_execution][aggregate][nulls]")
{
  compare_gpu_vs_cpu("SELECT SUM(v), MIN(v), MAX(v) FROM agg_n");
  compare_gpu_vs_cpu("SELECT SUM(d), MIN(d), MAX(d) FROM agg_n");
  compare_gpu_vs_cpu("SELECT SUM(f), MIN(f), MAX(f) FROM agg_n");
}

TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution GROUP BY groups NULL keys together",
                 "[integration][gpu_execution][aggregate][nulls]")
{
  // The NULL group key forms its own group; the all-NULL group (g=3) yields
  // COUNT(v)=0 and SUM/AVG/MIN/MAX = NULL.
  compare_gpu_vs_cpu(
    "SELECT g, COUNT(*), COUNT(v), SUM(v), AVG(v), MIN(v), MAX(v) FROM agg_n GROUP BY g");
}

TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution grouped SUM/AVG over NULL values",
                 "[integration][gpu_execution][aggregate][nulls]")
{
  compare_gpu_vs_cpu("SELECT g, SUM(d), AVG(d) FROM agg_n GROUP BY g");
}

TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution grouped COUNT(DISTINCT) ignores NULLs",
                 "[integration][gpu_execution][aggregate][nulls]")
{
  // Grouped COUNT(DISTINCT) supports INTEGER and skips NULLs.
  compare_gpu_vs_cpu("SELECT g, COUNT(DISTINCT v) FROM agg_n GROUP BY g");
}

// INTEGER remains outside the scoped BIGINT/VARCHAR ungrouped distinct path.
TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution ungrouped COUNT(DISTINCT) falls back to CPU",
                 "[integration][gpu_execution][aggregate][nulls]")
{
  expect_plan_fallback_matches_cpu("SELECT COUNT(DISTINCT v) FROM agg_n");
}

TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution ungrouped AVG skips NULLs (non-null denominator)",
                 "[integration][gpu_execution][aggregate][nulls]")
{
  // AVG divides SUM by the count of non-null values, not the row count, so AVG
  // over a NULL-containing column matches DuckDB: AVG(v) = 335/5 = 67.
  compare_gpu_vs_cpu("SELECT AVG(v), AVG(d), AVG(f) FROM agg_n");
}

// A wholly-NULL column checkpoints to CONSTANT all-null validity; the native
// scan synthesizes its null mask, so aggregates must see NULLs rather than
// sentinel values. Split into ungrouped/grouped cases: Catch2 aborts a test
// case at the first REQUIRE failure, so bundling them would leave the grouped
// query unexercised.
TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution ungrouped aggregates over a wholly-NULL column",
                 "[integration][gpu_execution][aggregate][nulls]")
{
  compare_gpu_vs_cpu(
    "SELECT SUM(allnull), AVG(allnull), MIN(allnull), MAX(allnull), COUNT(allnull) FROM agg_n");
}

TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution grouped aggregates over a wholly-NULL column",
                 "[integration][gpu_execution][aggregate][nulls]")
{
  compare_gpu_vs_cpu("SELECT g, SUM(allnull), COUNT(allnull) FROM agg_n GROUP BY g");
}

TEST_CASE_METHOD(AggNullFixture,
                 "gpu_execution ungrouped COUNT DISTINCT BIGINT and VARCHAR",
                 "[integration][gpu_execution][aggregate][count_distinct]")
{
  run_ok("CREATE TABLE distinct_n (id BIGINT, phrase VARCHAR)");
  run_ok(
    "INSERT INTO distinct_n VALUES (-1, ''), (7, 'hello'), (7, 'hello'), "
    "(9000000000000, 'é'), (NULL, NULL)");
  run_ok("CHECKPOINT");
  for (auto const* column : {"id", "phrase"}) {
    auto query = std::string("SELECT COUNT(DISTINCT ") + column + ") FROM distinct_n";
    compare_gpu_vs_cpu(query);
    compare_gpu_vs_cpu(query);
    compare_gpu_vs_cpu(query + " WHERE id IS NULL");
    compare_gpu_vs_cpu(query + " WHERE id = 42");
  }
  run_ok("CREATE TABLE distinct_empty (id BIGINT, phrase VARCHAR)");
  run_ok("CHECKPOINT");
  compare_gpu_vs_cpu("SELECT COUNT(DISTINCT id) FROM distinct_empty");
  compare_gpu_vs_cpu("SELECT COUNT(DISTINCT phrase) FROM distinct_empty");
  expect_plan_fallback_matches_cpu("SELECT COUNT(DISTINCT id), COUNT(*) FROM distinct_n");
  expect_plan_fallback_matches_cpu(
    "SELECT COUNT(DISTINCT id), COUNT(DISTINCT phrase) FROM distinct_n");
  expect_plan_fallback_matches_cpu("SELECT SUM(DISTINCT id) FROM distinct_n");
  expect_plan_fallback_matches_cpu(
    "SELECT COUNT(DISTINCT id) FILTER (WHERE id > 0) FROM distinct_n");
  expect_plan_fallback_matches_cpu("SELECT COUNT(DISTINCT (id + 1)) FROM distinct_n");
}
