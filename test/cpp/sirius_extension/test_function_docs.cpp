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

#include "catch.hpp"
#include "util/env_guard.hpp"

#include <duckdb.hpp>

#include <regex>
#include <set>
#include <string>
#include <vector>

namespace {

std::vector<std::string> strings(duckdb::Value const& list)
{
  std::vector<std::string> result;
  if (list.IsNull()) { return result; }
  for (auto const& child : duckdb::ListValue::GetChildren(list)) {
    result.push_back(child.ToString());
  }
  return result;
}

}  // namespace

TEST_CASE("Every Sirius table function is documented",
          "[sirius_extension][function_docs][isolated_context]")
{
  // Registration does not need the GPU runtime.
  sirius::util::env_guard const disable_sirius{"SIRIUS_DISABLE", "1"};
  duckdb::DuckDB db(nullptr);
  duckdb::Connection con(db);

  auto result = con.Query(
    "SELECT function_name, description, examples, categories, parameters FROM duckdb_functions() "
    "WHERE extension_name = 'sirius' AND function_type = 'table'");
  REQUIRE_FALSE(result->HasError());

  // One row per overload: a function missing here was registered without its extension name.
  std::multiset<std::string> const expected{"gpu_execution",
                                            "pin_table",
                                            "pin_table",
                                            "profiler_start",
                                            "profiler_stop",
                                            "reset_sirius_cache",
                                            "sirius_analyze_parquet",
                                            "sirius_create_ann_index",
                                            "sirius_drop_ann_index",
                                            "sirius_knn_search",
                                            "sirius_read_parquet",
                                            "sirius_set_query_label",
                                            "sirius_set_session_label",
                                            "sirius_stream_source",
                                            "unpin_table"};
  std::multiset<std::string> names;
  std::set<std::string> pin_table_descriptions;
  std::regex const unnamed_parameter{"col[0-9]+"};
  auto const& rows = result->Collection();
  for (duckdb::idx_t row = 0; row < rows.Count(); row++) {
    auto const name = rows.GetValue(0, row).ToString();
    CAPTURE(name);
    names.insert(name);

    auto const description = rows.GetValue(1, row);
    REQUIRE_FALSE(description.IsNull());
    auto const text = description.ToString();
    CHECK_FALSE(text.empty());
    CHECK(text.find('\n') == std::string::npos);
    if (name == "pin_table") { pin_table_descriptions.insert(text); }

    CHECK_FALSE(strings(rows.GetValue(2, row)).empty());
    CHECK_FALSE(strings(rows.GetValue(3, row)).empty());
    for (auto const& parameter : strings(rows.GetValue(4, row))) {
      CAPTURE(parameter);
      CHECK_FALSE(std::regex_match(parameter, unnamed_parameter));
    }
  }
  CHECK(names == expected);
  // Each pin_table overload has its own description.
  CHECK(pin_table_descriptions.size() == 2);
}
