// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#include "sirius_extension.hpp"
#include "util/env_guard.hpp"

#include <catch.hpp>
#include <duckdb.hpp>
#include <duckdb/main/extension_helper.hpp>

#include <map>
#include <optional>
#include <set>
#include <string>

namespace {

/// Name and description of every setting one duckdb_settings() call lists.
std::map<std::string, std::string> list_settings(duckdb::Connection& con,
                                                 std::string const& arguments)
{
  auto result = con.Query("SELECT name, description FROM duckdb_settings(" + arguments + ")");
  REQUIRE(result != nullptr);
  INFO((result->HasError() ? result->GetError() : arguments));
  REQUIRE_FALSE(result->HasError());
  auto& rows = result->Collection();
  std::map<std::string, std::string> settings;
  for (duckdb::idx_t row = 0; row < rows.Count(); ++row) {
    auto const description = rows.GetValue(1, row);
    settings.emplace(rows.GetValue(0, row).ToString(),
                     description.IsNull() ? std::string{} : description.ToString());
  }
  return settings;
}

std::set<std::string> names_in(std::map<std::string, std::string> const& settings,
                               std::set<std::string> const& names)
{
  std::set<std::string> result;
  for (auto const& name : names) {
    if (settings.contains(name)) { result.insert(name); }
  }
  return result;
}

}  // namespace

TEST_CASE("Sirius settings are described and hide low-level tuning knobs",
          "[sirius][config][settings][isolated_context]")
{
  sirius::util::env_guard const disable_sirius{"SIRIUS_DISABLE", "1"};
  sirius::util::env_guard const no_test_options{"SIRIUS_ENABLE_TEST_OPTIONS", std::nullopt};

  duckdb::DBConfig config;
  config.options.load_extensions = false;
  duckdb::DuckDB db(nullptr, &config);
  duckdb::ExtensionHelper::LoadExtension(db, "parquet");
  duckdb::ExtensionHelper::LoadExtension(db, "core_functions");
  duckdb::Connection con(db);

  auto const before = list_settings(con, "include_all := true");
  db.LoadStaticExtension<duckdb::SiriusExtension>();
  auto const all = list_settings(con, "include_all := true");

  std::set<std::string> sirius;
  for (auto const& [name, description] : all) {
    if (!before.contains(name)) { sirius.insert(name); }
  }
  REQUIRE(sirius.contains("gpu_execution"));

  for (auto const& name : sirius) {
    CAPTURE(name);
    auto const& description = all.at(name);
    CHECK_FALSE(description.empty());
    CHECK(description.ends_with('.'));
    CHECK_FALSE(description.starts_with("TEST ONLY"));
  }

  auto const listed     = names_in(list_settings(con, ""), sirius);
  auto const debug      = names_in(list_settings(con, "debug := true"), sirius);
  auto const deprecated = names_in(list_settings(con, "deprecated := true"), sirius);

  // Each setting is listed by exactly one of the three filters.
  auto filtered = listed;
  filtered.insert(debug.begin(), debug.end());
  filtered.insert(deprecated.begin(), deprecated.end());
  CHECK(filtered == sirius);
  CHECK(listed.size() + debug.size() + deprecated.size() == sirius.size());

  std::set<std::string> const expected_debug{"dynamic_filter_domain_coverage_threshold",
                                             "dynamic_filter_inlist_max_l2_fraction",
                                             "dynamic_filter_keep_threshold",
                                             "enable_regex_jit_impl",
                                             "enable_runtime_size_estimation",
                                             "like_swar_fastpath",
                                             "mark_join_build_switch_ratio",
                                             "max_build_hash_table_bytes",
                                             "max_build_probe_resident_bytes",
                                             "sort_sample_bytes",
                                             "use_hw_decompression"};
  CHECK(debug == expected_debug);
  CHECK(deprecated == std::set<std::string>{"expression_executor_strategy"});
  CHECK(all.at("expression_executor_strategy") ==
        "Deprecated setting. Use expression_evaluator_strategy instead.");

  // Hidden settings stay settable.
  auto set = con.Query("SET like_swar_fastpath = false");
  REQUIRE(set != nullptr);
  INFO((set->HasError() ? set->GetError() : "set"));
  REQUIRE_FALSE(set->HasError());
  auto value = con.Query("SELECT current_setting('like_swar_fastpath')");
  REQUIRE(value != nullptr);
  REQUIRE_FALSE(value->HasError());
  CHECK_FALSE(value->Collection().GetValue(0, 0).GetValue<bool>());
}
