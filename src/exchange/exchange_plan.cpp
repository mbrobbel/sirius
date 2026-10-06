/*
 * Copyright 2026, Sirius Contributors.
 * SPDX-License-Identifier: Apache-2.0
 */

#include "exchange/exchange_plan.hpp"

#include "exchange/generated/sirius/exchange/v1/exchange.pb.h"
#include "sirius/exception.hpp"
#include "substrait/plan.pb.h"

#include <google/protobuf/descriptor.h>
#include <google/protobuf/message.h>

#include <algorithm>
#include <limits>
#include <set>
#include <string_view>
#include <utility>

namespace sirius::exchange {
namespace {

namespace protobuf                    = duckdb::google::protobuf;
constexpr std::string_view source_url = "type.googleapis.com/sirius.exchange.v1.ExchangeSource";
constexpr std::string_view sink_url   = "type.googleapis.com/sirius.exchange.v1.ExchangeSink";
constexpr std::string_view destination_url =
  "type.googleapis.com/sirius.exchange.v1.ExchangeDestination";

[[noreturn]] void invalid(const std::string& message)
{
  throw sirius::invalid_input_exception("Substrait exchange: " + message);
}

template <typename T>
T metadata(const protobuf::Any& any, std::string_view expected)
{
  if (any.type_url() != expected) { invalid("unsupported metadata type " + any.type_url()); }
  T result;
  if (!result.ParseFromString(any.value())) { invalid("malformed " + std::string(expected)); }
  return result;
}

id128 identity(const v1::Id128& id) { return {id.high(), id.low()}; }

// Unknown type variations can change the representation of otherwise familiar scalar types.
void plain_type(const protobuf::Message& type)
{
  auto* field = type.GetDescriptor()->FindFieldByName("type_variation_reference");
  if (field && type.GetReflection()->GetUInt32(type, field) != 0) {
    invalid("source schema type variations are unsupported");
  }
}

logical_type column_type(const substrait::Type& type)
{
  auto* kind = type.GetReflection()->GetOneofFieldDescriptor(
    type, type.GetDescriptor()->FindOneofByName("kind"));
  if (!kind || kind->cpp_type() != protobuf::FieldDescriptor::CPPTYPE_MESSAGE) {
    invalid("source schema needs a supported scalar type");
  }
  plain_type(type.GetReflection()->GetMessage(type, kind));
  switch (type.kind_case()) {
    case substrait::Type::kBool: return logical_type::make(type_id::BOOLEAN);
    case substrait::Type::kI8: return logical_type::make(type_id::TINYINT);
    case substrait::Type::kI16: return logical_type::make(type_id::SMALLINT);
    case substrait::Type::kI32: return logical_type::make(type_id::INTEGER);
    case substrait::Type::kI64: return logical_type::make(type_id::BIGINT);
    case substrait::Type::kFp32: return logical_type::make(type_id::FLOAT);
    case substrait::Type::kFp64: return logical_type::make(type_id::DOUBLE);
    case substrait::Type::kString: return logical_type::make(type_id::VARCHAR);
    case substrait::Type::kVarchar:
      if (type.varchar().length() < 1) { invalid("source varchar length must be positive"); }
      return logical_type::make(type_id::VARCHAR);
    case substrait::Type::kFixedChar:
      if (type.fixed_char().length() < 1) { invalid("source fixed-char length must be positive"); }
      return logical_type::make(type_id::VARCHAR);
    case substrait::Type::kBinary: invalid("binary exchange columns are unsupported by Sirius");
    case substrait::Type::kDate: return logical_type::make(type_id::DATE);
    case substrait::Type::kPrecisionTimestamp:
      switch (type.precision_timestamp().precision()) {
        case 0: return logical_type::make(type_id::TIMESTAMP_SEC);
        case 3: return logical_type::make(type_id::TIMESTAMP_MS);
        case 6: return logical_type::make(type_id::TIMESTAMP);
        case 9: return logical_type::make(type_id::TIMESTAMP_NS);
        default: invalid("unsupported source timestamp precision");
      }
    case substrait::Type::kDecimal: {
      const auto& decimal = type.decimal();
      if (decimal.precision() < 1 || decimal.precision() > 38 || decimal.scale() < 0 ||
          decimal.scale() > decimal.precision()) {
        invalid("invalid source decimal precision or scale");
      }
      return logical_type::make_decimal(decimal.precision(), decimal.scale());
    }
    default: invalid("source schema needs a supported scalar type");
  }
}

void check_common(const substrait::RelCommon& common)
{
  if (common.has_emit()) { invalid("sink emit mappings are unsupported; project before exchange"); }
  if (common.has_advanced_extension() && common.advanced_extension().has_enhancement()) {
    invalid("additional relation enhancements are unsupported");
  }
}

input source(substrait::ReadRel& read)
{
  auto declared = metadata<v1::ExchangeSource>(read.extension_table().detail(), source_url);
  if (!declared.has_query_id() || !declared.has_receiver_fragment_id() ||
      !declared.has_exchange_id()) {
    invalid("source requires query, receiver fragment, and exchange identities");
  }
  if (read.has_advanced_extension() && read.advanced_extension().has_enhancement()) {
    invalid("additional source enhancements are unsupported");
  }
  if (read.common().advanced_extension().has_enhancement()) {
    invalid("additional source relation enhancements are unsupported");
  }
  if (read.common().has_emit()) {
    invalid("source emit mappings are unsupported; project above the read");
  }
  if (!read.has_base_schema() || !read.base_schema().has_struct_()) {
    invalid("source requires base_schema");
  }
  const auto& schema = read.base_schema();
  plain_type(schema.struct_());
  if (schema.names_size() == 0 || schema.names_size() != schema.struct_().types_size()) {
    invalid("source schema requires one name per scalar column");
  }
  if (read.has_projection()) {
    if (!read.projection().has_select() || read.projection().select().struct_items_size() == 0) {
      invalid("source projection requires top-level column selections");
    }
    for (const auto& item : read.projection().select().struct_items()) {
      if (item.has_child() || item.field() < 0 || item.field() >= schema.names_size()) {
        invalid("source projection requires valid top-level column selections");
      }
    }
  }
  input result;
  result.address = {identity(declared.query_id()),
                    identity(declared.receiver_fragment_id()),
                    declared.exchange_id()};
  for (int index = 0; index < schema.names_size(); ++index) {
    if (schema.names(index).empty()) { invalid("source column names must not be empty"); }
    result.names.push_back(schema.names(index));
    result.types.push_back(column_type(schema.struct_().types(index)));
  }
  for (auto sender : declared.expected_sender_ids()) {
    if (!result.expected_senders.insert(sender).second) {
      invalid("source repeats an expected sender id");
    }
  }
  if (result.expected_senders.empty()) { invalid("source requires expected sender ids"); }
  read.mutable_named_table()->add_names("sirius_stream_" + std::to_string(result.stream_id()));
  return result;
}

int hash_column(const substrait::Expression::FieldReference& field)
{
  if (!field.has_root_reference() || !field.has_direct_reference() ||
      !field.direct_reference().has_struct_field() ||
      field.direct_reference().struct_field().has_child() ||
      field.direct_reference().struct_field().field() < 0) {
    invalid("hash exchange keys must be direct top-level column references");
  }
  return field.direct_reference().struct_field().field();
}

output sink(const substrait::ExchangeRel& exchange)
{
  if (!exchange.has_input()) { invalid("sink requires an input relation"); }
  check_common(exchange.common());
  auto declared = metadata<v1::ExchangeSink>(exchange.advanced_extension().enhancement(), sink_url);
  if (!declared.has_query_id() || !declared.has_sender_fragment_id() || !declared.has_sender_id()) {
    invalid("sink requires query, sender fragment, and sender identities");
  }
  const auto count = exchange.partition_count();
  if (count < 1 || exchange.targets_size() != count) {
    invalid("sink requires one target per positive partition count");
  }
  output result;
  result.query_id    = identity(declared.query_id());
  result.fragment_id = identity(declared.sender_fragment_id());
  result.sender_id   = declared.sender_id();
  result.targets.resize(count);
  std::set<int> partitions;
  std::set<route> addresses;
  for (const auto& target : exchange.targets()) {
    if (target.partition_id_size() != 1 || target.partition_id(0) < 0 ||
        target.partition_id(0) >= count || !partitions.insert(target.partition_id(0)).second) {
      invalid("targets must uniquely cover every partition exactly once");
    }
    auto endpoint = metadata<v1::ExchangeDestination>(target.extended(), destination_url);
    if (!endpoint.has_receiver_fragment_id() || !endpoint.has_exchange_id() ||
        endpoint.peer_id().empty()) {
      invalid("destination requires receiver fragment, exchange id, and peer id");
    }
    destination destination{
      {result.query_id, identity(endpoint.receiver_fragment_id()), endpoint.exchange_id()},
      endpoint.peer_id()};
    if (!addresses.insert(destination.address).second) {
      invalid("sink repeats a destination address");
    }
    result.targets[target.partition_id(0)] = std::move(destination);
  }
  switch (exchange.exchange_kind_case()) {
    case substrait::ExchangeRel::kSingleTarget: {
      const auto& expression = exchange.single_target().expression();
      if (count != 1 || !expression.has_literal() ||
          !((expression.literal().has_i32() && expression.literal().i32() == 0) ||
            (expression.literal().has_i64() && expression.literal().i64() == 0)) ||
          expression.literal().type_variation_reference() != 0) {
        invalid("gather requires a constant zero bucket and one target");
      }
      result.mode = distribution::gather;
      break;
    }
    case substrait::ExchangeRel::kBroadcast:
      result.mode = count == 1 ? distribution::gather : distribution::broadcast;
      break;
    case substrait::ExchangeRel::kScatterByFields:
      if (exchange.scatter_by_fields().fields_size() == 0) {
        invalid("hash exchange requires at least one key");
      }
      for (const auto& field : exchange.scatter_by_fields().fields()) {
        result.hash_columns.push_back(hash_column(field));
      }
      result.mode = count == 1 ? distribution::gather : distribution::hash;
      break;
    default: invalid("unsupported exchange routing mode");
  }
  return result;
}

std::int64_t fetch_literal(const substrait::Expression& expression,
                           std::int64_t null_default,
                           const char* field)
{
  if (!expression.has_literal() || expression.literal().type_variation_reference() != 0) {
    invalid(std::string("fetch ") + field + " requires a constant integer");
  }
  const auto& literal = expression.literal();
  if (literal.has_null()) {
    const auto& type = literal.null();
    if (!(type.has_i8() || type.has_i16() || type.has_i32() || type.has_i64())) {
      invalid(std::string("fetch ") + field + " NULL requires an integer type");
    }
    auto* kind = type.GetReflection()->GetOneofFieldDescriptor(
      type, type.GetDescriptor()->FindOneofByName("kind"));
    plain_type(type.GetReflection()->GetMessage(type, kind));
    return null_default;
  }
  std::int64_t value;
  if (literal.has_i8()) {
    value = literal.i8();
    if (value > std::numeric_limits<std::int8_t>::max()) {
      invalid(std::string("fetch ") + field + " exceeds its integer width");
    }
  } else if (literal.has_i16()) {
    value = literal.i16();
    if (value > std::numeric_limits<std::int16_t>::max()) {
      invalid(std::string("fetch ") + field + " exceeds its integer width");
    }
  } else if (literal.has_i32()) {
    value = literal.i32();
  } else if (literal.has_i64()) {
    value = literal.i64();
  } else {
    invalid(std::string("fetch ") + field + " requires a constant integer");
  }
  if (value < 0) { invalid(std::string("fetch ") + field + " must be nonnegative"); }
  return value;
}

// The bundled importer reads legacy scalar fields; normalize modern constant expressions here.
void normalize_fetch(substrait::FetchRel& fetch)
{
  auto* reflection   = fetch.GetReflection();
  auto* count_field  = fetch.GetDescriptor()->FindFieldByName("count");
  auto* offset_field = fetch.GetDescriptor()->FindFieldByName("offset");
  if (fetch.has_count_expr()) {
    const auto count = fetch_literal(fetch.count_expr(), -1, "count");
    reflection->SetInt64(&fetch, count_field, count);
  } else if (fetch.count_mode_case() == substrait::FetchRel::COUNT_MODE_NOT_SET) {
    reflection->SetInt64(&fetch, count_field, -1);
  } else if (reflection->GetInt64(fetch, count_field) < -1) {
    invalid("legacy fetch count must be nonnegative or -1");
  }
  if (fetch.has_offset_expr()) {
    const auto offset = fetch_literal(fetch.offset_expr(), 0, "offset");
    reflection->SetInt64(&fetch, offset_field, offset);
  } else if (reflection->GetInt64(fetch, offset_field) < 0) {
    invalid("legacy fetch offset must be nonnegative");
  }
}

// Reflection includes relations inside expressions, so unsupported nested exchanges cannot hide
// in a subquery or in a relation added by a newer Substrait schema.
void rewrite(protobuf::Message& message,
             plan& result,
             std::vector<substrait::FetchRel*>& fetches,
             int depth = 0)
{
  if (depth > 100) { invalid("plan nesting exceeds 100 messages"); }
  if (message.GetDescriptor() == substrait::ExchangeRel::descriptor()) {
    invalid("only a fragment's root relation may be an exchange sink");
  }
  if (message.GetDescriptor() == substrait::ReadRel::descriptor()) {
    auto& read = static_cast<substrait::ReadRel&>(message);
    if (read.has_extension_table()) { result.inputs.push_back(source(read)); }
  }
  if (message.GetDescriptor() == substrait::FetchRel::descriptor()) {
    fetches.push_back(&static_cast<substrait::FetchRel&>(message));
  }
  const auto* reflection = message.GetReflection();
  std::vector<const protobuf::FieldDescriptor*> fields;
  reflection->ListFields(message, &fields);
  for (auto* field : fields) {
    if (field->cpp_type() != protobuf::FieldDescriptor::CPPTYPE_MESSAGE) { continue; }
    if (field->is_repeated()) {
      const int count = reflection->FieldSize(message, field);
      for (int index = 0; index < count; ++index) {
        rewrite(
          *reflection->MutableRepeatedMessage(&message, field, index), result, fetches, depth + 1);
      }
    } else {
      rewrite(*reflection->MutableMessage(&message, field), result, fetches, depth + 1);
    }
  }
}

}  // namespace

plan rewrite_substrait(const std::string& serialized)
{
  substrait::Plan parsed;
  if (!parsed.ParseFromString(serialized)) { invalid("malformed plan protobuf"); }
  plan result;
  if (parsed.relations_size() == 1 && parsed.relations(0).has_root()) {
    auto* root = parsed.mutable_relations(0)->mutable_root()->mutable_input();
    if (root->has_exchange()) {
      result.sink          = sink(root->exchange());
      substrait::Rel child = root->exchange().input();
      root->Swap(&child);
    }
  }
  std::vector<substrait::FetchRel*> fetches;
  rewrite(parsed, result, fetches);
  if (result.has_exchange()) {
    if (parsed.relations_size() != 1 || !parsed.relations(0).has_root()) {
      invalid("exchange fragments require exactly one root");
    }
    auto query = result.sink ? result.sink->query_id : result.inputs.front().address.query_id;
    auto fragment =
      result.sink ? result.sink->fragment_id : result.inputs.front().address.fragment_id;
    std::set<std::uint64_t> streams;
    for (const auto& source : result.inputs) {
      if (source.address.query_id != query || source.address.fragment_id != fragment) {
        invalid("all boundaries must belong to the same query and fragment");
      }
      if (!streams.insert(source.stream_id()).second) {
        invalid("a source stream may only be read once");
      }
    }
    for (auto* fetch : fetches) {
      normalize_fetch(*fetch);
    }
  }
  if (!parsed.SerializeToString(&result.rewritten)) {
    invalid("could not serialize rewritten plan");
  }
  return result;
}

}  // namespace sirius::exchange
