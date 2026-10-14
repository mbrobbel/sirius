// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#include "planner/delim_cte_inversion.hpp"

#include "log/logging.hpp"

#include <duckdb/optimizer/column_binding_replacer.hpp>
#include <duckdb/planner/expression/bound_columnref_expression.hpp>
#include <duckdb/planner/expression_iterator.hpp>
#include <duckdb/planner/operator/logical_aggregate.hpp>
#include <duckdb/planner/operator/logical_comparison_join.hpp>
#include <duckdb/planner/operator/logical_cteref.hpp>
#include <duckdb/planner/operator/logical_delim_get.hpp>
#include <duckdb/planner/operator/logical_filter.hpp>
#include <duckdb/planner/operator/logical_materialized_cte.hpp>
#include <duckdb/planner/operator/logical_projection.hpp>

#include <set>
#include <unordered_map>
#include <unordered_set>

namespace sirius::planner {
namespace {
using namespace duckdb;
using slot = unique_ptr<LogicalOperator>*;

struct location {
  slot owner;
  LogicalOperator* parent;
};

using tree_index = std::unordered_map<LogicalOperator*, location>;

void index_tree(slot owner, LogicalOperator* parent, tree_index& index)
{
  auto* op = owner->get();
  index.emplace(op, location{owner, parent});
  for (auto& child : op->children)
    index_tree(&child, op, index);
}

bool within(LogicalOperator* node, LogicalOperator* root, tree_index const& index)
{
  for (; node; node = index.at(node).parent) {
    if (node == root) return true;
  }
  return false;
}

bool named(LogicalOperator& op, char const* prefix)
{
  return op.type == LogicalOperatorType::LOGICAL_MATERIALIZED_CTE &&
         op.Cast<LogicalMaterializedCTE>().ctename.GetIdentifierName().starts_with(prefix);
}

bool domain_wrapper(LogicalOperator const& op)
{
  return op.children.size() == 1 && (op.type == LogicalOperatorType::LOGICAL_PROJECTION ||
                                     op.type == LogicalOperatorType::LOGICAL_FILTER);
}

// Keep the optimized domain computation, but deduplicate only the input columns it uses.
// Projection indexes remain local to their original tables; no new table indexes are needed.
unique_ptr<LogicalOperator> copy_inlined_domain(LogicalAggregate& aggregate,
                                                LogicalCTERef& domain,
                                                LogicalProjection& producer,
                                                vector<unique_ptr<Expression>>& dedup_columns)
{
  auto result = make_uniq<LogicalAggregate>(
    aggregate.group_index, aggregate.aggregate_index, vector<unique_ptr<Expression>>{});
  result->grouping_sets     = aggregate.grouping_sets;
  result->distinct_validity = aggregate.distinct_validity;
  for (auto& stats : aggregate.group_stats)
    result->group_stats.push_back(stats ? make_uniq<BaseStatistics>(stats->Copy()) : nullptr);
  for (auto& group : aggregate.groups)
    result->groups.push_back(group->Copy());
  std::unordered_map<TableIndex, std::set<ProjectionIndex>> needed;
  bool valid           = true;
  auto require_columns = [&](Expression const& expr) {
    if (expr.IsVolatile()) valid = false;
    ExpressionIterator::VisitExpression<BoundColumnRefExpression>(expr, [&](auto const& ref) {
      if (ref.Depth() != 0 || !ref.Binding().column_index.IsValid()) valid = false;
      needed[ref.Binding().table_index].insert(ref.Binding().column_index);
    });
  };
  for (auto& group : result->groups)
    require_columns(*group);
  vector<LogicalOperator*> ancestors{result.get()};
  // Compaction replacements are ascending, so the replacer cannot chain a new index again.
  auto remap = [&](ColumnBindingReplacer& replacer) {
    for (auto* ancestor : ancestors)
      replacer.VisitOperatorBindings(*ancestor);
  };
  bool only_filters = true;
  auto* tail        = static_cast<LogicalOperator*>(result.get());
  auto* input       = aggregate.children[0].get();
  while (input != &domain) {
    if (!domain_wrapper(*input)) return nullptr;
    unique_ptr<LogicalOperator> copy;
    if (input->type == LogicalOperatorType::LOGICAL_PROJECTION) {
      only_filters     = false;
      auto& projection = input->Cast<LogicalProjection>();
      auto required    = needed[projection.table_index];
      vector<unique_ptr<Expression>> expressions;
      ColumnBindingReplacer replacer;
      for (auto i : ProjectionIndex::GetIndexes(projection.expressions.size())) {
        auto& expr = *projection.expressions[i];
        if (expr.IsVolatile()) return nullptr;
        // Retain potentially throwing expressions to preserve their evaluation.
        if (!required.count(i) && !expr.CanThrow()) continue;
        replacer.replacement_bindings.emplace_back(
          ColumnBinding(projection.table_index, i),
          ColumnBinding(projection.table_index, ProjectionIndex(expressions.size())));
        require_columns(expr);
        expressions.push_back(expr.Copy());
      }
      if (expressions.empty()) return nullptr;
      for (auto i : required) {
        if (!i.IsValid() || i.GetIndex() >= projection.expressions.size()) return nullptr;
      }
      remap(replacer);
      copy = make_uniq<LogicalProjection>(projection.table_index, std::move(expressions));
    } else {
      auto filter = make_uniq<LogicalFilter>();
      for (auto& expr : input->expressions) {
        require_columns(*expr);
        filter->expressions.push_back(expr->Copy());
      }
      // Interior filter output maps are unnecessary: the aggregate fixes the public schema.
      copy = std::move(filter);
    }
    copy->estimated_cardinality     = input->estimated_cardinality;
    copy->has_estimated_cardinality = input->has_estimated_cardinality;
    tail->children.push_back(std::move(copy));
    tail = tail->children[0].get();
    ancestors.push_back(tail);
    input = input->children[0].get();
  }
  if (!valid || needed[domain.table_index].empty()) return nullptr;
  vector<LogicalType> types;
  ColumnBindingReplacer replacer;
  for (auto i : needed[domain.table_index]) {
    if (!i.IsValid() || i.GetIndex() >= domain.chunk_types.size()) return nullptr;
    replacer.replacement_bindings.emplace_back(
      ColumnBinding(domain.table_index, i),
      ColumnBinding(domain.table_index, ProjectionIndex(types.size())));
    auto const& type = domain.chunk_types[i];
    // Floating-point grouping merges signed zero, which a later expression can distinguish.
    // Admit only domains whose grouping equality preserves the value used by expressions.
    if (!type.IsIntegral() && type.id() != LogicalTypeId::DECIMAL &&
        type.id() != LogicalTypeId::BOOLEAN)
      return nullptr;
    types.push_back(type);
    dedup_columns.push_back(make_uniq<BoundColumnRefExpression>(
      domain.chunk_types[i], ColumnBinding(producer.table_index, i)));
  }
  std::set<ProjectionIndex> group_columns;
  for (auto& group : aggregate.groups) {
    auto& ref = group->Cast<BoundColumnRefExpression>();
    if (ref.Binding().table_index != domain.table_index) only_filters = false;
    group_columns.insert(ref.Binding().column_index);
  }
  remap(replacer);
  tail->children.push_back(make_uniq<LogicalDelimGet>(domain.table_index, std::move(types)));
  if (only_filters && group_columns.size() == aggregate.groups.size() &&
      group_columns == needed[domain.table_index]) {
    // Filtering unique grouping keys preserves uniqueness; only rename/reorder their bindings.
    auto projection =
      make_uniq<LogicalProjection>(aggregate.group_index, std::move(result->groups));
    projection->children = std::move(result->children);
    return std::move(projection);
  }
  return std::move(result);
}

vector<LogicalCTERef*> references(LogicalOperator& root, TableIndex cte)
{
  vector<LogicalCTERef*> result;
  if (root.type == LogicalOperatorType::LOGICAL_CTE_REF) {
    auto& ref = root.Cast<LogicalCTERef>();
    if (ref.cte_index == cte) result.push_back(&ref);
  }
  for (auto& child : root.children) {
    auto found = references(*child, cte);
    result.insert(result.end(), found.begin(), found.end());
  }
  return result;
}

bool ordinary_ref(LogicalCTERef const& ref, vector<LogicalType> const& types)
{
  return ref.children.empty() && ref.expressions.empty() && !ref.is_recurring &&
         ref.correlated_columns == 0 && ref.chunk_types == types;
}

void try_invert(unique_ptr<LogicalOperator>& plan)
{
  auto& cte    = plan->Cast<LogicalMaterializedCTE>();
  auto decline = [&](char const* reason) {
    SIRIUS_LOG_DEBUG(
      "delim_cte_inversion: declined {}: {}", cte.ctename.GetIdentifierName(), reason);
  };
  if (cte.children.size() != 2 || !cte.correlated_columns.empty() ||
      cte.children[0]->type != LogicalOperatorType::LOGICAL_PROJECTION ||
      cte.children[0]->children.size() != 1 || cte.children[0]->expressions.empty()) {
    return decline("producer is not an uncorrelated projection of the CTE width");
  }
  auto& producer = cte.children[0]->Cast<LogicalProjection>();
  vector<LogicalType> producer_types;
  for (auto& expr : producer.expressions)
    producer_types.push_back(expr->GetReturnType());
  auto refs = references(cte, cte.table_index);
  if (refs.size() != 2) return decline("expected exactly two outer CTE references");

  tree_index index;
  index_tree(&plan, nullptr, index);
  LogicalCTERef* row              = nullptr;
  LogicalCTERef* domain           = nullptr;
  LogicalAggregate* aggregate_ptr = nullptr;
  for (auto* ref : refs) {
    auto* parent = index.at(ref).parent;
    if (!parent || !within(ref, cte.children[1].get(), index) ||
        !ordinary_ref(*ref, producer_types))
      return decline("invalid outer CTE reference");
    if (parent->type == LogicalOperatorType::LOGICAL_COMPARISON_JOIN && !row) {
      row = ref;
    } else {
      while (parent && domain_wrapper(*parent))
        parent = index.at(parent).parent;
      if (!parent || parent->type != LogicalOperatorType::LOGICAL_AGGREGATE_AND_GROUP_BY || domain)
        return decline("outer references are not a join input and a dedup input");
      domain        = ref;
      aggregate_ptr = &parent->Cast<LogicalAggregate>();
    }
  }
  if (!row || !domain) return decline("missing join or dedup input");
  auto& join      = index.at(row).parent->Cast<LogicalComparisonJoin>();
  auto& aggregate = *aggregate_ptr;
  if (join.children.size() != 2 || join.conditions.empty() ||
      !join.duplicate_eliminated_columns.empty() || join.delim_flipped) {
    return decline("join already has delim metadata or invalid arity");
  }
  if (aggregate.children.size() != 1 || !aggregate.expressions.empty() ||
      !aggregate.grouping_functions.empty() || aggregate.groups.empty() ||
      aggregate.grouping_sets.size() > 1)
    return decline("aggregate is not plain deduplication");
  if (!aggregate.grouping_sets.empty()) {
    auto& set = aggregate.grouping_sets[0];
    if (set.size() != aggregate.groups.size()) return decline("incomplete grouping set");
    for (auto i : ProjectionIndex::GetIndexes(aggregate.groups.size())) {
      if (!set.count(i)) return decline("incomplete grouping set");
    }
  }
  vector<LogicalType> dedup_types;
  vector<unique_ptr<Expression>> dedup_columns;
  bool wrapped_domain = aggregate.children[0].get() != domain;
  for (auto& expr : aggregate.groups) {
    if (expr->GetExpressionClass() != ExpressionClass::BOUND_COLUMN_REF)
      return decline("dedup group is not a column reference");
    auto& ref    = expr->Cast<BoundColumnRefExpression>();
    auto binding = ref.Binding();
    if (ref.Depth() != 0 || !binding.column_index.IsValid())
      return decline("invalid dedup group reference");
    dedup_types.push_back(ref.GetReturnType());
    if (wrapped_domain) continue;
    if (binding.table_index != domain->table_index ||
        binding.column_index.GetIndex() >= producer_types.size() ||
        ref.GetReturnType() != producer_types[binding.column_index])
      return decline("dedup group does not reference a matching producer column");
    dedup_columns.push_back(make_uniq<BoundColumnRefExpression>(
      ref.GetReturnType(), ColumnBinding(producer.table_index, binding.column_index)));
  }
  unique_ptr<LogicalOperator> inlined_domain;
  if (wrapped_domain) {
    inlined_domain = copy_inlined_domain(aggregate, *domain, producer, dedup_columns);
    if (!inlined_domain) return decline("unsupported optimized domain computation");
  }

  LogicalMaterializedCTE* dedup_cte = nullptr;
  vector<LogicalOperator*> scans;
  auto* dedup_root       = static_cast<LogicalOperator*>(&aggregate);
  auto* aggregate_parent = index.at(dedup_root).parent;
  while (aggregate_parent && aggregate_parent->type == LogicalOperatorType::LOGICAL_PROJECTION &&
         aggregate_parent->children.size() == 1) {
    auto bindings     = dedup_root->GetColumnBindings();
    auto& expressions = aggregate_parent->expressions;
    bool identity     = expressions.size() == bindings.size();
    for (idx_t i = 0; identity && i < expressions.size(); ++i) {
      identity = expressions[i]->GetExpressionClass() == ExpressionClass::BOUND_COLUMN_REF;
      if (identity) {
        auto& ref = expressions[i]->Cast<BoundColumnRefExpression>();
        identity =
          ref.Depth() == 0 && ref.Binding() == bindings[i] && ref.GetReturnType() == dedup_types[i];
      }
    }
    if (!identity) break;
    dedup_root       = aggregate_parent;
    aggregate_parent = index.at(dedup_root).parent;
  }
  if (aggregate_parent && named(*aggregate_parent, "__duckdb_delim_dedup_")) {
    if (wrapped_domain) return decline("optimized materialized dedup producer");
    dedup_cte = &aggregate_parent->Cast<LogicalMaterializedCTE>();
    if (dedup_cte->children.size() != 2 || dedup_cte->children[0].get() != dedup_root ||
        !dedup_cte->correlated_columns.empty() || dedup_cte->column_count != dedup_types.size()) {
      return decline("invalid dedup CTE producer");
    }
    auto dedup_refs = references(cte, dedup_cte->table_index);
    for (auto* ref : dedup_refs) {
      if (!within(ref, dedup_cte->children[1].get(), index) || !ordinary_ref(*ref, dedup_types)) {
        return decline("invalid dedup CTE consumer");
      }
      scans.push_back(ref);
    }
  } else {
    scans.push_back(&aggregate);
  }
  if (scans.empty()) return decline("no dedup consumers");
  auto side   = join.children[0].get() == row ? 0 : 1;
  auto* inner = join.children[1 - side].get();
  std::unordered_set<TableIndex> scan_tables;
  for (auto* scan : scans) {
    if (!within(scan, inner, index))
      return decline("dedup consumer is outside the opposite join input");
    // Nested delim joins own their scans independently during physical lowering.
    for (auto* ancestor = index.at(scan).parent; ancestor != &join;
         ancestor       = index.at(ancestor).parent) {
      if (ancestor->type == LogicalOperatorType::LOGICAL_MATERIALIZED_CTE)
        return decline("dedup consumer crosses a materialized CTE boundary");
      if (ancestor->type == LogicalOperatorType::LOGICAL_DELIM_JOIN) {
        auto& nested = ancestor->Cast<LogicalComparisonJoin>();
        if (!within(scan, nested.children[nested.delim_flipped ? 1 : 0].get(), index))
          return decline("dedup consumer is owned by a nested delim join");
      }
    }
    scan_tables.insert(scan == &aggregate ? aggregate.group_index
                                          : scan->Cast<LogicalCTERef>().table_index);
  }
  std::unordered_set<LogicalOperator*> path{&join};
  for (auto* ancestor = index.at(&join).parent; ancestor != &cte;
       ancestor       = index.at(ancestor).parent) {
    if (!ancestor) return decline("join is outside the CTE consumer");
    if (ancestor != dedup_cte && !domain_wrapper(*ancestor)) {
      return decline("unsupported operator between CTE and join");
    }
    path.insert(ancestor);
  }
  if (dedup_cte && !path.count(dedup_cte)) return decline("dedup CTE does not enclose the join");
  for (auto binding : cte.GetColumnBindings()) {
    if (binding.table_index == row->table_index)
      return decline("row binding escapes the CTE output");
  }

  bool clean = true;
  for (auto const& [op, location] : index) {
    LogicalOperatorVisitor::EnumerateExpressions(*op, [&](unique_ptr<Expression>* expr) {
      ExpressionIterator::VisitExpression<BoundColumnRefExpression>(**expr, [&](auto const& ref) {
        auto table = ref.Binding().table_index;
        if (table == domain->table_index && !within(op, &aggregate, index)) clean = false;
        if (table == row->table_index && !path.count(op)) clean = false;
        if (scan_tables.count(table) && !within(op, inner, index) && !path.count(op)) clean = false;
      });
    });
  }
  if (!clean) return decline("bindings escape their permitted scope");

  // All checks precede mutation; retain scan table indexes and the consumer output bindings.
  D_ASSERT(!inlined_domain || scans.size() == 1);
  auto old_bindings = row->GetColumnBindings();
  for (auto* scan : scans) {
    auto table =
      scan == &aggregate ? aggregate.group_index : scan->Cast<LogicalCTERef>().table_index;
    unique_ptr<LogicalOperator> replacement =
      inlined_domain ? std::move(inlined_domain) : make_uniq<LogicalDelimGet>(table, dedup_types);
    replacement->estimated_cardinality     = scan->estimated_cardinality;
    replacement->has_estimated_cardinality = scan->has_estimated_cardinality;
    *index.at(scan).owner                  = std::move(replacement);
  }
  join.type                         = LogicalOperatorType::LOGICAL_DELIM_JOIN;
  join.duplicate_eliminated_columns = std::move(dedup_columns);
  join.delim_flipped                = side == 1;
  join.children[side]               = std::move(cte.children[0]);
  ColumnBindingReplacer replacer;
  replacer.AddReplacements(old_bindings, join.children[side]->GetColumnBindings());
  replacer.stop_operator = join.children[1 - side].get();
  replacer.VisitOperator(*cte.children[1]);
  if (dedup_cte) {
    auto child                 = std::move(dedup_cte->children[1]);
    *index.at(dedup_cte).owner = std::move(child);
  }
  auto child = std::move(cte.children[1]);
  plan       = std::move(child);
}
}  // namespace

void invert_delim_ctes(duckdb::unique_ptr<duckdb::LogicalOperator>& plan)
{
  for (auto& child : plan->children)
    invert_delim_ctes(child);
  if (named(*plan, "__duckdb_delim_") && !named(*plan, "__duckdb_delim_dedup_")) try_invert(plan);
}
}  // namespace sirius::planner
