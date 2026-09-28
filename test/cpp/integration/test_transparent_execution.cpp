/*
 * Copyright 2025, Sirius Contributors.
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

#include <catch.hpp>
#include <duckdb.hpp>
#include <duckdb/main/client_context.hpp>
#include <utils/gpu_execution_fixture.hpp>
#include <utils/sirius_test_env.hpp>
#include <utils/transparent_execution_test_utils.hpp>

#include <string>

/// Transparent execution checks using persistent native storage.
class TransparentExecutionFixture : public sirius::test::GpuExecutionFixture {
 public:
  TransparentExecutionFixture()
  {
    run_ok("SET gpu_execution = true;");
    run_ok("SET enable_duckdb_fallback = true;");
  }

  std::unique_ptr<duckdb::Connection> make_connection()
  {
    if (sirius::test::g_integration_env && sirius::test::g_integration_env->is_active()) {
      return std::make_unique<duckdb::Connection>(
        sirius::test::g_integration_env->make_connection());
    }
    REQUIRE(db);
    return std::make_unique<duckdb::Connection>(*db);
  }

  static std::string read_setting(duckdb::Connection& connection, const std::string& name)
  {
    duckdb::Value setting;
    auto lookup_result = connection.context->TryGetCurrentSetting(name, setting);
    REQUIRE(lookup_result.GetScope() != duckdb::SettingScope::INVALID);
    REQUIRE_FALSE(setting.IsNull());
    return setting.ToString();
  }

  /// Assert GPU routing and that disabling interception bypasses GPU execution.
  void require_transparent_execution(const std::string& query)
  {
    run_ok("CHECKPOINT;");
    auto before_gpu_stats = sirius::test::get_transparent_execution_stats(*con);

    // Run via transparent GPU execution (plain SQL).
    auto gpu_result = con->Query(query);
    REQUIRE(gpu_result);
    if (gpu_result->HasError()) {
      UNSCOPED_INFO("Transparent GPU error: " << gpu_result->GetError());
    }
    REQUIRE_FALSE(gpu_result->HasError());
    auto after_gpu_stats = sirius::test::get_transparent_execution_stats(*con);
    sirius::test::require_transparent_execution_delta(before_gpu_stats, after_gpu_stats, 1, 0, 1);

    // Disable transparent execution and run on CPU.
    con->Query("SET gpu_execution = false;");
    auto cpu_result = con->Query(query);
    con->Query("SET gpu_execution = true;");
    REQUIRE(cpu_result);
    REQUIRE_FALSE(cpu_result->HasError());
    auto after_cpu_stats = sirius::test::get_transparent_execution_stats(*con);
    sirius::test::require_transparent_execution_delta(after_gpu_stats, after_cpu_stats, 0, 0, 0);
  }
};

// ============================== Test cases ==============================

TEST_CASE_METHOD(TransparentExecutionFixture,
                 "transparent execution: simple filter",
                 "[transparent][integration]")
{
  // Create test data.
  con->Query("CREATE TABLE test_t AS SELECT i AS id, i * 2 AS val FROM range(1000) t(i);");
  require_transparent_execution("SELECT * FROM test_t WHERE val > 500 ORDER BY id LIMIT 10;");
}

TEST_CASE_METHOD(TransparentExecutionFixture,
                 "transparent execution: aggregation",
                 "[transparent][integration]")
{
  con->Query("CREATE TABLE test_agg AS SELECT i % 10 AS grp, i AS val FROM range(1000) t(i);");
  require_transparent_execution(
    "SELECT grp, SUM(val) AS total FROM test_agg GROUP BY grp ORDER BY grp;");
}

TEST_CASE_METHOD(TransparentExecutionFixture,
                 "transparent execution: statistics-propagated aggregates",
                 "[transparent][integration][gpu_values]")
{
  con->Query("CREATE TABLE test_stats AS SELECT i AS id FROM range(1000) t(i);");

  // DuckDB can fold these aggregates from table statistics into constant
  // EXPRESSION_GET/DUMMY_SCAN sources. Transparent execution must leave
  // STATISTICS_PROPAGATION enabled and execute the resulting GPU_VALUES plan.
  require_transparent_execution("SELECT count(*), min(id), max(id) FROM test_stats;");
}

TEST_CASE_METHOD(TransparentExecutionFixture,
                 "transparent execution: join",
                 "[transparent][integration]")
{
  con->Query("CREATE TABLE test_left AS SELECT i AS id, i * 3 AS val FROM range(100) t(i);");
  con->Query("CREATE TABLE test_right AS SELECT i * 2 AS id, i AS other FROM range(100) t(i);");
  require_transparent_execution(
    "SELECT l.id, l.val, r.other FROM test_left l JOIN test_right r ON l.id = r.id ORDER BY "
    "l.id;");
}

TEST_CASE_METHOD(TransparentExecutionFixture,
                 "transparent execution: top-N",
                 "[transparent][integration]")
{
  con->Query("CREATE TABLE test_topn AS SELECT i AS id, i * 7 AS val FROM range(10000) t(i);");
  require_transparent_execution("SELECT * FROM test_topn ORDER BY val DESC LIMIT 5;");
}

TEST_CASE_METHOD(TransparentExecutionFixture,
                 "transparent execution: fallback for unsupported (window)",
                 "[transparent][integration]")
{
  // Window functions are not supported by Sirius — should fall back to CPU silently.
  con->Query("CREATE TABLE test_win AS SELECT i AS id, i % 5 AS grp FROM range(100) t(i);");
  auto before_stats = sirius::test::get_transparent_execution_stats(*con);
  auto result       = con->Query(
    "SELECT id, grp, ROW_NUMBER() OVER (PARTITION BY grp ORDER BY id) AS rn "
          "FROM test_win ORDER BY id;");
  REQUIRE(result);
  REQUIRE_FALSE(result->HasError());
  REQUIRE(result->RowCount() == 100);
  auto after_stats = sirius::test::get_transparent_execution_stats(*con);
  sirius::test::require_transparent_execution_delta(before_stats, after_stats, 0, 1, 0);
}

TEST_CASE_METHOD(TransparentExecutionFixture,
                 "transparent execution: disable via SET",
                 "[transparent][integration]")
{
  // When disabled, queries should still work (CPU path).
  auto before_stats = sirius::test::get_transparent_execution_stats(*con);
  con->Query("SET gpu_execution = false;");
  con->Query("CREATE TABLE test_off AS SELECT i AS id FROM range(10) t(i);");
  auto result = con->Query("SELECT * FROM test_off ORDER BY id;");
  REQUIRE(result);
  REQUIRE_FALSE(result->HasError());
  REQUIRE(result->RowCount() == 10);
  auto after_stats = sirius::test::get_transparent_execution_stats(*con);
  sirius::test::require_transparent_execution_delta(before_stats, after_stats, 0, 0, 0);
  con->Query("SET gpu_execution = true;");
}

TEST_CASE_METHOD(TransparentExecutionFixture,
                 "transparent execution: gpu_execution is session scoped",
                 "[transparent][integration]")
{
  auto other_con = make_connection();

  REQUIRE(read_setting(*con, "gpu_execution") == "true");
  REQUIRE(read_setting(*other_con, "gpu_execution") == "true");

  other_con->Query("SET gpu_execution = false;");
  REQUIRE(read_setting(*con, "gpu_execution") == "true");
  REQUIRE(read_setting(*other_con, "gpu_execution") == "false");

  con->Query("SET gpu_execution = false;");
  other_con->Query("SET gpu_execution = true;");
  REQUIRE(read_setting(*con, "gpu_execution") == "false");
  REQUIRE(read_setting(*other_con, "gpu_execution") == "true");
}

TEST_CASE_METHOD(TransparentExecutionFixture,
                 "transparent execution: prepared statement can execute repeatedly",
                 "[transparent][integration]")
{
  run_ok("CREATE TABLE test_prepared AS SELECT i AS id FROM range(10) t(i);");
  run_ok("CHECKPOINT;");

  auto before_stats = sirius::test::get_transparent_execution_stats(*con);
  auto prepared     = con->Prepare("SELECT SUM(id) AS total FROM test_prepared;");
  REQUIRE(prepared);
  REQUIRE_FALSE(prepared->HasError());

  auto require_total = [](duckdb::QueryResult& result, const std::string& expected) {
    auto chunk = result.Fetch();
    REQUIRE(chunk);
    REQUIRE(chunk->size() == 1);
    REQUIRE(chunk->GetValue(0, 0).ToString() == expected);
    REQUIRE_FALSE(result.Fetch());
  };

  auto first_result = prepared->Execute();
  REQUIRE(first_result);
  REQUIRE_FALSE(first_result->HasError());
  require_total(*first_result, "45");
  first_result.reset();

  auto second_result = prepared->Execute();
  REQUIRE(second_result);
  REQUIRE_FALSE(second_result->HasError());
  require_total(*second_result, "45");

  auto after_stats = sirius::test::get_transparent_execution_stats(*con);
  // 3 rebinds: one at Prepare, one per Execute (OnExecutePrepared re-decides GPU
  // eligibility for Sirius-backed prepared statements on every execute).
  sirius::test::require_transparent_execution_delta(before_stats, after_stats, 3, 0, 2);
}
