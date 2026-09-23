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

/**
 * @file test_gpu_execution_multi_format.cpp
 * @brief Integration tests for multi-format scan support through gpu_execution.
 *
 * C++ checks execution routes and internal state. SQL suites compare results
 * for Parquet, Hive, Iceberg, and CSV scans.
 */

#include "yyjson.hpp"

#include <cudf/utilities/default_stream.hpp>

#include <catch.hpp>
#include <duckdb.hpp>
#include <op/scan/iceberg_metadata_reader.hpp>
#include <signal.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>
#include <utils/child_process_environment.hpp>
#include <utils/dynamic_filter_test_utils.hpp>
#include <utils/parquet_fixture_utils.hpp>
#include <utils/sirius_test_env.hpp>
#include <utils/transparent_execution_test_utils.hpp>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <memory>
#include <optional>
#include <sstream>
#include <string>
#include <thread>

namespace fs = std::filesystem;

static fs::path get_project_root()
{
#ifdef SIRIUS_PROJECT_ROOT
  return fs::path(SIRIUS_PROJECT_ROOT);
#else
  return fs::path(__FILE__).parent_path().parent_path().parent_path().parent_path();
#endif
}

struct sirius_config_env_guard {
  sirius_config_env_guard(const std::string& config_path)
  {
    setenv("SIRIUS_CONFIG_FILE", config_path.c_str(), 1);
  }
  ~sirius_config_env_guard() { unsetenv("SIRIUS_CONFIG_FILE"); }
};

/**
 * @brief How a query is expected to be routed by the transparent optimizer.
 *
 * Correctness alone cannot distinguish "the GPU computed the right answer" from "the GPU
 * declined and DuckDB computed the right answer" — both return the same rows. Every
 * comparison therefore asserts the route as well, so a silent regression to CPU shows up as a
 * test failure rather than a pass. This matters most for the iceberg cases below, where the
 * delete gate deliberately routes some tables to CPU: when a delete kind starts running on the
 * GPU, its expectation flips from `plan_fallback` to `gpu` and the assertion proves it.
 */
enum class gpu_route {
  gpu,              ///< Plan generated and executed on the GPU.
  plan_fallback,    ///< Declined at plan time (NotImplementedException) — DuckDB CPU ran it.
  runtime_fallback  ///< GPU plan installed, then failed at execute time — CPU fallback ran it.
};

/**
 * @brief Base fixture asserting execution routes for multi-format tests.
 */
class MultiFormatFixtureBase {
 public:
  MultiFormatFixtureBase()
  {
    if (sirius::test::g_integration_env && sirius::test::g_integration_env->is_active()) {
      con =
        std::make_unique<duckdb::Connection>(sirius::test::g_integration_env->make_connection());
    } else {
      auto cfg_path = fs::path(__FILE__).parent_path() / "integration.yaml";
      REQUIRE(fs::exists(cfg_path));
      config_guard = std::make_unique<sirius_config_env_guard>(cfg_path.string());
      db           = std::make_unique<duckdb::DuckDB>(nullptr);
      con          = std::make_unique<duckdb::Connection>(*db);
    }
  }

  /// Collect all rows from a MaterializedQueryResult as sorted vectors of stringified values.
  static std::vector<std::vector<std::string>> collect_rows(duckdb::MaterializedQueryResult& result)
  {
    std::vector<std::vector<std::string>> rows;
    for (duckdb::idx_t r = 0; r < result.RowCount(); r++) {
      std::vector<std::string> row;
      row.reserve(result.ColumnCount());
      for (duckdb::idx_t c = 0; c < result.ColumnCount(); c++) {
        row.push_back(result.GetValue(c, r).ToString());
      }
      rows.push_back(std::move(row));
    }
    std::sort(rows.begin(), rows.end());
    return rows;
  }

  /// Assert that `query` took `route`, in terms of the transparent-execution counters.
  static void require_route(const duckdb::SiriusContext::transparent_execution_stats& before,
                            const duckdb::SiriusContext::transparent_execution_stats& after,
                            gpu_route route)
  {
    switch (route) {
      case gpu_route::gpu:
        sirius::test::require_transparent_execution_delta(before, after, 1, 0, 1, 0);
        break;
      case gpu_route::plan_fallback:
        // create_plan threw NotImplementedException: no rebind, no GPU execution, one fallback.
        sirius::test::require_transparent_execution_delta(before, after, 0, 1, 0, 0);
        break;
      case gpu_route::runtime_fallback:
        // The rebind succeeded and the GPU operator ran, then bailed to the stashed CPU plan.
        sirius::test::require_transparent_execution_delta(before, after, 1, 0, 1, 1);
        break;
    }
  }

  using query_result = duckdb::unique_ptr<duckdb::MaterializedQueryResult>;

  std::pair<query_result, query_result> require_execution_routes(const std::string& query,
                                                                 gpu_route route)
  {
    // Enable transparent GPU execution
    con->Query("SET gpu_execution = true;");
    auto before_gpu_stats = sirius::test::get_transparent_execution_stats(*con);

    // Run on GPU (transparent — plain SQL goes through Sirius optimizer hook)
    auto gpu_result = con->Query(query);
    REQUIRE(gpu_result);
    if (gpu_result->HasError()) {
      UNSCOPED_INFO("transparent GPU execution error: " << gpu_result->GetError());
    }
    REQUIRE_FALSE(gpu_result->HasError());
    auto after_gpu_stats = sirius::test::get_transparent_execution_stats(*con);
    require_route(before_gpu_stats, after_gpu_stats, route);

    // Run on CPU (disable transparent execution)
    con->Query("SET gpu_execution = false;");
    auto cpu_result = con->Query(query);
    con->Query("SET gpu_execution = true;");
    REQUIRE(cpu_result);
    REQUIRE_FALSE(cpu_result->HasError());
    auto after_cpu_stats = sirius::test::get_transparent_execution_stats(*con);
    sirius::test::require_transparent_execution_delta(after_gpu_stats, after_cpu_stats, 0, 0, 0);

    return {std::move(gpu_result), std::move(cpu_result)};
  }

  void require_gpu_execution(const std::string& query)
  {
    require_execution_routes(query, gpu_route::gpu);
  }

  std::unique_ptr<duckdb::DuckDB> db;
  std::unique_ptr<duckdb::Connection> con;
  std::unique_ptr<sirius_config_env_guard> config_guard;
};

class ParquetCountCarrierFixture : public MultiFormatFixtureBase {
 public:
  ParquetCountCarrierFixture() : scratch{"parquet_count_carrier"}
  {
    sirius::test::scoped_sirius_disable disable;
    duckdb::DuckDB writer_db(nullptr);
    duckdb::Connection writer(writer_db);
    std::string const rows =
      "SELECT i::BIGINT AS id, (i * 0.25)::DOUBLE AS amount, "
      "('label-' || i)::VARCHAR AS label, (i % 100)::SMALLINT AS small, "
      "DATE '2024-01-01' + i::INTEGER AS day, "
      "CASE WHEN i % 3 = 0 THEN NULL::BOOLEAN ELSE i % 2 = 0 END AS flag "
      "FROM range(257) t(i)";
    auto write = [&](std::string const& relative_path, std::string const& query) {
      fs::create_directories((scratch.path() / relative_path).parent_path());
      auto result = writer.Query("COPY (" + query + ") TO " + scratch.file_literal(relative_path) +
                                 " (FORMAT PARQUET)");
      INFO(relative_path);
      REQUIRE(result);
      if (result->HasError()) { UNSCOPED_INFO(result->GetError()); }
      REQUIRE_FALSE(result->HasError());
      REQUIRE(fs::file_size(scratch.path() / relative_path) > 0);
    };
    write("flat.parquet", rows);
    write("parts/a.parquet", rows + " WHERE i < 129");
    write("parts/b.parquet", rows + " WHERE i >= 129");
    write("hive/part=2024/a.parquet", rows + " WHERE i < 129");
    write("hive/part=2025/b.parquet", rows + " WHERE i >= 129");
    write("evolved/part=2024/a.parquet", rows + " WHERE i < 129");
    write("evolved/part=2025/b.parquet",
          "SELECT id, amount, label, small, day FROM (" + rows + ") WHERE id >= 129");
    write("strings.parquet", "SELECT 'a-' || i AS a, 'b-' || i AS b FROM range(257) t(i)");
    write("null_carrier.parquet",
          "SELECT CASE WHEN i % 4 = 0 THEN NULL::BIGINT ELSE i END AS id, "
          "NULL::BOOLEAN AS flag FROM range(257) t(i)");
    write("empty.parquet", rows + " WHERE false");
    // Keep COUNT(*) in the scan instead of answering it from parquet statistics.
    optimizer_guard =
      std::make_unique<sirius::test::disabled_optimizers_guard>(*con, "statistics_propagation");
  }

  std::string scan(std::string const& relative_path, bool hive = false) const
  {
    return "read_parquet(" + scratch.file_literal(relative_path) +
           (hive ? ", hive_partitioning=true)" : ", hive_partitioning=false)");
  }

  sirius::test::scratch_dir scratch;
  std::unique_ptr<sirius::test::disabled_optimizers_guard> optimizer_guard;
};

TEST_CASE_METHOD(ParquetCountCarrierFixture,
                 "parquet flat count star and filtered counts match the CPU oracle",
                 "[integration][scan][parquet][gpu][carrier]")
{
  SECTION("count star includes rows with a null carrier")
  {
    require_gpu_execution("SELECT count(*) FROM " + scan("flat.parquet"));
  }
  SECTION("predicate on a non-carrier column")
  {
    require_gpu_execution("SELECT count(*) FROM " + scan("flat.parquet") + " WHERE id >= 129");
  }
  SECTION("all-pruned data predicate")
  {
    require_gpu_execution("SELECT count(*) FROM " + scan("flat.parquet") + " WHERE id < 0");
  }
}

TEST_CASE_METHOD(ParquetCountCarrierFixture,
                 "parquet counts remain correct with an entirely null carrier column",
                 "[integration][scan][parquet][gpu][carrier]")
{
  require_gpu_execution("SELECT count(*) FROM " + scan("null_carrier.parquet"));
  require_gpu_execution("SELECT count(id) FROM " + scan("null_carrier.parquet"));
}

TEST_CASE_METHOD(ParquetCountCarrierFixture,
                 "parquet partition-only grouped counts match the CPU oracle",
                 "[integration][scan][parquet][gpu][carrier]")
{
  require_gpu_execution("SELECT part, count(*) FROM " + scan("hive/part=*/*.parquet", true) +
                        " GROUP BY part ORDER BY part");
}

TEST_CASE_METHOD(ParquetCountCarrierFixture,
                 "parquet multi-file count star matches the CPU oracle",
                 "[integration][scan][parquet][gpu][carrier]")
{
  require_gpu_execution("SELECT count(*) FROM " + scan("parts/*.parquet"));
}

TEST_CASE_METHOD(ParquetCountCarrierFixture,
                 "parquet partition counts tolerate a missing non-output carrier candidate",
                 "[integration][scan][parquet][gpu][carrier][schema_evolution]")
{
  require_gpu_execution(
    "SELECT part, count(*) FROM read_parquet(" + scratch.file_literal("evolved/part=*/*.parquet") +
    ", hive_partitioning=true, union_by_name=true) GROUP BY part ORDER BY part");
}

TEST_CASE_METHOD(ParquetCountCarrierFixture,
                 "parquet all-varchar count star matches the CPU oracle",
                 "[integration][scan][parquet][gpu][carrier]")
{
  require_gpu_execution("SELECT count(*) FROM " + scan("strings.parquet"));
}

TEST_CASE_METHOD(ParquetCountCarrierFixture,
                 "parquet empty-file count star matches the CPU oracle",
                 "[integration][scan][parquet][gpu][carrier]")
{
  require_gpu_execution("SELECT count(*) FROM " + scan("empty.parquet"));
}

// CSV result comparisons are in test/sqltest/suites/csv_disabled.

//===----------------------------------------------------------------------===//
// Iceberg scan tests
//
// These cases were hidden ([.] tag) and hard-skipped (iceberg_available = false) while iceberg
// was out of the build, so they passed for a month without running any SQL — which is how the
// rot went unnoticed. They are now unhidden, unskipped, and a missing iceberg extension is a
// hard failure rather than a WARN-and-return.
//
// C++ asserts execution routes and internal state. The iceberg SQL suite compares
// DuckDB and Sirius results against each other and the original literal expectations.
//===----------------------------------------------------------------------===//

/**
 * @brief Iceberg test fixture.
 *
 * Loads the community iceberg extension and resolves the fixture tables under
 * test/cpp/integration/data/. Tests assert GPU and fallback execution routes.
 */
class GPUExecutionIcebergFixture : public MultiFormatFixtureBase {
 public:
  // Iceberg-extension build these expectations were verified against. A different build is not
  // automatically wrong, so a mismatch warns rather than fails — but it is recorded, so a
  // behaviour change arriving with an extension upgrade is attributable instead of mysterious.
  //
  // What the pin is really protecting is the CPU oracle for EQUALITY deletes: the 1.4.4-era
  // build ignored them silently, so an oracle on that build would agree with a GPU path that
  // dropped them too. 45163a28 (the build INSTALL resolves for DuckDB v1.5.5) was checked by
  // hand against test/cpp/integration/data/iceberg_v2_equality_delete and returns the 3
  // surviving rows, not all 5. Re-run that check when bumping this, rather than assuming a
  // newer build only improves.
  static constexpr const char* kVerifiedIcebergVersion = "45163a28";

  // Which delete kinds the GPU scan path applies itself. Positional deletes and V3 deletion
  // vectors are applied by iceberg_gpu_ingestible (they share one per-file position map), so
  // those tables must execute as GPU_SCAN — asserting the route is what distinguishes "the GPU
  // applied the deletes" from "the GPU quietly handed the table back to DuckDB", since both
  // return the same rows. Equality deletes match on key values and need the key columns
  // force-projected into the scan, which is not wired; the gate still sends them to CPU.
  static constexpr gpu_route kPositionalDeleteRoute = gpu_route::gpu;
  static constexpr gpu_route kDeletionVectorRoute   = gpu_route::gpu;
  static constexpr gpu_route kEqualityDeleteRoute   = gpu_route::plan_fallback;

  // Every GPU-route case pins its snapshot; see pinned_scan().
  static constexpr gpu_route kUnpinnedRoute = gpu_route::plan_fallback;

  GPUExecutionIcebergFixture()
  {
    require_repo_root_cwd();
    load_iceberg_extension();

    auto root = get_project_root();
    v1_path   = (root / "test/cpp/integration/data/iceberg_v1").string();
    v2_path   = (root / "test/cpp/integration/data/iceberg_v2_delete").string();

    auto const conf       = root / "test/cpp/integration/data/iceberg_conformance";
    conf_append_only_path = (conf / "append_only/conf/append_only").string();
    conf_drop_readd_path  = (conf / "drop_readd/conf/drop_readd").string();
    conf_rename_col_path  = (conf / "rename_col/conf/rename_col").string();
    conf_add_column_path  = (conf / "add_column/conf/add_column").string();
  }

  /**
   * @brief Fail early, and legibly, when the process cwd is not the repo root.
   *
   * Fixture metadata (manifest lists, manifests, data files) stores repo-relative paths, so
   * iceberg_scan resolves them against the process cwd — passing an absolute table path does
   * not help. Without this check every iceberg case fails with an opaque
   * `IO Error: Cannot open file "test/cpp/..."` that reads like a missing fixture.
   */
  static void require_repo_root_cwd()
  {
    if (!fs::exists("test/cpp/integration/data/iceberg_v1/metadata")) {
      FAIL(
        "iceberg fixture metadata stores repo-relative paths, so these tests must run with "
        "the repo root as cwd; cwd is "
        << fs::current_path().string());
    }
  }

  /**
   * @brief Load the iceberg extension, failing hard if it is unavailable.
   *
   * Deliberately LOAD without INSTALL: a test must not reach the network, and an INSTALL here
   * would silently paper over an unprovisioned environment. The extension is a build/CI
   * dependency and is installed once, out of band.
   */
  void load_iceberg_extension()
  {
    auto load = con->Query("LOAD iceberg;");
    REQUIRE(load);
    if (load->HasError()) {
      FAIL("the iceberg extension is required by these tests but could not be loaded: "
           << load->GetError()
           << "\nprovision it once with:  duckdb -unsigned -c \"INSTALL avro; INSTALL iceberg;\"");
    }

    auto version = con->Query(
      "SELECT extension_version FROM duckdb_extensions() WHERE extension_name = 'iceberg';");
    REQUIRE(version);
    REQUIRE_FALSE(version->HasError());
    REQUIRE(version->RowCount() == 1);
    auto const installed = version->GetValue(0, 0).ToString();
    if (installed != kVerifiedIcebergVersion) {
      WARN("iceberg extension version " << installed << " differs from the verified "
                                        << kVerifiedIcebergVersion
                                        << "; reader behaviour differences trace to this");
    }

    // The pyiceberg corpus has no `version-hint.text`, so it is legible only with version
    // guessing on. Set it in THIS session rather than relying on the extension's default: the
    // default is not knowable from the version string (see require_session_can_read), and a
    // suite that depends on an ambient value fails in a way that reads like a Sirius gate bug.
    // Session-scoped on purpose -- Sirius mirrors the session, so this is also what makes the
    // mirror the thing under test rather than a global nobody set.
    auto set_guessing = con->Query("SET unsafe_enable_version_guessing = true;");
    REQUIRE(set_guessing);
    REQUIRE_FALSE(set_guessing->HasError());
  }

  /// Non-data manifest entries (V2 positional, V2 equality, V3 deletion vectors — the last
  /// reported as content='POSITION_DELETES', file_format='PUFFIN') at the snapshot being read.
  ///
  /// ⚠️ The 'EXISTING' here is the `content` column's, where it means "is a DATA file" — NOT the
  /// `status` column's, where it means "carried over from an earlier snapshot". Both columns use
  /// that string for unrelated things.
  ///
  /// Counts entries regardless of status, so this is a count of what the manifest LISTS, not of
  /// what still applies. That is deliberate: it keeps the canary honest about a table whose only
  /// delete entry has been retired, which still lists one and applies none.
  int64_t delete_file_count(const std::string& table_path, const std::string& extra_args = "")
  {
    auto result = con->Query("SELECT count(*) FROM iceberg_metadata('" + table_path + "'" +
                             extra_args + ") WHERE content <> 'EXISTING';");
    REQUIRE(result);
    if (result->HasError()) { UNSCOPED_INFO("iceberg_metadata error: " << result->GetError()); }
    REQUIRE_FALSE(result->HasError());
    REQUIRE(result->RowCount() == 1);
    return result->GetValue(0, 0).GetValue<int64_t>();
  }

  /**
   * @brief `iceberg_scan(path, snapshot_from_id = <current>)` — the only spelling the GPU path
   *        accepts; the unpinned form declines at plan time.
   *
   * The id comes from the metadata's own `current-snapshot-id`, NOT from the highest sequence
   * number: rollback, branches and staged WAP commits all break that derivation, which is the
   * defect this gate exists to close.
   */
  std::string pinned_scan(const std::string& table_path)
  {
    return "iceberg_scan('" + table_path +
           "', snapshot_from_id = " + std::to_string(current_snapshot_id(table_path)) + ")";
  }

  /// Read from the metadata JSON `version-hint.text` points at. yyjson rather than the json
  /// extension, which the test environment does not install.
  int64_t current_snapshot_id(const std::string& table_path)
  {
    fs::path const meta_dir = fs::path(table_path) / "metadata";
    fs::path metadata_json;

    std::ifstream hint(meta_dir / "version-hint.text");
    std::string version;
    if (hint && std::getline(hint, version) && !version.empty()) {
      metadata_json = meta_dir / ("v" + version + ".metadata.json");
    }
    // The pyiceberg corpus has no version hint; its `00001-<uuid>` names are zero-padded, so
    // lexicographic order is version order. An unpadded v1..v10 series would need the hint.
    if (metadata_json.empty() || !fs::exists(metadata_json)) {
      for (auto const& entry : fs::directory_iterator(meta_dir)) {
        auto const name = entry.path().filename().string();
        if (name.size() > 14 && name.compare(name.size() - 14, 14, ".metadata.json") == 0 &&
            entry.path().string() > metadata_json.string()) {
          metadata_json = entry.path();
        }
      }
    }
    INFO("iceberg table metadata: " << metadata_json.string());
    REQUIRE(fs::exists(metadata_json));

    std::ifstream f(metadata_json, std::ios::binary);
    REQUIRE(f);
    std::string const text{std::istreambuf_iterator<char>(f), std::istreambuf_iterator<char>()};

    auto* doc = duckdb_yyjson::yyjson_read(text.data(), text.size(), 0);
    REQUIRE(doc != nullptr);
    auto* root = duckdb_yyjson::yyjson_doc_get_root(doc);
    auto* cur  = duckdb_yyjson::yyjson_obj_get(root, "current-snapshot-id");
    REQUIRE(cur != nullptr);
    auto const id = duckdb_yyjson::yyjson_get_sint(cur);
    duckdb_yyjson::yyjson_doc_free(doc);
    REQUIRE(id > 0);
    return id;
  }

  /**
   * @brief Canary: prove the table really carries delete files before asserting deletes work.
   *
   * The engine once fabricated empty delete data, and the delete tests still looked correct —
   * with nothing to delete, a right-looking row set proves nothing. Asserting the delete-file
   * count up front means a delete test can no longer pass by having no deletes to apply.
   */
  void require_delete_files(const std::string& table_path, int64_t expected)
  {
    INFO("delete-file canary for " << table_path);
    REQUIRE(delete_file_count(table_path) == expected);
  }

  /// Assert the candidate route and that disabling GPU execution reaches only DuckDB.
  void require_iceberg_execution(const std::string& query, gpu_route route)
  {
    INFO("query: " << query);

    con->Query("SET gpu_execution = true;");
    auto before     = sirius::test::get_transparent_execution_stats(*con);
    auto gpu_result = con->Query(query);
    REQUIRE(gpu_result);
    if (gpu_result->HasError()) {
      UNSCOPED_INFO("transparent GPU execution error: " << gpu_result->GetError());
    }
    REQUIRE_FALSE(gpu_result->HasError());
    require_route(before, sirius::test::get_transparent_execution_stats(*con), route);

    auto const before_cpu = sirius::test::get_transparent_execution_stats(*con);
    con->Query("SET gpu_execution = false;");
    auto cpu_result = con->Query(query);
    con->Query("SET gpu_execution = true;");
    REQUIRE(cpu_result);
    if (cpu_result->HasError()) {
      UNSCOPED_INFO("DuckDB reader error: " << cpu_result->GetError());
    }
    REQUIRE_FALSE(cpu_result->HasError());
    // Prove the oracle is actually DuckDB. `gpu_execution=false` disables transparent
    // interception but does NOT unload Sirius (only SIRIUS_DISABLE=1 does), so "we set the flag"
    // is not evidence — zero rebinds, zero fallbacks and zero GPU executions is. Without this,
    // a comparison against an oracle that had quietly stayed on the GPU would be the engine
    // agreeing with itself.
    sirius::test::require_transparent_execution_delta(
      before_cpu, sirius::test::get_transparent_execution_stats(*con), 0, 0, 0);
  }

  /**
   * @brief Assert the user's own session can read this table, which is what Sirius relies on.
   *
   * pyiceberg's SqlCatalog writes no `version-hint.text`, so reading these tables needs
   * `unsafe_enable_version_guessing`. The fixture now SETs it in this session, so the corpus
   * precondition is explicit rather than inherited.
   *
   * ⚠️ Do NOT restore a claim about what the flag defaults to. Measured 2026-08-27 in fresh
   * processes, the runtime default differs per build:
   *
   *     iceberg 75726455 (DuckDB v1.5.4) -> false
   *     iceberg 45163a28 (DuckDB v1.5.5) -> true     <- kVerifiedIcebergVersion
   *
   * while duckdb-iceberg's source registers `Value::BOOLEAN(false)` at BOTH `45163a28` and on
   * `main`. The shipped binary therefore does not match the commit whose version it reports, so
   * the version string is not evidence of the behaviour and neither is the source. This asserts
   * the only thing that is checkable in-process: the OUTER connection can read the table, with
   * the extension version and the effective flag both reported when it cannot.
   *
   * Sirius must not be the thing that makes these tables legible -- it mirrors the session's
   * value rather than forcing one, which is why this checks the session and not Sirius.
   */
  void require_session_can_read(const std::string& table_path)
  {
    auto r = con->Query("SELECT count(*) FROM iceberg_scan('" + table_path + "');");
    REQUIRE(r);
    if (r->HasError()) {
      auto flag = con->Query("SELECT current_setting('unsafe_enable_version_guessing');");
      auto ver  = con->Query(
        "SELECT extension_version FROM duckdb_extensions() WHERE extension_name = 'iceberg';");
      FAIL("this session cannot read conformance table '"
           << table_path << "': " << r->GetError() << "\nunsafe_enable_version_guessing is "
           << (flag && !flag->HasError() ? flag->GetValue(0, 0).ToString() : "unreadable")
           << ", iceberg extension_version is "
           << (ver && !ver->HasError() && ver->RowCount() == 1 ? ver->GetValue(0, 0).ToString()
                                                               : "unreadable")
           << " (verified: " << kVerifiedIcebergVersion
           << "). Both are reported together because neither predicts the other: the shipped "
              "binary's version string does not identify the tree it was built from.");
    }
  }

  std::string v1_path;
  std::string v2_path;
  std::string conf_append_only_path;
  std::string conf_drop_readd_path;
  std::string conf_rename_col_path;
  std::string conf_add_column_path;
};

//===----------------------------------------------------------------------===//
// Conformance — tables written by pyiceberg, expectations recorded from pyiceberg's own
// scan (see test/cpp/integration/data/iceberg_conformance/README.md).
//
// Why these exist: every fixture above was hand-built, and the whole set was green at
// 27 cases / 580 assertions while the GPU scan path returned silently WRONG rows for a
// dropped-and-re-added column. A hand-built fixture cannot catch that, because it encodes
// the same assumption the implementation makes — that Iceberg columns resolve by NAME.
// They resolve by field ID.
//
// `rename_col` and `add_column` were previously NOT wired up here: name resolution failed at
// runtime, the query took the runtime fallback, and the next GPU query on that connection hung
// forever. Catch2 has no per-case timeout, so a hang stalls the whole suite rather than failing
// it, and no tag expresses "expected to hang". They now decline at PLAN time, so they are safe
// to run in-process and are wired up below.
//
// The SQL conformance suites assert connection liveness with a second query in the same
// worker. Their query timeouts isolate a poisoned connection without stalling the suite.
// C++ retains the execution-route assertions.
//===----------------------------------------------------------------------===//

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - delete data is read once per query",
                 "[integration][gpu_execution][iceberg]")
{
  // The planner builds the plan TWICE for an iceberg scan -- iceberg_scan is not serializable,
  // so validation is followed by a replan at execute -- and each build walked the manifest list,
  // every manifest, and every positional-delete file a row at a time, on the planning thread.
  //
  // Asserting on a counter rather than a log line is deliberate: release builds compile
  // INFO/DEBUG logging out, so there is no observable signal at this level, and a green suite
  // would only show the memo did not corrupt results -- not that it removed any work. Without
  // the memo this delta is 2; the memo is what makes it 1. If someone later breaks it, the cost
  // does not quietly return, this fails.
  require_delete_files(v2_path, 1);

  // Pinned, because an unpinned scan declines before any delete is read and the delta would be
  // zero -- which would look like the memo working perfectly while it was never consulted.
  auto const before = sirius::op::scan::iceberg_delete_data_uncached_read_count();
  auto result       = con->Query("SELECT count(*) FROM " + pinned_scan(v2_path) + ";");
  REQUIRE(result);
  REQUIRE_FALSE(result->HasError());
  auto const after = sirius::op::scan::iceberg_delete_data_uncached_read_count();

  UNSCOPED_INFO("manifest-walking delete reads for one query: " << (after - before));
  CHECK(after - before == 1);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - an explicitly pinned scan reaches the GPU",
                 "[integration][gpu_execution][iceberg]")
{
  // The narrowest statement of what works: a V2 table with a live positional delete, named by
  // snapshot, applying its deletes on the GPU.
  require_delete_files(v2_path, 1);
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(v2_path) + " ORDER BY count;",
                            kPositionalDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - conformance append-only matches pyiceberg",
                 "[integration][gpu_execution][iceberg]")
{
  // The baseline. No evolution, no deletes — so if this ever declines, the gate has begun
  // over-refusing. It is also the case that proves the delete-gate probe can read a table
  // with no version hint: pyiceberg's SqlCatalog writes none. Sirius does not force
  // unsafe_enable_version_guessing on its metadata connection, so the session must already be
  // able to read this table -- asserted rather than assumed.
  require_session_can_read(conf_append_only_path);
  REQUIRE(delete_file_count(conf_append_only_path) == 0);
  require_iceberg_execution(
    "SELECT id, name FROM " + pinned_scan(conf_append_only_path) + " ORDER BY id;", gpu_route::gpu);
}

// `y` was dropped and re-added under the same name, so it is a NEW field id (4). The one
// data file holds the ORIGINAL `y` at field id 3. Per the Iceberg spec the re-added column
// is absent from that file and must read NULL; pyiceberg and DuckDB both return NULL.
// Resolving by name finds the old column and returns 111/222 — with no error and no
// fallback, which is the worst failure mode available.
//
// The route is the assertion that matters here. Correct rows alone would also be produced by
// a GPU scan that had silently read the dropped column and happened to agree, so this pins
// that the table was REFUSED at plan time and DuckDB answered. When field-ID resolution
// lands, the expected route flips back to `gpu` and the rows stay exactly as written.
//
// ⚠️ Must stay UNPINNED, and it is the concrete cost of requiring `snapshot_from_id`. This
// table's single snapshot carries schema-id 0, where `y` is still field id 3; the drop and re-add
// that made it id 4 came later as metadata with no new snapshot. `snapshot_from_id` is Iceberg's
// TIME-TRAVEL selector, so it selects that snapshot's SCHEMA too:
//
//     iceberg_scan(path)                        current schema (2) -> y is field 4 -> NULL
//     iceberg_scan(path, snapshot_from_id = S)  snapshot schema (0) -> y is field 3 -> 111, 222
//
// Both answers are correct for the question asked, and DuckDB with Sirius unloaded returns 111/222
// for the pinned form too. A user told to pin to reach the GPU gets a DIFFERENT table, not the
// same one faster, so pinning here would change the expected rows.
//
// It declines on the unpinned-snapshot gate; the schema gate is covered by the two cases below.
TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - conformance dropped-and-re-added column reads NULL",
                 "[integration][gpu_execution][iceberg]")
{
  require_session_can_read(conf_drop_readd_path);
  require_iceberg_execution(
    "SELECT id, x, y FROM iceberg_scan('" + conf_drop_readd_path + "') ORDER BY id;",
    gpu_route::plan_fallback);
}

// Pinned, unlike the case above: these schema changes arrived WITH new snapshots, so both forms
// return the same rows and pinning is what keeps the schema gate reachable at all.
//
// A rename keeps the field id and changes the NAME, so the id space stays contiguous and the
// field-id gap test cannot see it. The older data file still spells the column `val` where the
// table now says `value`, so resolving by name finds nothing and throws — at SCAN time, which
// takes the runtime fallback and then deadlocks the connection. Declining at plan time is what
// makes this case runnable in-process at all.
TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - conformance renamed column declines at plan time",
                 "[integration][gpu_execution][iceberg]")
{
  require_session_can_read(conf_rename_col_path);
  require_iceberg_execution(
    "SELECT id, value FROM " + pinned_scan(conf_rename_col_path) + " ORDER BY id;",
    gpu_route::plan_fallback);
}

// An ADD keeps ids contiguous too (1,2,3 over three columns), so the gap test misses it for the
// same reason. `b` is simply absent from the first data file and must read NULL there. Same
// runtime-throw-then-deadlock path as the rename before the plan-time decline.
TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - conformance added column declines at plan time",
                 "[integration][gpu_execution][iceberg]")
{
  require_session_can_read(conf_add_column_path);
  require_iceberg_execution(
    "SELECT id, a, b FROM " + pinned_scan(conf_add_column_path) + " ORDER BY id;",
    gpu_route::plan_fallback);
}

//===----------------------------------------------------------------------===//
// V1 — append-only. Runs on the GPU: iceberg data files are parquet, and iceberg_scan binds
// them into the same MultiFileBindData read_parquet produces.
//===----------------------------------------------------------------------===//

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - V1 basic scan",
                 "[integration][gpu_execution][iceberg]")
{
  REQUIRE(delete_file_count(v1_path) == 0);  // append-only: nothing for the gate to catch
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(v1_path) + " ORDER BY count;",
                            gpu_route::gpu);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - data file with no field ids declines",
                 "[integration][gpu_execution][iceberg]")
{
  // A data file with NO parquet field ids plus the `schema.name-mapping.default` that makes it
  // legible: an `add_files` migration. A probe hole rather than a schema change -- the columns
  // still match, so a probe comparing only the pairs it could read reports it as fine. DuckDB and
  // a name-resolving GPU scan return the SAME rows here, so only the route catches it.
  auto const path =
    (get_project_root() / "test/cpp/integration/data/iceberg_v1_no_field_ids").string();
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(path) + " ORDER BY count;",
                            gpu_route::plan_fallback);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - promoted column type declines",
                 "[integration][gpu_execution][iceberg]")
{
  // `count` stored as INT32 while the table declares it `long`. Promotion keeps both the name and
  // the field id, so a pair comparison reports the file as current, and nothing throws -- the scan
  // just returns the file's narrower type. Only comparing the TYPE catches it.
  auto const path =
    (get_project_root() / "test/cpp/integration/data/iceberg_v1_promoted_type").string();
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(path) + " ORDER BY count;",
                            gpu_route::plan_fallback);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - reordered data file declines",
                 "[integration][gpu_execution][iceberg]")
{
  // The same two fields with the same ids and types, stored in the opposite physical order. This
  // is a VALID Iceberg table -- ids identify fields, so order carries no meaning, and DuckDB reads
  // it BY_FIELD_ID and returns the snapshot's order.
  //
  // The GPU path does not: for a full `SELECT *` build_scan_plan installs no reader projection, so
  // cuDF emits the file's order while the rest of the plan expects the bound snapshot's. `fruit`
  // and `count` are castable to each other's declared types, so nothing throws -- the values come
  // back swapped and converted. A (name, field id) membership comparison cannot see it; only
  // comparing the ORDER can.
  //
  // Declining a valid layout is the gate's fail-closed bias, and DuckDB returns the right rows.
  auto const path =
    (get_project_root() / "test/cpp/integration/data/iceberg_v1_reordered").string();
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(path) + " ORDER BY count;",
                            gpu_route::plan_fallback);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - unpinned scan declines",
                 "[integration][gpu_execution][iceberg]")
{
  // The plain spelling is what a user writes, and the one the GPU path cannot answer. It must
  // decline at PLAN time -- a runtime fallback poisons the connection.
  //
  // On the append-only table on purpose: with no deletes at all, the decline can only be about the
  // missing snapshot identity.
  require_iceberg_execution(
    "SELECT fruit, count FROM iceberg_scan('" + v1_path + "') ORDER BY count;", kUnpinnedRoute);

  // And the connection still answers a GPU query afterwards — the liveness half of the point.
  require_iceberg_execution("SELECT count(*) FROM " + pinned_scan(v1_path) + ";", gpu_route::gpu);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - allow_moved_paths declines",
                 "[integration][gpu_execution][iceberg]")
{
  // On an append-only table the path rewrite is harmless to the rows, so only the route shows it.
  require_iceberg_execution(
    "SELECT fruit, count FROM iceberg_scan('" + v1_path + "', snapshot_from_id = " +
      std::to_string(current_snapshot_id(v1_path)) + ", allow_moved_paths = true) ORDER BY count;",
    gpu_route::plan_fallback);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - V1 count(*)",
                 "[integration][gpu_execution][iceberg]")
{
  require_iceberg_execution("SELECT count(*) FROM " + pinned_scan(v1_path) + ";", gpu_route::gpu);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - V1 filter and order by",
                 "[integration][gpu_execution][iceberg]")
{
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(v1_path) + " WHERE count > 1 ORDER BY count;",
    gpu_route::gpu);
}

//===----------------------------------------------------------------------===//
// V2 positional deletes.
//
// Fixture: 5 rows apple/1..elderberry/5; the delete file removes positions 1 and 3
// (banana/2, date/4). Surviving: apple/1, cherry/3, elderberry/5.
//===----------------------------------------------------------------------===//

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - V2 positional deletes basic scan",
                 "[integration][gpu_execution][iceberg]")
{
  require_delete_files(v2_path, 1);
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(v2_path) + " ORDER BY count;",
                            kPositionalDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - V2 positional deletes count(*)",
                 "[integration][gpu_execution][iceberg]")
{
  require_delete_files(v2_path, 1);
  // 5 data rows minus 2 deleted. A count that reads row counts from parquet footers without
  // applying deletes returns 5 — this is the case that catches it.
  require_iceberg_execution("SELECT count(*) FROM " + pinned_scan(v2_path) + ";",
                            kPositionalDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - V2 positional deletes order by desc",
                 "[integration][gpu_execution][iceberg]")
{
  require_delete_files(v2_path, 1);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(v2_path) + " ORDER BY count DESC;",
    kPositionalDeleteRoute);
}

//===----------------------------------------------------------------------===//
// V2 equality deletes.
//
// Fixture: 5 rows apple/1..elderberry/5; an equality-delete file keyed on (fruit, count)
// removes banana/2 and date/4. Surviving: apple/1, cherry/3, elderberry/5.
//===----------------------------------------------------------------------===//

class GPUExecutionIcebergEqualityDeleteFixture : public GPUExecutionIcebergFixture {
 public:
  GPUExecutionIcebergEqualityDeleteFixture()
  {
    eq_path =
      (get_project_root() / "test/cpp/integration/data/iceberg_v2_equality_delete").string();
  }

  std::string eq_path;
};

TEST_CASE_METHOD(GPUExecutionIcebergEqualityDeleteFixture,
                 "gpu_execution iceberg - V2 equality deletes basic scan",
                 "[integration][gpu_execution][iceberg]")
{
  require_delete_files(eq_path, 1);
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(eq_path) + " ORDER BY count;",
                            kEqualityDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergEqualityDeleteFixture,
                 "gpu_execution iceberg - V2 equality deletes count(*)",
                 "[integration][gpu_execution][iceberg]")
{
  require_delete_files(eq_path, 1);
  require_iceberg_execution("SELECT count(*) FROM " + pinned_scan(eq_path) + ";",
                            kEqualityDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergEqualityDeleteFixture,
                 "gpu_execution iceberg - V2 equality deletes filter on surviving rows",
                 "[integration][gpu_execution][iceberg]")
{
  require_delete_files(eq_path, 1);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(eq_path) + " WHERE count > 2 ORDER BY count;",
    kEqualityDeleteRoute);
}

//===----------------------------------------------------------------------===//
// V2 equality delete edge cases: single-column keys, multiple delete files, everything
// deleted, equality combined with positional, and multiple data files.
//===----------------------------------------------------------------------===//

class GPUExecutionIcebergEqEdgeCaseFixture : public GPUExecutionIcebergFixture {
 public:
  GPUExecutionIcebergEqEdgeCaseFixture()
  {
    auto root       = get_project_root();
    single_col_path = (root / "test/cpp/integration/data/iceberg_v2_eq_single_col").string();
    multi_del_path  = (root / "test/cpp/integration/data/iceberg_v2_eq_multi_delete_file").string();
    all_del_path    = (root / "test/cpp/integration/data/iceberg_v2_eq_all_deleted").string();
    combined_path   = (root / "test/cpp/integration/data/iceberg_v2_eq_pos_combined").string();
    multi_data_path = (root / "test/cpp/integration/data/iceberg_v2_eq_multi_data_file").string();
  }

  std::string single_col_path;
  std::string multi_del_path;
  std::string all_del_path;
  std::string combined_path;
  std::string multi_data_path;
};

TEST_CASE_METHOD(GPUExecutionIcebergEqEdgeCaseFixture,
                 "gpu_execution iceberg - V2 equality deletes single-column key",
                 "[integration][gpu_execution][iceberg]")
{
  require_delete_files(single_col_path, 1);
  // The key is the fruit column alone (field_id=1); count is not part of it.
  // Delete entries "banana" and "date" must match on fruit regardless of count.
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(single_col_path) + " ORDER BY count;",
    kEqualityDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergEqEdgeCaseFixture,
                 "gpu_execution iceberg - V2 equality deletes multiple delete files",
                 "[integration][gpu_execution][iceberg]")
{
  // Two separate one-row equality-delete files — exercises the concatenate/deduplicate path.
  require_delete_files(multi_del_path, 2);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(multi_del_path) + " ORDER BY count;",
    kEqualityDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergEqEdgeCaseFixture,
                 "gpu_execution iceberg - V2 equality deletes all rows deleted",
                 "[integration][gpu_execution][iceberg]")
{
  // The delete file covers every data row — an all-false mask, which is where an
  // apply_boolean_mask that mishandles the empty result shows up.
  require_delete_files(all_del_path, 1);
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(all_del_path) + ";",
                            kEqualityDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergEqEdgeCaseFixture,
                 "gpu_execution iceberg - V2 equality and positional deletes combined",
                 "[integration][gpu_execution][iceberg]")
{
  // Both delete kinds against one table. The positional delete removes row 0 (apple/1). The
  // equality delete sits at the same sequence number as the data, so per the Iceberg spec it
  // does NOT apply — deletes only affect data files with strictly lower sequence numbers.
  // banana/2 surviving is the assertion that the sequence-number rule is honoured.
  require_delete_files(combined_path, 2);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(combined_path) + " ORDER BY count;",
    kEqualityDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergEqEdgeCaseFixture,
                 "gpu_execution iceberg - V2 equality deletes multiple data files",
                 "[integration][gpu_execution][iceberg]")
{
  // Two data files, one delete file removing a row from each: file 1 loses banana/2, file 2
  // loses elderberry/5. Exercises applying one delete set across per-file boundaries — the
  // same boundary the GPU path's one-file-per-batch coalescing rule exists to protect.
  require_delete_files(multi_data_path, 1);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(multi_data_path) + " ORDER BY count;",
    kEqualityDeleteRoute);
}

//===----------------------------------------------------------------------===//
// V3 deletion vectors (Puffin / Roaring bitmap).
//
// Same data and same surviving rows as the V2 positional fixture — deliberately, so the DV
// path is checked to produce identical output to the positional path.
//===----------------------------------------------------------------------===//

class GPUExecutionIcebergDVFixture : public GPUExecutionIcebergFixture {
 public:
  GPUExecutionIcebergDVFixture()
  {
    dv_path =
      (get_project_root() / "test/cpp/integration/data/iceberg_v3_deletion_vector").string();
  }

  std::string dv_path;
};

TEST_CASE_METHOD(GPUExecutionIcebergDVFixture,
                 "gpu_execution iceberg - V3 deletion vector basic scan",
                 "[integration][gpu_execution][iceberg]")
{
  require_delete_files(dv_path, 1);
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(dv_path) + " ORDER BY count;",
                            kDeletionVectorRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergDVFixture,
                 "gpu_execution iceberg - V3 deletion vector count(*)",
                 "[integration][gpu_execution][iceberg]")
{
  require_delete_files(dv_path, 1);
  require_iceberg_execution("SELECT count(*) FROM " + pinned_scan(dv_path) + ";",
                            kDeletionVectorRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergDVFixture,
                 "gpu_execution iceberg - V3 deletion vector filter",
                 "[integration][gpu_execution][iceberg]")
{
  // A filter over the surviving rows: a deleted row must not reappear because a predicate
  // happens to select it.
  require_delete_files(dv_path, 1);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(dv_path) + " WHERE count > 2 ORDER BY count;",
    kDeletionVectorRoute);
}

//===----------------------------------------------------------------------===//
// A deletion vector that a later commit REPLACED.
//
// Every other fixture here is a single snapshot in which every manifest entry is ADDED — the one
// shape where reading a manifest as a picture of the present is accidentally correct. A manifest
// is a log: compaction routinely rewrites a deletion vector, leaving the superseded entry at
// status DELETED beside its replacement, both naming the same data file.
//
// Hand-forged, because pyiceberg refuses to write delete files at all, so the conformance corpus
// cannot reach this. See data/iceberg_v3_dv_replaced/generate.py.
//===----------------------------------------------------------------------===//

class GPUExecutionIcebergDVReplacedFixture : public GPUExecutionIcebergFixture {
 public:
  GPUExecutionIcebergDVReplacedFixture()
  {
    dv_replaced_path =
      (get_project_root() / "test/cpp/integration/data/iceberg_v3_dv_replaced").string();
  }

  std::string dv_replaced_path;
};

TEST_CASE_METHOD(GPUExecutionIcebergDVReplacedFixture,
                 "gpu_execution iceberg - a retired deletion vector stays retired",
                 "[integration][gpu_execution][iceberg]")
{
  // Two PUFFIN entries for one data file: the live replacement (ADDED, deletes positions 1 and 3)
  // and the vector it replaced (DELETED, deletes 0, 1, 2 and 4). The retired one is written LAST,
  // so a reader that ignores `status` and lets the last entry win returns 1 row rather than 3 —
  // resurrecting rows the table deleted and dropping rows it kept.
  REQUIRE(delete_file_count(dv_replaced_path) == 2);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(dv_replaced_path) + " ORDER BY count;",
    kDeletionVectorRoute);
}

//===----------------------------------------------------------------------===//
// A manifest entry that points at ANOTHER data file's deletion vector.
//
// Hand-forged; see data/iceberg_v3_dv_misbound/generate.py.
//===----------------------------------------------------------------------===//

class GPUExecutionIcebergDVMisboundFixture : public GPUExecutionIcebergFixture {
 public:
  GPUExecutionIcebergDVMisboundFixture()
  {
    dv_misbound_path =
      (get_project_root() / "test/cpp/integration/data/iceberg_v3_dv_misbound").string();
  }

  std::string dv_misbound_path;
};

TEST_CASE_METHOD(
  GPUExecutionIcebergDVMisboundFixture,
  "gpu_execution iceberg - a deletion vector bound to the wrong data file is refused",
  "[integration][gpu_execution][iceberg]")
{
  // One Puffin holds a vector for each of two data files, and the two manifest entries carry each
  // other's offsets. Both blobs are well formed, so magic and CRC pass; both delete exactly one
  // position, so record_count passes as well. Only the footer descriptor's `referenced-data-file`
  // disagrees, which is why the spec requires content_offset/content_size_in_bytes to match it.
  //
  // The route IS the assertion. Delete files are read while the physical plan is built, so the
  // refusal lands at plan time and no GPU work begins. DuckDB's own reader does not check the
  // descriptor, so the rows below are the misbound answer it produces once Sirius hands the query
  // back: each file loses the position the OTHER file's vector names, so 'elderberry' and 'grape'
  // go instead of the 'banana' and 'jackfruit' the footer actually pairs. Eight rows either way —
  // the corruption never shows up in a count. Sirius declining is what keeps that answer from
  // being produced on the GPU and reported as ours.
  REQUIRE(delete_file_count(dv_misbound_path) == 2);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(dv_misbound_path) + " ORDER BY count;",
    gpu_route::plan_fallback);
}

//===----------------------------------------------------------------------===//
// Delete entries that a later commit RETIRED.
//
// Both tables still LIST a delete file; neither still applies one. See
// data/generate_retired_entry_fixtures.py.
//===----------------------------------------------------------------------===//

class GPUExecutionIcebergRetiredFixture : public GPUExecutionIcebergFixture {
 public:
  GPUExecutionIcebergRetiredFixture()
  {
    auto const root  = get_project_root() / "test/cpp/integration/data";
    pos_retired_path = (root / "iceberg_v2_pos_delete_retired").string();
    eq_retired_path  = (root / "iceberg_v2_eq_delete_retired").string();
  }

  std::string pos_retired_path;
  std::string eq_retired_path;
};

TEST_CASE_METHOD(GPUExecutionIcebergRetiredFixture,
                 "gpu_execution iceberg - a retired positional-delete file stops applying",
                 "[integration][gpu_execution][iceberg]")
{
  // The canary counts what the manifest LISTS, so it is 1 here even though nothing applies —
  // which is the whole point: the table looks like it has a delete file and must behave as
  // though it does not. A status-blind reader returns 3 of these 5 rows.
  require_delete_files(pos_retired_path, 1);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(pos_retired_path) + " ORDER BY count;",
    gpu_route::gpu);
}

TEST_CASE_METHOD(GPUExecutionIcebergRetiredFixture,
                 "gpu_execution iceberg - a retired equality-delete file does not refuse the GPU",
                 "[integration][gpu_execution][iceberg]")
{
  // Equality deletes are the one thing this path refuses outright, so the ROUTE is the
  // assertion. With the table's only equality-delete entry retired there is nothing to apply,
  // and the gate must not keep refusing over it. Getting this wrong costs performance rather
  // than correctness, which is exactly why the rows alone would never reveal it.
  require_delete_files(eq_retired_path, 1);
  require_iceberg_execution(
    "SELECT fruit, count FROM " + pinned_scan(eq_retired_path) + " ORDER BY count;",
    gpu_route::gpu);
}

//===----------------------------------------------------------------------===//
// Schema evolution, snapshot selection, and sequence numbers.
//===----------------------------------------------------------------------===//

//===----------------------------------------------------------------------===//
// Schema evolution: CPU-only on purpose. Read this before "fixing" it to run on the GPU.
//
// File 1 holds {fruit, count}; a later snapshot added `color`, so file 2 holds
// {fruit, count, color} and file 1's rows must read back with color NULL. DuckDB's reader does
// that; the GPU scan cannot synthesize a column absent from a data file, so it accepts the plan
// and then throws "Projected column 'color' not found in parquet file" at execute time.
//
// That is a RUNTIME fallback, and running one here would poison the whole suite. Any GPU query
// issued after a runtime fallback deadlocks in that connection — a pre-existing engine bug with
// nothing to do with iceberg. It reproduces in two statements and no iceberg at all:
//
//   SELECT * FROM read_parquet([file_without_color, file_with_color], union_by_name=true);
//   SELECT count(*) FROM read_parquet('nation.parquet');   -- hangs forever
//
// The engine's six runtime-fallback tests miss it because their fault injector
// (sirius_test_inject_transparent_gpu_error) throws one line BEFORE sirius_execute_query, so no
// pipeline or task is ever built and there is no dirty state to leave behind.
//
// So this case asserts what is actually dependable today: DuckDB returns the right rows for a
// schema-evolved iceberg table. It deliberately does NOT exercise the GPU path, because that
// path's observable behaviour right now is "correct answer, then a dead session" — not something
// worth pinning a test to.
//
// When the engine deadlock is fixed, run this through require_iceberg_execution with
// gpu_route::runtime_fallback. When the GPU learns to inject typed NULLs for missing columns
// (the MISSING entry kind in scan_plan), it becomes gpu_route::gpu.
//===----------------------------------------------------------------------===//

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - schema evolution added column",
                 "[integration][gpu_execution][iceberg]")
{
  auto path = (get_project_root() / "test/cpp/integration/data/iceberg_schema_evolution").string();
  REQUIRE(delete_file_count(path) == 0);  // append-only — deletes are not what this covers

  auto const before = sirius::test::get_transparent_execution_stats(*con);
  con->Query("SET gpu_execution = false;");
  auto result = con->Query("SELECT * FROM iceberg_scan('" + path + "') ORDER BY count;");
  con->Query("SET gpu_execution = true;");
  REQUIRE(result);
  if (result->HasError()) { UNSCOPED_INFO("iceberg_scan error: " << result->GetError()); }
  REQUIRE_FALSE(result->HasError());

  // No rebind, no fallback, no GPU execution: proves this really stayed off the GPU path, and
  // therefore that no runtime fallback happened to poison the cases that follow.
  sirius::test::require_transparent_execution_delta(
    before, sirius::test::get_transparent_execution_stats(*con), 0, 0, 0);

  std::vector<std::vector<std::string>> expected{{"apple", "1", "NULL"},
                                                 {"banana", "2", "NULL"},
                                                 {"cherry", "3", "NULL"},
                                                 {"date", "4", "brown"},
                                                 {"elderberry", "5", "purple"}};
  std::sort(expected.begin(), expected.end());
  REQUIRE(collect_rows(result->Cast<duckdb::MaterializedQueryResult>()) == expected);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - snapshot-aware deletes",
                 "[integration][gpu_execution][iceberg]")
{
  auto path = (get_project_root() / "test/cpp/integration/data/iceberg_snapshot_deletes").string();

  // The current snapshot carries a delete file, and the GPU path applies it itself — the route
  // assertion is what distinguishes that from quietly handing the table back to DuckDB, since
  // both return the same three rows.
  require_delete_files(path, 1);
  require_iceberg_execution("SELECT * FROM " + pinned_scan(path) + " ORDER BY count;",
                            kPositionalDeleteRoute);

  // Snapshot 1 predates the delete: all 5 rows, and — the point of this case — the gate must
  // probe the SAME snapshot the scan reads, so this one runs on the GPU. A gate that ignored
  // snapshot_from_id would see the table's delete file and needlessly decline.
  auto const snap1_args = ", snapshot_from_id = 9400000000000001";
  REQUIRE(delete_file_count(path, snap1_args) == 0);
  require_iceberg_execution(
    "SELECT * FROM iceberg_scan('" + path + "'" + snap1_args + ") ORDER BY count;", gpu_route::gpu);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - equality delete sequence numbers",
                 "[integration][gpu_execution][iceberg]")
{
  auto path = (get_project_root() / "test/cpp/integration/data/iceberg_eq_seq_number").string();
  // seq1 = [apple/1, banana/2, cherry/3], seq2 = delete(banana), seq3 = [banana/4, date/5].
  // The re-inserted banana/4 must survive: its data sequence number is above the delete's, and
  // an implementation that matches on key alone would wrongly drop it.
  require_delete_files(path, 1);
  require_iceberg_execution("SELECT fruit, count FROM " + pinned_scan(path) + " ORDER BY count;",
                            kEqualityDeleteRoute);
}

TEST_CASE_METHOD(GPUExecutionIcebergFixture,
                 "gpu_execution iceberg - deflate compressed manifests",
                 "[integration][gpu_execution][iceberg]")
{
  auto path = (get_project_root() / "test/cpp/integration/data/iceberg_deflate_manifest").string();
  REQUIRE(delete_file_count(path) == 0);
  require_iceberg_execution("SELECT * FROM " + pinned_scan(path) + " ORDER BY count;",
                            gpu_route::gpu);
}

//===----------------------------------------------------------------------===//
// Hive-partitioned parquet scan tests
//===----------------------------------------------------------------------===//

/**
 * @brief Test fixture for hive-partitioned parquet scans via gpu_execution.
 *
 * Dataset: test/cpp/integration/data/hive_partitioned/
 *   year=2024/month=01/data.parquet  (id=1, name=alice, amount=100.5)
 *   year=2024/month=02/data.parquet  (id=2, name=bob,   amount=200.75)
 *   year=2025/month=01/data.parquet  (id=3, name=charlie, amount=300.25)
 *
 * Partition columns (year, month) are NOT in the parquet files — their
 * values come from the directory paths.
 */
class HivePartitionDataset {
 public:
  HivePartitionDataset() : scratch{"hive_count_star"}, hive_dir{scratch.path()}
  {
    auto const source_root = get_project_root() / "test/cpp/integration/data/hive_partitioned";
    auto const y2024_m01   = source_root / "year=2024/month=01/data.parquet";
    auto const y2024_m02   = source_root / "year=2024/month=02/data.parquet";
    auto const y2025_m01   = source_root / "year=2025/month=01/data.parquet";
    REQUIRE(fs::exists(y2024_m01));
    REQUIRE(fs::exists(y2024_m02));
    REQUIRE(fs::exists(y2025_m01));

    fs::create_directories(hive_dir / "h/year=2024/month=01");
    fs::create_directories(hive_dir / "h/year=2024/month=02");
    fs::create_directories(hive_dir / "h/year=2025/month=01");
    fs::create_directories(hive_dir / "flat");

    fs::copy_file(y2024_m01,
                  hive_dir / "h/year=2024/month=01/data.parquet",
                  fs::copy_options::overwrite_existing);
    fs::copy_file(y2024_m02,
                  hive_dir / "h/year=2024/month=02/data.parquet",
                  fs::copy_options::overwrite_existing);
    fs::copy_file(y2025_m01,
                  hive_dir / "h/year=2025/month=01/data.parquet",
                  fs::copy_options::overwrite_existing);

    fs::copy_file(
      y2024_m01, hive_dir / "flat/part_2024_01.parquet", fs::copy_options::overwrite_existing);
    fs::copy_file(
      y2024_m02, hive_dir / "flat/part_2024_02.parquet", fs::copy_options::overwrite_existing);
    fs::copy_file(
      y2025_m01, hive_dir / "flat/part_2025_01.parquet", fs::copy_options::overwrite_existing);

    hive_path   = (hive_dir / "h/year=*/month=*/*.parquet").string();
    flat_path   = (hive_dir / "flat/*.parquet").string();
    config_path = hive_dir / "watchdog_integration.yaml";
    write_watchdog_config(config_path);
    is_hive_available = true;
  }

  struct watchdog_result {
    bool timed_out{false};
    std::string error;
  };

  std::string hive_scan() const
  {
    return "read_parquet(" + sirius::test::sql_literal(hive_path) + ", hive_partitioning=true)";
  }

  std::string flat_scan() const
  {
    return "read_parquet(" + sirius::test::sql_literal(flat_path) + ")";
  }

  static void write_watchdog_result(fs::path const& path, watchdog_result const& result)
  {
    std::ofstream out(path);
    out << result.error;
  }

  static void write_watchdog_config(fs::path const& path)
  {
    std::ofstream out(path);
    out << R"(sirius:
  topology:
    num_gpus: 1
  memory:
    gpu:
      usage_limit_fraction: 0.2
      reservation_limit_fraction: 1.0
    host:
      capacity_bytes: 8000000000
      initial_number_pools: 4
      pool_size: 512
      block_size: 1048576
  executor:
    pipeline:
      num_threads: 2
    task_creator:
      num_threads: 1
    downgrade:
      num_threads: 1
      monitor_period: 10ms
  operator_params:
    scan_task_batch_size: 100000000
    max_sort_partition_bytes: 0
    hash_partition_bytes: 100000000
    concat_batch_bytes: 100000000
    max_build_hash_table_bytes: 90000000
)";
  }

  static watchdog_result read_watchdog_result(fs::path const& path)
  {
    watchdog_result result;
    std::ifstream in(path);
    if (!in) {
      result.error = "watchdog child did not write a result file";
      return result;
    }
    result.error =
      std::string{std::istreambuf_iterator<char>(in), std::istreambuf_iterator<char>()};
    return result;
  }

  watchdog_result run_gpu_query_with_watchdog(std::string const& query,
                                              std::chrono::seconds timeout)
  {
    static std::atomic<std::uint64_t> next_child_id{0};
    auto const output_path =
      hive_dir / ("watchdog_result_" + std::to_string(next_child_id.fetch_add(1)) + ".txt");
    std::vector<std::string> child_arguments{"sirius_unittest",
                                             "gpu_execution hive partition watchdog child runner"};
    std::vector<char*> child_argv;
    child_argv.reserve(child_arguments.size() + 1);
    for (auto& argument : child_arguments) {
      child_argv.push_back(argument.data());
    }
    child_argv.push_back(nullptr);

    sirius::test::child_process_environment child_environment{
      {{"SIRIUS_HIVE_WATCHDOG_QUERY", query},
       {"SIRIUS_HIVE_WATCHDOG_OUTPUT", output_path.string()},
       {"SIRIUS_HIVE_WATCHDOG_CONFIG", config_path.string()},
       {"SIRIUS_CONFIG_FILE", config_path.string()}}};

    pid_t pid{};
    auto const spawn_result = ::posix_spawn(
      &pid, "/proc/self/exe", nullptr, nullptr, child_argv.data(), child_environment.data());
    REQUIRE(spawn_result == 0);

    int status      = 0;
    auto const stop = std::chrono::steady_clock::now() + timeout;
    while (std::chrono::steady_clock::now() < stop) {
      auto const waited = ::waitpid(pid, &status, WNOHANG);
      if (waited == pid) {
        auto result = read_watchdog_result(output_path);
        if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
          result.error = result.error.empty() ? "watchdog child exited abnormally" : result.error;
        }
        return result;
      }
      if (waited < 0) {
        watchdog_result result;
        result.error = "waitpid failed while waiting for watchdog child";
        return result;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds{50});
    }

    (void)::kill(pid, SIGKILL);
    (void)::waitpid(pid, &status, 0);
    watchdog_result out;
    out.timed_out = true;
    out.error     = "query timed out after " + std::to_string(timeout.count()) + " seconds";
    return out;
  }

  void require_gpu_execution(std::string const& query)
  {
    auto result = run_gpu_query_with_watchdog(query, std::chrono::seconds{60});
    INFO(query);
    INFO(result.error);
    REQUIRE_FALSE(result.timed_out);
    REQUIRE(result.error.empty());
  }

  sirius::test::scratch_dir scratch;
  fs::path hive_dir;
  fs::path config_path;
  std::string hive_path;
  std::string flat_path;
  bool is_hive_available = false;  // No extension is needed for DuckDB hive partition discovery.
};

class EscapedHivePartitionDataset {
 public:
  EscapedHivePartitionDataset() : scratch{"hive_unescape"}, hive_dir{scratch.path()}
  {
    auto const source =
      get_project_root() /
      "test/cpp/integration/data/hive_partitioned/year=2024/month=01/data.parquet";
    REQUIRE(fs::exists(source));

    copy_partition(source, hive_dir / "space/city=New%20York/data.parquet");
    copy_partition(source, hive_dir / "slash/city=Path%2FTeam/data.parquet");
    copy_partition(source, hive_dir / "percent/city=100%25/data.parquet");
    copy_partition(source, hive_dir / "multiple/city=Los%20Angeles%2FWest/data.parquet");
    copy_partition(source, hive_dir / "two_columns/city=New%20York/dept=R%26D/data.parquet");
    copy_partition(source, hive_dir / "plain/city=Boston/data.parquet");
  }

  std::string partition_scan(fs::path const& relative_glob) const
  {
    return "read_parquet(" + sirius::test::sql_literal((hive_dir / relative_glob).string()) +
           ", hive_partitioning=true)";
  }

 private:
  static void copy_partition(fs::path const& source, fs::path const& target)
  {
    fs::create_directories(target.parent_path());
    fs::copy_file(source, target, fs::copy_options::overwrite_existing);
  }

  sirius::test::scratch_dir scratch;
  fs::path hive_dir;
};

class GPUExecutionHivePartitionFixture : public MultiFormatFixtureBase,
                                         public HivePartitionDataset {};

class GPUExecutionEscapedHivePartitionFixture : public MultiFormatFixtureBase,
                                                public EscapedHivePartitionDataset {};

TEST_CASE("gpu_execution hive partition watchdog child runner",
          "[.][gpu_execution][hive_partition][watchdog_child]")
{
  auto const* query      = std::getenv("SIRIUS_HIVE_WATCHDOG_QUERY");
  auto const* output_raw = std::getenv("SIRIUS_HIVE_WATCHDOG_OUTPUT");
  if (query == nullptr || output_raw == nullptr) { return; }

  HivePartitionDataset::watchdog_result out;
  try {
    auto const* config_raw = std::getenv("SIRIUS_HIVE_WATCHDOG_CONFIG");
    if (config_raw == nullptr) { out.error = "watchdog child missing config path"; }

    std::unique_ptr<sirius_config_env_guard> config_guard;
    std::unique_ptr<duckdb::DuckDB> db;
    std::unique_ptr<duckdb::Connection> con;
    if (out.error.empty()) {
      // Shared test-environment startup resets these values after exec, so restore the
      // watchdog's isolated-context environment before opening its database.
      config_guard = std::make_unique<sirius_config_env_guard>(config_raw);
      ::unsetenv("SIRIUS_DISABLE");
      db  = std::make_unique<duckdb::DuckDB>(nullptr);
      con = std::make_unique<duckdb::Connection>(*db);
    }

    auto set_gpu = out.error.empty() ? con->Query("SET gpu_execution = true;") : nullptr;
    if (!set_gpu) {
      out.error = "SET gpu_execution returned nullptr";
    } else if (set_gpu->HasError()) {
      out.error = set_gpu->GetError();
    }

    auto before_gpu_stats = out.error.empty()
                              ? sirius::test::get_transparent_execution_stats(*con)
                              : duckdb::SiriusContext::transparent_execution_stats{};
    if (out.error.empty()) {
      auto result = con->Query(query);
      if (!result) {
        out.error = "query returned nullptr";
      } else if (result->HasError()) {
        out.error = result->GetError();
      }
    }

    if (out.error.empty()) {
      auto after_gpu_stats = sirius::test::get_transparent_execution_stats(*con);
      if (after_gpu_stats.successful_rebinds != before_gpu_stats.successful_rebinds + 1 ||
          after_gpu_stats.fallbacks != before_gpu_stats.fallbacks ||
          after_gpu_stats.executions != before_gpu_stats.executions + 1) {
        out.error = "transparent execution stats delta mismatch";
      }
    }
  } catch (std::exception const& e) {
    out.error = e.what();
  } catch (...) {
    out.error = "query threw an unknown exception";
  }

  HivePartitionDataset::write_watchdog_result(output_raw, out);
  REQUIRE(out.error.empty());
}

TEST_CASE_METHOD(HivePartitionDataset,
                 "gpu_execution hive partition count star completes under watchdog",
                 "[gpu_execution][hive_partition][count_star][watchdog]")
{
  SECTION("hive count star") { require_gpu_execution("SELECT count(*) FROM " + hive_scan()); }

  SECTION("flat count star control")
  {
    require_gpu_execution("SELECT count(*) FROM " + flat_scan());
  }

  SECTION("partition-filtered count star")
  {
    require_gpu_execution("SELECT count(*) FROM " + hive_scan() + " WHERE year = 2024");
  }

  SECTION("partition column select")
  {
    require_gpu_execution("SELECT year FROM " + hive_scan() + " ORDER BY year");
  }
}

TEST_CASE_METHOD(GPUExecutionEscapedHivePartitionFixture,
                 "gpu_execution hive partition - unescapes varchar partition values",
                 "[integration][gpu_execution][hive_partition][unescape]")
{
  SECTION("projects a space-escaped partition column")
  {
    MultiFormatFixtureBase::require_gpu_execution("SELECT city FROM " +
                                                  partition_scan("space/city=*/*.parquet"));
  }

  SECTION("projects data and a space-escaped partition column")
  {
    MultiFormatFixtureBase::require_gpu_execution("SELECT id, city FROM " +
                                                  partition_scan("space/city=*/*.parquet"));
  }

  SECTION("unescapes a slash")
  {
    MultiFormatFixtureBase::require_gpu_execution("SELECT city FROM " +
                                                  partition_scan("slash/city=*/*.parquet"));
  }

  SECTION("unescapes a percent sign")
  {
    MultiFormatFixtureBase::require_gpu_execution("SELECT city FROM " +
                                                  partition_scan("percent/city=*/*.parquet"));
  }

  SECTION("unescapes multiple sequences in one value")
  {
    MultiFormatFixtureBase::require_gpu_execution("SELECT city FROM " +
                                                  partition_scan("multiple/city=*/*.parquet"));
  }

  SECTION("unescapes every partition column")
  {
    MultiFormatFixtureBase::require_gpu_execution(
      "SELECT id, city, dept FROM " + partition_scan("two_columns/city=*/dept=*/*.parquet"));
  }

  SECTION("preserves an unescaped partition value")
  {
    MultiFormatFixtureBase::require_gpu_execution("SELECT id, city FROM " +
                                                  partition_scan("plain/city=*/*.parquet"));
  }
}

TEST_CASE_METHOD(GPUExecutionHivePartitionFixture,
                 "gpu_execution hive partition - basic scan with partition columns",
                 "[.][integration][gpu_execution][hive_partition]")
{
  if (!is_hive_available) {
    WARN("hive extension not available — skipping");
    return;
  }
  MultiFormatFixtureBase::require_gpu_execution("SELECT * FROM read_parquet('" + hive_path +
                                                "', hive_partitioning=true) ORDER BY id");
}

TEST_CASE_METHOD(GPUExecutionHivePartitionFixture,
                 "gpu_execution hive partition - filter on data column",
                 "[.][integration][gpu_execution][hive_partition]")
{
  if (!is_hive_available) {
    WARN("hive extension not available — skipping");
    return;
  }
  MultiFormatFixtureBase::require_gpu_execution(
    "SELECT * FROM read_parquet('" + hive_path +
    "', hive_partitioning=true) WHERE id >= 2 ORDER BY id");
}

TEST_CASE_METHOD(GPUExecutionHivePartitionFixture,
                 "gpu_execution hive partition - filter on partition column",
                 "[.][integration][gpu_execution][hive_partition]")
{
  if (!is_hive_available) {
    WARN("hive extension not available — skipping");
    return;
  }
  MultiFormatFixtureBase::require_gpu_execution(
    "SELECT id, name, year FROM read_parquet('" + hive_path +
    "', hive_partitioning=true) WHERE year = 2024 ORDER BY id");
}

TEST_CASE_METHOD(GPUExecutionHivePartitionFixture,
                 "gpu_execution hive partition - group by partition column",
                 "[.][integration][gpu_execution][hive_partition]")
{
  if (!is_hive_available) {
    WARN("hive extension not available — skipping");
    return;
  }
  MultiFormatFixtureBase::require_gpu_execution(
    "SELECT year, SUM(amount) as total FROM read_parquet('" + hive_path +
    "', hive_partitioning=true) GROUP BY year ORDER BY year");
}

TEST_CASE_METHOD(GPUExecutionHivePartitionFixture,
                 "gpu_execution hive partition - reversed column order",
                 "[.][integration][gpu_execution][hive_partition]")
{
  if (!is_hive_available) {
    WARN("hive extension not available — skipping");
    return;
  }
  MultiFormatFixtureBase::require_gpu_execution(
    "SELECT year, month, amount, name, id FROM read_parquet('" + hive_path +
    "', hive_partitioning=true) ORDER BY id");
}

TEST_CASE_METHOD(GPUExecutionHivePartitionFixture,
                 "gpu_execution hive partition - aggregation on data column",
                 "[.][integration][gpu_execution][hive_partition]")
{
  if (!is_hive_available) {
    WARN("hive extension not available — skipping");
    return;
  }
  MultiFormatFixtureBase::require_gpu_execution("SELECT SUM(amount) as total FROM read_parquet('" +
                                                hive_path + "', hive_partitioning=true)");
}
