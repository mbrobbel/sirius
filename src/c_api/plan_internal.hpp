// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#pragma once
#include <sirius-c/plan/plan.h>

#include <atomic>
#include <memory>
#include <string>
#include <vector>

namespace sirius::c_api {
struct field {
  std::string name;
  bool nullable;
};
struct comparison {
  size_t column;
  sirius_comparison operation;
  int64_t literal;
};
// A relation owns a complete immutable scan/filter description. Each factory
// allocates a new description, which also gives its columns a distinct scope.
struct relation {
  std::string path;
  std::vector<field> fields;
  std::vector<comparison> filters;
};
template <class T>
void retain(T* value) noexcept
{
  if (value) { value->references.fetch_add(1, std::memory_order_relaxed); }
}
template <class T>
void release(T* value) noexcept
{
  if (value && value->references.fetch_sub(1, std::memory_order_acq_rel) == 1) { delete value; }
}
}  // namespace sirius::c_api

struct sirius_relation {
  explicit sirius_relation(std::shared_ptr<const sirius::c_api::relation> value)
    : value(std::move(value))
  {
  }
  std::atomic<size_t> references{1};
  const std::shared_ptr<const sirius::c_api::relation> value;
};
struct sirius_expression {
  sirius_expression(std::shared_ptr<const sirius::c_api::relation> scope,
                    sirius::c_api::comparison value)
    : scope(std::move(scope)), value(value)
  {
  }
  std::atomic<size_t> references{1};
  const std::shared_ptr<const sirius::c_api::relation> scope;
  const sirius::c_api::comparison value;
};
struct sirius_plan {
  explicit sirius_plan(std::shared_ptr<const sirius::c_api::relation> root) : root(std::move(root))
  {
  }
  std::atomic<size_t> references{1};
  const std::shared_ptr<const sirius::c_api::relation> root;
};
