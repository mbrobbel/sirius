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

#include "transparent/sirius_optimizer_extension.hpp"

#include "sirius_context.hpp"

#include <duckdb/common/enums/optimizer_type.hpp>
#include <duckdb/common/exception.hpp>
#include <duckdb/common/types/value.hpp>
#include <duckdb/main/client_context.hpp>
#include <duckdb/main/config.hpp>
#include <duckdb/planner/expression/bound_columnref_expression.hpp>
#include <duckdb/planner/expression/bound_conjunction_expression.hpp>
#include <duckdb/planner/expression_iterator.hpp>
#include <duckdb/planner/operator/logical_filter.hpp>
#include <duckdb/planner/operator/logical_get.hpp>
#include <duckdb/storage/table/row_group_reorderer.hpp>
#include <log/logging.hpp>
#include <util/duckdb_error_message.hpp>

#include <chrono>
#include <cstddef>
#include <exception>
#include <map>
#include <unordered_set>
#include <utility>
#include <vector>

namespace sirius::transparent {

namespace {

bool gpu_execution_enabled(const duckdb::ClientContext& context)
{
  duckdb::Value setting;
  auto lookup_result = context.TryGetCurrentSetting("gpu_execution", setting);
  return lookup_result && !setting.IsNull() && setting.GetValue<bool>();
}

//===--------------------------------------------------------------------===//
// Join-dependent filter derivation (see sirius_pre_optimizer_hook)
//===--------------------------------------------------------------------===//

/// Guards against quadratic blow-up: an N-branch disjunction derives one N-child OR per table.
constexpr std::size_t kMaxDerivedDisjuncts = 16;

void collect_table_indexes(duckdb::Expression const& expr, std::unordered_set<duckdb::idx_t>& out)
{
  duckdb::ExpressionIterator::VisitExpression<duckdb::BoundColumnRefExpression>(
    expr, [&out](duckdb::BoundColumnRefExpression const& colref) {
      out.insert(colref.binding.table_index);
    });
}

bool references_multiple_tables(duckdb::Expression const& expr)
{
  std::unordered_set<duckdb::idx_t> tables;
  collect_table_indexes(expr, tables);
  return tables.size() > 1;
}

/// The single-table conjuncts of one OR branch, grouped by table index. Ordered so the derived
/// predicates - and therefore the plan - are deterministic.
using per_table_conjuncts =
  std::map<duckdb::idx_t, duckdb::vector<duckdb::unique_ptr<duckdb::Expression>>>;

/// AND-decompose @p expr; every leaf that restricts exactly one table is filed under that table in
/// @p conjuncts.
void extract_single_table_conjuncts(duckdb::Expression const& expr, per_table_conjuncts& conjuncts)
{
  if (expr.GetExpressionClass() == duckdb::ExpressionClass::BOUND_CONJUNCTION &&
      expr.GetExpressionType() == duckdb::ExpressionType::CONJUNCTION_AND) {
    for (auto const& child : expr.Cast<duckdb::BoundConjunctionExpression>().children) {
      extract_single_table_conjuncts(*child, conjuncts);
    }
    return;
  }
  // The derived copy is evaluated on rows the original disjunction rejects, and - once pushed into
  // a scan - ahead of the branch it came from. Anything whose evaluation is observable (volatile,
  // throwing, subquery, unbound parameter) would therefore change behaviour, so it is left alone.
  // This is stricter than DuckDB's own JoinDependentFilterRule, which only checks volatility.
  if (expr.IsVolatile() || expr.CanThrow() || expr.HasSubquery() || expr.HasParameter()) { return; }

  std::unordered_set<duckdb::idx_t> tables;
  collect_table_indexes(expr, tables);
  if (tables.size() != 1) { return; }

  conjuncts[*tables.begin()].push_back(expr.Copy());
}

/// AND @p conjuncts back into a single expression. Never called with an empty list.
duckdb::unique_ptr<duckdb::Expression> conjoin(
  duckdb::vector<duckdb::unique_ptr<duckdb::Expression>> const& conjuncts)
{
  if (conjuncts.size() == 1) { return conjuncts[0]->Copy(); }
  auto result =
    duckdb::make_uniq<duckdb::BoundConjunctionExpression>(duckdb::ExpressionType::CONJUNCTION_AND);
  for (auto const& conjunct : conjuncts) {
    result->children.push_back(conjunct->Copy());
  }
  return result;
}

/// Append to @p filter, for every table restricted by *all* branches of an OR-ed filter expression,
/// the OR of those per-branch restrictions.
void derive_join_dependent_filters(duckdb::LogicalFilter& filter)
{
  duckdb::vector<duckdb::unique_ptr<duckdb::Expression>> derived;

  for (auto const& expression : filter.expressions) {
    if (expression->GetExpressionClass() != duckdb::ExpressionClass::BOUND_CONJUNCTION ||
        expression->GetExpressionType() != duckdb::ExpressionType::CONJUNCTION_OR) {
      continue;
    }
    auto const& disjunction   = expression->Cast<duckdb::BoundConjunctionExpression>();
    std::size_t const num_alt = disjunction.children.size();
    if (num_alt < 2 || num_alt > kMaxDerivedDisjuncts) { continue; }

    // A disjunction confined to one table is already pushable as it stands; there is nothing to
    // derive. Only a branch spanning several tables hides a single-table restriction.
    bool spans_multiple_tables = false;
    for (auto const& branch : disjunction.children) {
      if (references_multiple_tables(*branch)) {
        spans_multiple_tables = true;
        break;
      }
    }
    if (!spans_multiple_tables) { continue; }

    std::vector<per_table_conjuncts> per_branch(num_alt);
    for (std::size_t i = 0; i < num_alt; i++) {
      extract_single_table_conjuncts(*disjunction.children[i], per_branch[i]);
    }

    for (auto const& entry : per_branch[0]) {
      auto const table_index = entry.first;

      // A branch that restricts this table not at all leaves it unrestricted overall - the
      // disjunction then implies nothing about it.
      bool restricted_by_every_branch = true;
      for (std::size_t i = 1; i < num_alt; i++) {
        if (per_branch[i].find(table_index) == per_branch[i].end()) {
          restricted_by_every_branch = false;
          break;
        }
      }
      if (!restricted_by_every_branch) { continue; }

      // Drop the conjuncts that every branch carries verbatim: DuckDB's DistributivityRule hoists
      // those out of the OR by itself, so keeping them would only bolt a duplicate predicate onto
      // the scan. Dropping them weakens each branch, and a weaker branch still yields an implied
      // (so still sound) disjunction.
      auto is_branch_invariant = [&](duckdb::Expression const& conjunct) {
        for (std::size_t i = 0; i < num_alt; i++) {
          auto const& other = per_branch[i].find(table_index)->second;
          bool found        = false;
          for (auto const& candidate : other) {
            if (candidate->Equals(conjunct)) {
              found = true;
              break;
            }
          }
          if (!found) { return false; }
        }
        return true;
      };

      auto restriction = duckdb::make_uniq<duckdb::BoundConjunctionExpression>(
        duckdb::ExpressionType::CONJUNCTION_OR);
      bool every_branch_restricts_further = true;
      for (std::size_t i = 0; i < num_alt; i++) {
        duckdb::vector<duckdb::unique_ptr<duckdb::Expression>> varying;
        for (auto const& conjunct : per_branch[i].find(table_index)->second) {
          if (!is_branch_invariant(*conjunct)) { varying.push_back(conjunct->Copy()); }
        }
        if (varying.empty()) {
          // This branch adds nothing beyond what is already hoisted, so the OR is trivially true.
          every_branch_restricts_further = false;
          break;
        }
        restriction->children.push_back(conjoin(varying));
      }
      if (!every_branch_restricts_further) { continue; }

      derived.push_back(std::move(restriction));
    }
  }

  for (auto& restriction : derived) {
    filter.expressions.push_back(std::move(restriction));
  }
}

void derive_join_dependent_filters_recursive(duckdb::LogicalOperator& op)
{
  if (op.type == duckdb::LogicalOperatorType::LOGICAL_FILTER) {
    derive_join_dependent_filters(op.Cast<duckdb::LogicalFilter>());
  }
  for (auto& child : op.children) {
    derive_join_dependent_filters_recursive(*child);
  }
}

}  // namespace

void sirius_pre_optimizer_hook(duckdb::OptimizerExtensionInput& input,
                               duckdb::unique_ptr<duckdb::LogicalOperator>& plan)
{
  if (!plan || !gpu_execution_enabled(input.context)) { return; }
  // Mirror sirius_optimizer_hook's gate: when Sirius never initialized (or this
  // is one of its internal queries), the query runs on CPU and its plan must
  // stay byte-identical to a stock DuckDB plan — the derivation is
  // row-preserving but still perturbs EXPLAIN output and cost estimates.
  auto ctx = input.context.registered_state->Get<duckdb::SiriusContext>("sirius_state");
  if (!ctx || !ctx->is_initialized()) { return; }
  auto conn_state = duckdb::get_sirius_connection_state(input.context);
  if (!conn_state || conn_state->is_internal_query_active()) { return; }

  // Optimizer hooks must not throw: a failed derivation only costs the pushdown, never the query.
  try {
    derive_join_dependent_filters_recursive(*plan);
  } catch (std::exception& e) {
    SIRIUS_LOG_DEBUG("Transparent execution: join-dependent filter derivation failed: {}",
                     sirius::sanitized_message(e));
  }
}

namespace {

/// Clone a LogicalGet without the serialize round-trip: deserializing a serialized scan re-runs
/// the table function's bind (ParquetScanDeserialize re-binds the whole multi-file list,
/// re-opening every file), whereas FunctionData::Copy() is an in-memory copy
/// (MultiFileBindData::Copy for parquet). Field-for-field superset of what LogicalGet's
/// serialization restores, except `dynamic_filters`, which serialization also drops: sharing the
/// set would couple this clone to the CPU plan DuckDB keeps for fallback. Children are cloned by
/// the caller.
duckdb::unique_ptr<duckdb::LogicalOperator> clone_logical_get(duckdb::LogicalGet& get,
                                                              duckdb::ClientContext& context,
                                                              plan_copy_stats& stats)
{
  duckdb::unique_ptr<duckdb::FunctionData> bind_data_copy;
  if (get.bind_data) {
    try {
      bind_data_copy = get.bind_data->Copy();
    } catch (duckdb::NotImplementedException&) {
      // A bind data that declares itself uncopyable.
    } catch (duckdb::InternalException&) {
      // TableFunctionData's default Copy() signals "no copy support" as an InternalException
      // ("Copy not supported for TableFunctionData", duckdb/src/function/function.cpp). Only
      // these two types select the fallback; anything else (bad_alloc, a genuine internal
      // error from a real Copy()) propagates instead of being masked into the slow path.
    }
    if (!bind_data_copy) {
      // No usable Copy(): serialize this leaf the way the whole-plan Copy used to.
      // Deserialization re-binds the function, so this is the expensive path — but it only
      // runs for the leaf that needs it, and only when it has no cheap copy.
      ++stats.serialized_gets;
      return get.Copy(context);
    }
  }
  ++stats.bind_copied_gets;
  // ------------------------------------------------------------------------
  // DRIFT TRIPWIRE — hand-copied LogicalGet state.
  // This clone must restore every field LogicalGet's serialization does
  // (duckdb/src/planner/operator/logical_get.cpp) plus the base
  // LogicalOperator fields create_plan reads. When upgrading the DuckDB
  // submodule, diff LogicalGet (logical_get.hpp) and its Serialize() against
  // this list and extend it for any new field:
  //   ctor:         table_index, function, bind_data, returned_types, names,
  //                 virtual_columns
  //   copied below: projection_ids, table_filters (deep copy), parameters,
  //                 named_parameters, input_table_types, input_table_names,
  //                 projected_input, extra_info (field by field: move-only),
  //                 ordinality_idx, row_group_order_options, column_ids,
  //                 types, estimated_cardinality, has_estimated_cardinality,
  //                 expressions
  //   deliberately NOT copied: dynamic_filters (serialization drops it too;
  //                 sharing the set would couple the clone to the CPU
  //                 fallback plan — see the function comment).
  // A missed serialized field surfaces as the serialization-equivalence
  // checks in test/cpp/planner/test_copy_logical_plan.cpp failing, but a new
  // field the serializer does not cover will not — check the header, not
  // just Serialize(). (No sizeof(LogicalGet) static_assert: object layout
  // shifts with toolchain/standard-library changes, so it would fire on
  // unrelated compiler bumps rather than only on real field drift.)
  // ------------------------------------------------------------------------
  auto clone            = duckdb::make_uniq<duckdb::LogicalGet>(get.table_index,
                                                     get.function,
                                                     std::move(bind_data_copy),
                                                     get.returned_types,
                                                     get.names,
                                                     get.virtual_columns);
  clone->projection_ids = get.projection_ids;
  // Deep copy: create_plan moves table_filters out of the scan it consumes, so the clone must
  // never alias the original's filters.
  clone->table_filters     = std::move(*get.table_filters.Copy());
  clone->parameters        = get.parameters;
  clone->named_parameters  = get.named_parameters;
  clone->input_table_types = get.input_table_types;
  clone->input_table_names = get.input_table_names;
  clone->projected_input   = get.projected_input;
  // ExtraOperatorInfo is move-only (unique_ptr member); copy it field by field.
  clone->extra_info.file_filters = get.extra_info.file_filters;
  if (get.extra_info.total_files.IsValid()) {
    clone->extra_info.total_files = get.extra_info.total_files.GetIndex();
  }
  if (get.extra_info.filtered_files.IsValid()) {
    clone->extra_info.filtered_files = get.extra_info.filtered_files.GetIndex();
  }
  if (get.extra_info.sample_options) {
    clone->extra_info.sample_options = get.extra_info.sample_options->Copy();
  }
  clone->ordinality_idx = get.ordinality_idx;
  if (get.row_group_order_options) {
    clone->row_group_order_options =
      duckdb::make_uniq<duckdb::RowGroupOrderOptions>(*get.row_group_order_options);
  }
  auto column_ids = get.GetColumnIds();
  clone->SetColumnIds(std::move(column_ids));
  clone->types                     = get.types;
  clone->estimated_cardinality     = get.estimated_cardinality;
  clone->has_estimated_cardinality = get.has_estimated_cardinality;
  for (auto const& expression : get.expressions) {
    clone->expressions.push_back(expression->Copy());
  }
  return clone;
}

duckdb::unique_ptr<duckdb::LogicalOperator> clone_plan_structural(duckdb::LogicalOperator& plan,
                                                                  duckdb::ClientContext& context,
                                                                  plan_copy_stats& stats)
{
  ++stats.nodes;

  // Detach the children so the node-local Copy below serializes exactly one node; each child is
  // then cloned by recursion. The moved-out children are restored even when Copy throws.
  auto children = std::move(plan.children);
  plan.children.clear();

  duckdb::unique_ptr<duckdb::LogicalOperator> clone;
  try {
    if (plan.type == duckdb::LogicalOperatorType::LOGICAL_GET) {
      clone = clone_logical_get(plan.Cast<duckdb::LogicalGet>(), context, stats);
    } else {
      clone = plan.Copy(context);
    }
  } catch (...) {
    plan.children = std::move(children);
    throw;
  }
  plan.children = std::move(children);

  for (auto& child : plan.children) {
    clone->children.push_back(clone_plan_structural(*child, context, stats));
  }
  return clone;
}

}  // namespace

duckdb::unique_ptr<duckdb::LogicalOperator> copy_logical_plan(duckdb::LogicalOperator& plan,
                                                              duckdb::ClientContext& context,
                                                              plan_copy_stats* stats)
{
  plan_copy_stats local_stats;
  auto& used_stats = stats != nullptr ? *stats : local_stats;
  auto const start = std::chrono::steady_clock::now();
  auto clone       = clone_plan_structural(plan, context, used_stats);
  auto const micros =
    std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now() - start)
      .count();
  SIRIUS_LOG_DEBUG(
    "Transparent execution: cloned logical plan in {} us ({} nodes, {} scans bind-data-copied, "
    "{} scans serialized)",
    micros,
    used_stats.nodes,
    used_stats.bind_copied_gets,
    used_stats.serialized_gets);
  return clone;
}

void sirius_optimizer_hook(duckdb::OptimizerExtensionInput& input,
                           duckdb::unique_ptr<duckdb::LogicalOperator>& plan)
{
  if (!gpu_execution_enabled(input.context)) { return; }

  auto& context = input.context;

  auto ctx = context.registered_state->Get<duckdb::SiriusContext>("sirius_state");
  if (!ctx || !ctx->is_initialized()) { return; }
  auto conn_state = duckdb::get_sirius_connection_state(context);
  if (!conn_state || conn_state->is_internal_query_active()) { return; }

  // Copy the optimized plan into THIS connection's per-connection state,
  // stamped with the current planning generation. OnFinalizePrepare will
  // attempt create_plan() on this copy — that's the single source of truth for
  // GPU support. If the plan contains unsupported operators, create_plan()
  // throws and we fall back to CPU. A capture whose planning attempt never
  // reaches finalize (e.g. Connection::ExtractPlan) is structurally rejected
  // at the next attempt by the generation check.
  //
  // Plan-copy failures make the query ineligible for GPU execution. Optimizer
  // hooks must not throw, so log a readable message and decline the plan.
  try {
    conn_state->set_captured_plan(copy_logical_plan(*plan, context));
  } catch (std::exception& e) {
    SIRIUS_LOG_DEBUG("Transparent execution: failed to copy logical plan: {}",
                     sirius::sanitized_message(e));
  }
}

}  // namespace sirius::transparent
