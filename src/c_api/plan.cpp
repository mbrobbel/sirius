// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#include "c_api/error_internal.hpp"
#include "c_api/plan_internal.hpp"
#include "utf8proc_wrapper.hpp"

#include <limits>
#include <set>
#include <stdexcept>

namespace {
std::string text(const char* bytes, size_t size)
{
  if (!bytes || !size) { throw std::invalid_argument("Expected nonempty string"); }
  std::string result(bytes, size);
  if (result.find('\0') != std::string::npos) {
    throw std::invalid_argument("Embedded NUL in string");
  }
  return result;
}
}  // namespace

sirius_status sirius_relation_parquet(const char* path,
                                      size_t path_size,
                                      const sirius_i64_field* fields,
                                      size_t count,
                                      sirius_relation** out,
                                      sirius_error** error) noexcept
{
  if (out) { *out = nullptr; }
  return sirius::c_api::invoke(
    error,
    [&] {
      if (!out || !fields || !count || count > std::numeric_limits<uint32_t>::max()) {
        return sirius::c_api::fail(
          SIRIUS_INVALID_ARGUMENT, "Expected schema and output slot", error);
      }
      auto value  = std::make_shared<sirius::c_api::relation>();
      value->path = text(path, path_size);
      if (value->path.find_first_of("*?[]") != std::string::npos ||
          value->path.find("://") != std::string::npos) {
        throw std::invalid_argument("Expected a local file path without glob characters");
      }
      std::set<std::string> names;
      for (size_t i = 0; i < count; ++i) {
        auto name = text(fields[i].name, fields[i].name_size);
        if (!duckdb::Utf8Proc::IsValid(name.data(), name.size()) || !names.insert(name).second ||
            fields[i].nullable != 1) {
          throw std::invalid_argument("Invalid or duplicate schema field");
        }
        value->fields.push_back({std::move(name), fields[i].nullable != 0});
      }
      *out = new sirius_relation(std::move(value));
      return sirius_status{SIRIUS_SUCCESS};
    },
    SIRIUS_INVALID_PLAN);
}

sirius_status sirius_expression_compare_i64(const sirius_relation* input,
                                            size_t column,
                                            sirius_comparison comparison,
                                            int64_t literal,
                                            sirius_expression** out,
                                            sirius_error** error) noexcept
{
  if (out) { *out = nullptr; }
  return sirius::c_api::invoke(error, [&] {
    if (!input || !out) {
      return sirius::c_api::fail(
        SIRIUS_INVALID_ARGUMENT, "Expected relation and output slot", error);
    }
    if (column >= input->value->fields.size() || comparison > SIRIUS_GREATER_EQUAL) {
      return sirius::c_api::fail(SIRIUS_INVALID_PLAN, "Invalid column or comparison", error);
    }
    *out = new sirius_expression(input->value, {column, comparison, literal});
    return sirius_status{SIRIUS_SUCCESS};
  });
}

sirius_status sirius_relation_filter(const sirius_relation* input,
                                     const sirius_expression* predicate,
                                     sirius_relation** out,
                                     sirius_error** error) noexcept
{
  if (out) { *out = nullptr; }
  return sirius::c_api::invoke(error, [&] {
    if (!input || !predicate || !out) {
      return sirius::c_api::fail(
        SIRIUS_INVALID_ARGUMENT, "Expected relation, predicate and output slot", error);
    }
    if (input->value != predicate->scope) {
      return sirius::c_api::fail(
        SIRIUS_INVALID_PLAN, "Predicate belongs to a different relation", error);
    }
    auto value = std::make_shared<sirius::c_api::relation>(*input->value);
    value->filters.push_back(predicate->value);
    *out = new sirius_relation(std::move(value));
    return sirius_status{SIRIUS_SUCCESS};
  });
}

sirius_status sirius_plan_from_root(const sirius_relation* root,
                                    sirius_plan** out,
                                    sirius_error** error) noexcept
{
  if (out) { *out = nullptr; }
  return sirius::c_api::invoke(error, [&] {
    if (!root || !out) {
      return sirius::c_api::fail(SIRIUS_INVALID_ARGUMENT, "Expected root and output slot", error);
    }
    *out = new sirius_plan(root->value);
    return sirius_status{SIRIUS_SUCCESS};
  });
}
void sirius_relation_retain(sirius_relation* value) noexcept { sirius::c_api::retain(value); }
void sirius_relation_release(sirius_relation* value) noexcept { sirius::c_api::release(value); }
void sirius_expression_retain(sirius_expression* value) noexcept { sirius::c_api::retain(value); }
void sirius_expression_release(sirius_expression* value) noexcept { sirius::c_api::release(value); }
void sirius_plan_retain(sirius_plan* value) noexcept { sirius::c_api::retain(value); }
void sirius_plan_release(sirius_plan* value) noexcept { sirius::c_api::release(value); }
