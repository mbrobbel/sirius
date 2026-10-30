// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#include "c_api/context_internal.hpp"
#include "c_api/error_internal.hpp"
#include "c_api/plan_internal.hpp"
#include "core_functions_extension.hpp"
#include "duckdb/common/arrow/arrow_wrapper.hpp"
#include "duckdb/main/connection.hpp"
#include "duckdb/main/database.hpp"
#include "duckdb/main/extension_helper.hpp"
#include "duckdb/main/relation.hpp"
#include "duckdb/main/relation/table_function_relation.hpp"
#include "duckdb/parser/statement/relation_statement.hpp"
#include "duckdb/planner/planner.hpp"
#include "exec/arrow_result_stream.hpp"
#include "expression/ast/node.hpp"
#include "op/sirius_physical_filter.hpp"
#include "op/sirius_physical_result_collector.hpp"
#include "parquet_extension.hpp"
#include "planner/sirius_physical_plan_generator.hpp"
#include "sirius_engine.hpp"
#include "sirius_interface.hpp"

#include <rmm/error.hpp>

#include <sirius-c/plan/execution.h>

#include <array>
#include <cerrno>
#include <stdexcept>

namespace {
struct execution_resources {
  duckdb::shared_ptr<duckdb::SiriusContext> engine;
  duckdb::DuckDB database{nullptr};
  duckdb::Connection connection{database};
  explicit execution_resources(duckdb::shared_ptr<duckdb::SiriusContext> engine)
    : engine(std::move(engine))
  {
    database.LoadStaticExtension<duckdb::CoreFunctionsExtension>();
    database.LoadStaticExtension<duckdb::ParquetExtension>();
    connection.context->registered_state->Insert("sirius_state", this->engine);
    connection.context->registered_state->Insert(
      "sirius_connection_state", duckdb::make_shared_ptr<duckdb::SiriusConnectionState>());
  }
};
// Keep the execution environment alive until the foreign consumer releases the
// materialized stream, including on errors while reading it.
struct result_stream {
  std::unique_ptr<execution_resources> resources;
  ArrowArrayStream inner{};
  ~result_stream()
  {
    if (inner.release) { inner.release(&inner); }
  }
  static result_stream& get(ArrowArrayStream* stream)
  {
    return *static_cast<result_stream*>(stream->private_data);
  }
  const char* callback_error = nullptr;
  template <typename Output, typename Callback>
  static int read(ArrowArrayStream* stream, Output* out, Callback callback) noexcept
  {
    auto& s          = get(stream);
    s.callback_error = nullptr;
    *out             = {};
    int status       = EIO;
    try {
      return callback(&s.inner, out);
    } catch (const std::bad_alloc&) {
      s.callback_error = "Arrow output allocation failed";
      status           = ENOMEM;
    } catch (...) {
      s.callback_error = "Arrow output conversion failed";
    }
    // Exporters install release only after establishing a releasable object.
    if (out->release) { out->release(out); }
    *out = {};
    return status;
  }
  static int schema(ArrowArrayStream* stream, ArrowSchema* out) noexcept
  {
    return read(stream, out, get(stream).inner.get_schema);
  }
  static int next(ArrowArrayStream* stream, ArrowArray* out) noexcept
  {
    return read(stream, out, get(stream).inner.get_next);
  }
  static const char* error(ArrowArrayStream* stream) noexcept
  {
    auto& s = get(stream);
    if (s.callback_error) { return s.callback_error; }
    try {
      return s.inner.get_last_error(&s.inner);
    } catch (...) {
      return "Arrow output diagnostic unavailable";
    }
  }
  static void release(ArrowArrayStream* stream)
  {
    auto* self           = static_cast<result_stream*>(stream->private_data);
    stream->release      = nullptr;
    stream->private_data = nullptr;
    delete self;
  }
};

duckdb::unique_ptr<duckdb::QueryResult> execute(execution_resources& resources,
                                                const sirius::c_api::relation& relation)
{
  auto& connection = resources.connection;
  auto& client     = *connection.context;
  connection.BeginTransaction();
  try {
    duckdb::SiriusContext::StandaloneQueryScope window(*resources.engine, client, "public_plan");
    // The existing Parquet scan planner needs DuckDB's table-function binding.
    // No Substrait translator, SQL parser, or DuckDB execution is used here.
    auto scan = connection.TableFunction("parquet_scan",
                                         {duckdb::Value(relation.path)},
                                         {{"hive_partitioning", duckdb::Value(false)}});
    duckdb::Planner binder(client);
    binder.CreatePlan(duckdb::make_uniq<duckdb::RelationStatement>(scan));
    if (binder.types.size() != relation.fields.size()) {
      throw std::invalid_argument("Parquet schema width differs from plan");
    }
    for (size_t i = 0; i < relation.fields.size(); ++i) {
      if (binder.types[i] != duckdb::LogicalType::BIGINT ||
          binder.names[i] != relation.fields[i].name) {
        throw std::invalid_argument("Parquet column names/types differ from plan");
      }
    }
    auto prepared = duckdb::make_shared_ptr<duckdb::PreparedStatementData>(
      duckdb::StatementType::SELECT_STATEMENT);
    prepared->names = binder.names;
    prepared->types = binder.types;
    sirius::planner::sirius_physical_plan_generator generator(client);
    generator.contract_provenance.host_export_available = false;
    auto physical               = generator.create_plan(std::move(binder.plan));
    const auto type             = sirius::logical_type::make(sirius::type_id::BIGINT);
    const std::array operations = {sirius::comparison_type::equal,
                                   sirius::comparison_type::not_equal,
                                   sirius::comparison_type::lt,
                                   sirius::comparison_type::le,
                                   sirius::comparison_type::gt,
                                   sirius::comparison_type::ge};
    for (const auto& predicate : relation.filters) {
      sirius::ast::comparison expression{operations.at(predicate.operation),
                                         std::make_unique<sirius::ast::node>(sirius::ast::reference(
                                           static_cast<uint32_t>(predicate.column), type)),
                                         std::make_unique<sirius::ast::node>(sirius::ast::constant(
                                           sirius::value{predicate.literal}, type))};
      auto filter = duckdb::make_uniq<sirius::op::sirius_physical_filter>(
        physical->types,
        std::make_unique<sirius::ast::node>(std::move(expression)),
        physical->estimated_cardinality);
      filter->children.push_back(std::move(physical));
      physical = std::move(filter);
    }
    auto statement = duckdb::make_shared_ptr<sirius::sirius_prepared_statement_data>(
      std::move(prepared), std::move(physical));
    auto collector = duckdb::make_uniq_base<sirius::op::sirius_physical_result_collector,
                                            sirius::op::sirius_physical_materialized_collector>(
      *statement, client);
    sirius::sirius_engine execution(client, window.query_id(), "public_plan");
    execution.initialize(std::move(collector));
    execution.execute();
    auto result = execution.get_result();
    if (!result) { throw std::runtime_error("Execution returned no result"); }
    result->ThrowIfError();
    window.finish();
    connection.Commit();
    return result;
  } catch (...) {
    try {
      connection.Rollback();
    } catch (...) { /* Preserve the query failure. */
    }
    throw;
  }
}
}  // namespace

sirius_status sirius_context_execute_plan(sirius_context* context,
                                          const sirius_plan* plan,
                                          ArrowArrayStream* out,
                                          sirius_error** error) noexcept
{
  if (out) { *out = {}; }
  return sirius::c_api::invoke(
    error,
    [&] {
      if (!context || !plan || !out) {
        return sirius::c_api::fail(
          SIRIUS_INVALID_ARGUMENT, "Expected context, plan and empty Arrow stream", error);
      }
      auto result       = std::make_unique<result_stream>();
      result->resources = std::make_unique<execution_resources>(context->engine);
      auto data         = execute(*result->resources, *plan->root);
      sirius::exec::export_arrow_result(std::move(data), result->inner, 65536);
      *out = {result_stream::schema,
              result_stream::next,
              result_stream::error,
              result_stream::release,
              result.get()};
      result.release();
      return sirius_status{SIRIUS_SUCCESS};
    },
    SIRIUS_EXECUTION_FAILURE);
}
