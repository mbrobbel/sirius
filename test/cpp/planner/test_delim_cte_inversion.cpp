// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#include "planner/delim_cte_inversion.hpp"

#include <catch.hpp>
#include <duckdb/execution/column_binding_resolver.hpp>
#include <duckdb/optimizer/column_binding_replacer.hpp>
#include <duckdb/planner/expression/bound_cast_expression.hpp>
#include <duckdb/planner/expression/bound_columnref_expression.hpp>
#include <duckdb/planner/expression/bound_comparison_expression.hpp>
#include <duckdb/planner/expression/bound_constant_expression.hpp>
#include <duckdb/planner/operator/logical_aggregate.hpp>
#include <duckdb/planner/operator/logical_comparison_join.hpp>
#include <duckdb/planner/operator/logical_cross_product.hpp>
#include <duckdb/planner/operator/logical_cteref.hpp>
#include <duckdb/planner/operator/logical_dummy_scan.hpp>
#include <duckdb/planner/operator/logical_filter.hpp>
#include <duckdb/planner/operator/logical_materialized_cte.hpp>
#include <duckdb/planner/operator/logical_projection.hpp>

namespace {
using namespace duckdb;

unique_ptr<Expression> column(idx_t table, idx_t col = 0)
{
  return make_uniq<BoundColumnRefExpression>(
    LogicalType::INTEGER, ColumnBinding(TableIndex(table), ProjectionIndex(col)));
}

unique_ptr<LogicalOperator> cte_ref(idx_t table, idx_t cte)
{
  return make_uniq<LogicalCTERef>(TableIndex(table),
                                  TableIndex(cte),
                                  vector<LogicalType>{LogicalType::INTEGER},
                                  vector<Identifier>{"key"});
}

struct generated_plan {
  unique_ptr<LogicalOperator> root;
  LogicalComparisonJoin* join;
  LogicalAggregate* aggregate;
  LogicalProjection* producer;
  LogicalMaterializedCTE* outer;
};

generated_plan generated(bool materialized, bool flipped, JoinType type = JoinType::INNER)
{
  vector<unique_ptr<Expression>> values;
  values.push_back(make_uniq<BoundConstantExpression>(Value::INTEGER(7)));
  auto source = make_uniq<LogicalProjection>(TableIndex(1), std::move(values));
  source->children.push_back(make_uniq<LogicalDummyScan>(TableIndex(0)));
  auto producer      = LogicalProjection::CreateIdentity(TableIndex(2), std::move(source));
  auto* producer_ptr = producer.get();
  auto aggregate =
    make_uniq<LogicalAggregate>(TableIndex(6), TableIndex(7), vector<unique_ptr<Expression>>{});
  aggregate->groups.push_back(column(5));
  aggregate->children.push_back(cte_ref(5, 3));
  auto* aggregate_ptr = aggregate.get();

  unique_ptr<LogicalOperator> inner;
  if (materialized) {
    auto cross = make_uniq<LogicalCrossProduct>(cte_ref(9, 8), cte_ref(10, 8));
    inner      = std::move(cross);
  } else {
    inner = std::move(aggregate);
  }
  // The inner projection keeps dedup bindings local to the inner join input.
  vector<unique_ptr<Expression>> exprs;
  exprs.push_back(column(materialized ? 9 : 6));
  auto inner_projection = make_uniq<LogicalProjection>(TableIndex(11), std::move(exprs));
  inner_projection->children.push_back(std::move(inner));
  auto join        = make_uniq<LogicalComparisonJoin>(type);
  join->mark_index = TableIndex(15);
  join->children.push_back(cte_ref(4, 3));
  join->children.push_back(std::move(inner_projection));
  if (flipped) std::swap(join->children[0], join->children[1]);
  join->conditions.emplace_back(
    column(flipped ? 11 : 4), column(flipped ? 4 : 11), ExpressionType::COMPARE_EQUAL);
  auto* join_ptr = join.get();
  unique_ptr<LogicalOperator> consumer =
    LogicalProjection::CreateIdentity(TableIndex(12), std::move(join));
  if (materialized) {
    consumer = make_uniq<LogicalMaterializedCTE>(Identifier("__duckdb_delim_dedup_8"),
                                                 TableIndex(8),
                                                 1,
                                                 std::move(aggregate),
                                                 std::move(consumer),
                                                 CTEMaterialize::CTE_MATERIALIZE_DEFAULT);
  }
  consumer    = LogicalProjection::CreateIdentity(TableIndex(13), std::move(consumer));
  auto root   = make_uniq<LogicalMaterializedCTE>(Identifier("__duckdb_delim_3"),
                                                TableIndex(3),
                                                1,
                                                std::move(producer),
                                                std::move(consumer),
                                                CTEMaterialize::CTE_MATERIALIZE_DEFAULT);
  auto* outer = root.get();
  return {std::move(root), join_ptr, aggregate_ptr, producer_ptr, outer};
}

idx_t count(LogicalOperator const& op, LogicalOperatorType type)
{
  idx_t result = op.type == type;
  for (auto& child : op.children)
    result += count(*child, type);
  return result;
}
}  // namespace

TEST_CASE("Delim CTE inversion restores both dedup representations",
          "[planner][delim_cte_inversion]")
{
  auto materialized          = GENERATE(false, true);
  auto flipped               = GENERATE(false, true);
  auto explicit_grouping_set = GENERATE(false, true);
  auto plan                  = generated(materialized, flipped);
  if (explicit_grouping_set) plan.aggregate->grouping_sets.push_back({ProjectionIndex(0)});
  plan.outer->column_count = 100;  // Column pruning leaves this metadata at its original width.
  auto output              = plan.root->GetColumnBindings();
  ColumnBindingResolver before(true);
  before.VisitOperator(*plan.root);
  sirius::planner::invert_delim_ctes(plan.root);
  REQUIRE(count(*plan.root, LogicalOperatorType::LOGICAL_MATERIALIZED_CTE) == 0);
  REQUIRE(count(*plan.root, LogicalOperatorType::LOGICAL_DELIM_JOIN) == 1);
  REQUIRE(count(*plan.root, LogicalOperatorType::LOGICAL_DELIM_GET) == (materialized ? 2 : 1));
  REQUIRE(plan.root->GetColumnBindings() == output);
  REQUIRE(plan.join->delim_flipped == flipped);
  REQUIRE(plan.join->children[flipped ? 1 : 0].get() == plan.producer);
  REQUIRE(plan.join->duplicate_eliminated_columns.size() == 1);
  CHECK(plan.join->duplicate_eliminated_columns[0]->Cast<BoundColumnRefExpression>().Binding() ==
        ColumnBinding(TableIndex(2), ProjectionIndex(0)));
  plan.root->ResolveOperatorTypes();
  ColumnBindingResolver verifier(true);
  verifier.VisitOperator(*plan.root);
  ColumnBindingResolver resolver;
  resolver.VisitOperator(*plan.root);
  CHECK(plan.join->duplicate_eliminated_columns[0]->GetExpressionType() ==
        ExpressionType::BOUND_REF);
}

TEST_CASE("Delim CTE inversion preserves producer and consumer filters",
          "[planner][delim_cte_inversion]")
{
  auto plan = generated(GENERATE(false, true), GENERATE(false, true));
  auto producer_filter =
    make_uniq<LogicalFilter>(make_uniq<BoundConstantExpression>(Value::BOOLEAN(true)));
  producer_filter->children.push_back(std::move(plan.producer->children[0]));
  plan.producer->children[0] = std::move(producer_filter);
  auto& join_slot            = plan.outer->children[1]->children[0];
  auto filter = make_uniq<LogicalFilter>(make_uniq<BoundConstantExpression>(Value::BOOLEAN(true)));
  filter->children.push_back(std::move(join_slot));
  join_slot = std::move(filter);
  sirius::planner::invert_delim_ctes(plan.root);
  REQUIRE(plan.join->type == LogicalOperatorType::LOGICAL_DELIM_JOIN);
  CHECK(count(*plan.root, LogicalOperatorType::LOGICAL_FILTER) == 2);
  plan.root->ResolveOperatorTypes();
  ColumnBindingResolver verifier(true);
  verifier.VisitOperator(*plan.root);
}

TEST_CASE("Delim CTE inversion retains optimized domain computations",
          "[planner][delim_cte_inversion]")
{
  auto flipped = GENERATE(false, true);
  auto plan    = generated(false, flipped);
  plan.producer->children[0]->expressions.push_back(
    make_uniq<BoundConstantExpression>(Value::BOOLEAN(true)));
  plan.producer->expressions.insert(
    plan.producer->expressions.begin(),
    make_uniq<BoundColumnRefExpression>(LogicalType::BOOLEAN,
                                        ColumnBinding(TableIndex(1), ProjectionIndex(1))));
  auto& domain = plan.aggregate->children[0]->Cast<LogicalCTERef>();
  domain.chunk_types.insert(domain.chunk_types.begin(), LogicalType::BOOLEAN);
  auto& row = plan.join->children[flipped ? 1 : 0]->Cast<LogicalCTERef>();
  row.chunk_types.insert(row.chunk_types.begin(), LogicalType::BOOLEAN);
  (flipped ? plan.join->right_projection_map
           : plan.join->left_projection_map).push_back(ProjectionIndex(1));
  ColumnBindingReplacer shift;
  shift.replacement_bindings.emplace_back(ColumnBinding(TableIndex(4), ProjectionIndex(0)),
                                          ColumnBinding(TableIndex(4), ProjectionIndex(1)));
  shift.replacement_bindings.emplace_back(ColumnBinding(TableIndex(5), ProjectionIndex(0)),
                                          ColumnBinding(TableIndex(5), ProjectionIndex(1)));
  shift.VisitOperator(*plan.root);
  auto filter = make_uniq<LogicalFilter>(
    BoundComparisonExpression::Create(ExpressionType::COMPARE_GREATERTHAN,
                                      column(5, 1),
                                      make_uniq<BoundConstantExpression>(Value::INTEGER(0))));
  filter->children.push_back(std::move(plan.aggregate->children[0]));
  vector<unique_ptr<Expression>> expressions;
  expressions.push_back(make_uniq<BoundConstantExpression>(Value::INTEGER(42)));
  expressions.push_back(
    BoundCastExpression::AddDefaultCastToType(column(5, 1), LogicalType::SMALLINT));
  auto projection = make_uniq<LogicalProjection>(TableIndex(30), std::move(expressions));
  projection->children.push_back(std::move(filter));
  plan.aggregate->children[0] = std::move(projection);
  plan.aggregate->groups[0]   = make_uniq<BoundColumnRefExpression>(
    LogicalType::SMALLINT, ColumnBinding(TableIndex(30), ProjectionIndex(1)));
  // The generated fixture's inner projection is identified by its table index.
  for (auto& child : plan.join->children) {
    if (child->type == LogicalOperatorType::LOGICAL_PROJECTION) {
      child->expressions[0] = BoundCastExpression::AddDefaultCastToType(
        make_uniq<BoundColumnRefExpression>(LogicalType::SMALLINT,
                                            ColumnBinding(TableIndex(6), ProjectionIndex(0))),
        LogicalType::INTEGER);
    }
  }
  plan.root->ResolveOperatorTypes();
  ColumnBindingResolver before(true);
  before.VisitOperator(*plan.root);
  auto outputs = plan.root->GetColumnBindings();
  sirius::planner::invert_delim_ctes(plan.root);
  REQUIRE(plan.join->type == LogicalOperatorType::LOGICAL_DELIM_JOIN);
  CHECK(count(*plan.root, LogicalOperatorType::LOGICAL_DELIM_GET) == 1);
  CHECK(count(*plan.root, LogicalOperatorType::LOGICAL_AGGREGATE_AND_GROUP_BY) == 1);
  CHECK(count(*plan.root, LogicalOperatorType::LOGICAL_FILTER) == 1);
  REQUIRE(plan.join->duplicate_eliminated_columns.size() == 1);
  CHECK(plan.join->duplicate_eliminated_columns[0]->GetReturnType() == LogicalType::INTEGER);
  CHECK(plan.join->duplicate_eliminated_columns[0]->Cast<BoundColumnRefExpression>().Binding() ==
        ColumnBinding(TableIndex(2), ProjectionIndex(1)));
  CHECK(plan.root->GetColumnBindings() == outputs);
  plan.root->ResolveOperatorTypes();
  ColumnBindingResolver verifier(true);
  verifier.VisitOperator(*plan.root);
}

TEST_CASE("Delim CTE inversion declines floating point domain computations",
          "[planner][delim_cte_inversion]")
{
  auto plan = generated(false, false);
  // A raw floating domain may contain both signed zeros, which compare equal in GROUP BY.
  std::function<void(LogicalOperator&)> make_double = [&](LogicalOperator& op) {
    if (op.type == LogicalOperatorType::LOGICAL_CTE_REF)
      op.Cast<LogicalCTERef>().chunk_types[0] = LogicalType::DOUBLE;
    LogicalOperatorVisitor::EnumerateExpressions(op, [](unique_ptr<Expression>* expression) {
      if ((*expression)->GetExpressionClass() == ExpressionClass::BOUND_COLUMN_REF)
        (*expression)->SetReturnType(LogicalType::DOUBLE);
    });
    for (auto& child : op.children)
      make_double(*child);
  };
  make_double(*plan.root);
  plan.producer->children[0]->expressions[0] =
    make_uniq<BoundConstantExpression>(Value::DOUBLE(-0.0));
  auto projection =
    LogicalProjection::CreateIdentity(TableIndex(30), std::move(plan.aggregate->children[0]));
  plan.aggregate->groups[0] = make_uniq<BoundColumnRefExpression>(
    LogicalType::DOUBLE, ColumnBinding(TableIndex(30), ProjectionIndex(0)));
  plan.aggregate->children[0] = std::move(projection);
  plan.root->ResolveOperatorTypes();
  ColumnBindingResolver verifier(true);
  verifier.VisitOperator(*plan.root);
  auto original = plan.root->ToString();
  sirius::planner::invert_delim_ctes(plan.root);
  CHECK(plan.root->ToString() == original);
  CHECK(plan.join->type == LogicalOperatorType::LOGICAL_COMPARISON_JOIN);
}

TEST_CASE("Delim CTE inversion allows scan bindings in join conditions",
          "[planner][delim_cte_inversion]")
{
  auto plan = generated(false, false);
  ColumnBindingReplacer replacer;
  replacer.replacement_bindings.emplace_back(ColumnBinding(TableIndex(11), ProjectionIndex(0)),
                                             ColumnBinding(TableIndex(6), ProjectionIndex(0)));
  replacer.VisitOperator(*plan.root);
  auto child             = std::move(plan.join->children[1]->children[0]);
  plan.join->children[1] = std::move(child);
  sirius::planner::invert_delim_ctes(plan.root);
  REQUIRE(plan.join->type == LogicalOperatorType::LOGICAL_DELIM_JOIN);
  CHECK(plan.join->children[1]->type == LogicalOperatorType::LOGICAL_DELIM_GET);
  plan.root->ResolveOperatorTypes();
  ColumnBindingResolver verifier(true);
  verifier.VisitOperator(*plan.root);
}

TEST_CASE("Delim CTE inversion preserves mark join metadata", "[planner][delim_cte_inversion]")
{
  auto plan = generated(GENERATE(false, true), false, JoinType::MARK);
  sirius::planner::invert_delim_ctes(plan.root);
  REQUIRE(plan.join->type == LogicalOperatorType::LOGICAL_DELIM_JOIN);
  CHECK(plan.join->join_type == JoinType::MARK);
  CHECK(plan.join->mark_index == TableIndex(15));
  ColumnBindingResolver verifier(true);
  verifier.VisitOperator(*plan.root);
}

TEST_CASE("Delim CTE inversion leaves unsafe shapes untouched", "[planner][delim_cte_inversion]")
{
  auto plan = generated(GENERATE(false, true), false);
  SECTION("a similarly named CTE with a computed group")
  {
    plan.outer->ctename       = Identifier("__duckdb_delim_42");
    plan.aggregate->groups[0] = make_uniq<BoundConstantExpression>(Value::INTEGER(1));
  }
  SECTION("a conditionless join") { plan.join->conditions.clear(); }
  SECTION("a partial grouping set") { plan.aggregate->grouping_sets.emplace_back(); }
  SECTION("multiple grouping sets")
  {
    plan.aggregate->grouping_sets.push_back({ProjectionIndex(0)});
    plan.aggregate->grouping_sets.push_back({ProjectionIndex(0)});
  }
  SECTION("a domain binding in the producer")
  {
    auto filter = make_uniq<LogicalFilter>(column(5));
    filter->children.push_back(std::move(plan.producer->children[0]));
    plan.producer->children[0] = std::move(filter);
  }
  SECTION("a row binding in the inner input")
  {
    plan.join->children[1]->expressions[0] = column(4);
  }
  SECTION("an extra outer CTE consumer")
  {
    auto cross = make_uniq<LogicalCrossProduct>(std::move(plan.join->children[1]), cte_ref(20, 3));
    plan.join->children[1] = std::move(cross);
  }
  SECTION("a join buried under another join")
  {
    auto& slot = plan.outer->children[1]->children[0];
    auto cross =
      make_uniq<LogicalCrossProduct>(std::move(slot), make_uniq<LogicalDummyScan>(TableIndex(20)));
    slot = std::move(cross);
  }
  SECTION("wrong producer width")
  {
    plan.producer->expressions.push_back(make_uniq<BoundConstantExpression>(Value::INTEGER(3)));
  }
  auto* original = plan.root.get();
  auto text      = plan.root->ToString();
  sirius::planner::invert_delim_ctes(plan.root);
  CHECK(plan.root.get() == original);
  CHECK(plan.root->ToString() == text);
  CHECK(count(*plan.root, LogicalOperatorType::LOGICAL_DELIM_JOIN) == 0);
}
