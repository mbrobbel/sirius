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

#pragma once

#include "duckdb/function/function_set.hpp"
#include "duckdb/main/extension/extension_loader.hpp"
#include "duckdb/parser/parsed_data/create_table_function_info.hpp"

#include <string>
#include <utility>

namespace sirius {

/// What duckdb_functions() and the CLI's .manual show for one overload.
struct function_doc {
  /// One line of plain text whose first sentence stands on its own.
  std::string description;
  /// Complete SQL statements.
  duckdb::vector<std::string> examples;
  /// Short lowercase topics, such as "cache".
  duckdb::vector<std::string> categories;
};

/// A table function's overloads, each with its documentation.
class documented_table_function {
 public:
  documented_table_function(duckdb::TableFunction function, function_doc doc)
    : functions_(function.name)
  {
    add_overload(std::move(function), std::move(doc));
  }

  /// duckdb_functions() matches a doc to its overload by the positional parameter types.
  documented_table_function& add_overload(duckdb::TableFunction function, function_doc doc)
  {
    duckdb::FunctionDescription description;
    auto const& signature = function.GetSignature();
    for (duckdb::idx_t i = 0; i < signature.GetPositionalParameterCount(); i++) {
      auto const& parameter = signature.GetParameter(i);
      description.parameter_types.push_back(parameter.GetType());
      description.parameter_names.push_back(parameter.GetName().GetIdentifierName());
    }
    description.description = std::move(doc.description);
    description.examples    = std::move(doc.examples);
    description.categories  = std::move(doc.categories);
    descriptions_.push_back(std::move(description));
    functions_.AddFunction(std::move(function));
    return *this;
  }

  /// The catalog entry for every overload; creating it fails if the name is taken.
  [[nodiscard]] duckdb::CreateTableFunctionInfo create_info() const
  {
    duckdb::CreateTableFunctionInfo info(functions_);
    info.descriptions = descriptions_;
    return info;
  }

 private:
  duckdb::TableFunctionSet functions_;
  duckdb::vector<duckdb::FunctionDescription> descriptions_;
};

/// Registers table functions as entries of the extension being loaded, which is what
/// duckdb_functions().extension_name reports.
class table_function_registrar {
 public:
  explicit table_function_registrar(duckdb::ExtensionLoader& loader) : loader_(loader) {}

  /// Registers every overload at once: adding overloads to an existing entry would drop its
  /// extension name.
  void add(documented_table_function const& function) const
  {
    loader_.RegisterFunction(function.create_info());
  }

 private:
  duckdb::ExtensionLoader& loader_;
};

}  // namespace sirius
