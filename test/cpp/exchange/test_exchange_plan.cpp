/*
 * Copyright 2026, Sirius Contributors.
 * SPDX-License-Identifier: Apache-2.0
 */

#include "exchange/exchange_plan.hpp"
#include "exchange/generated/sirius/exchange/v1/exchange.pb.h"
#include "substrait/plan.pb.h"

#include <catch.hpp>

namespace {

namespace exchange = sirius::exchange;

substrait::Plan source_plan()
{
  substrait::Plan plan;
  auto* root = plan.add_relations()->mutable_root();
  root->add_names("value");
  auto* read = root->mutable_input()->mutable_read();
  read->mutable_base_schema()->add_names("value");
  read->mutable_base_schema()->mutable_struct_()->add_types()->mutable_i64();
  exchange::v1::ExchangeSource source;
  source.mutable_query_id()->set_high(19);
  source.mutable_query_id()->set_low(27);
  source.mutable_receiver_fragment_id()->set_high(42);
  source.set_exchange_id(7);
  source.add_expected_sender_ids(0);
  source.add_expected_sender_ids(3);
  read->mutable_extension_table()->mutable_detail()->PackFrom(source);
  return plan;
}

substrait::Plan sink_plan(int count = 1)
{
  auto plan              = source_plan();
  auto* input            = plan.mutable_relations(0)->mutable_root()->mutable_input();
  auto child             = *input;
  auto* sink             = input->mutable_exchange();
  *sink->mutable_input() = std::move(child);
  sink->set_partition_count(count);
  if (count == 1) {
    sink->mutable_single_target()->mutable_expression()->mutable_literal()->set_i32(0);
  } else {
    sink->mutable_broadcast();
  }
  exchange::v1::ExchangeSink metadata;
  metadata.mutable_query_id()->set_high(19);
  metadata.mutable_query_id()->set_low(27);
  metadata.mutable_sender_fragment_id()->set_high(42);
  metadata.set_sender_id(3);
  sink->mutable_advanced_extension()->mutable_enhancement()->PackFrom(metadata);
  for (int partition = 0; partition < count; ++partition) {
    auto* target = sink->add_targets();
    target->add_partition_id(partition);
    exchange::v1::ExchangeDestination destination;
    destination.mutable_receiver_fragment_id()->set_high(81);
    destination.mutable_receiver_fragment_id()->set_low(partition);
    destination.set_exchange_id(9);
    destination.set_peer_id("peer_" + std::to_string(partition));
    target->mutable_extended()->PackFrom(destination);
  }
  return plan;
}

substrait::ExchangeRel* sink(substrait::Plan& plan)
{
  return plan.mutable_relations(0)->mutable_root()->mutable_input()->mutable_exchange();
}

substrait::Plan fetch_plan(bool exchange_source = true)
{
  auto plan   = source_plan();
  auto* input = plan.mutable_relations(0)->mutable_root()->mutable_input();
  if (!exchange_source) { input->mutable_read()->mutable_named_table()->add_names("ordinary"); }
  auto child                               = *input;
  *input->mutable_fetch()->mutable_input() = std::move(child);
  return plan;
}

substrait::FetchRel* fetch(substrait::Plan& plan)
{
  return plan.mutable_relations(0)->mutable_root()->mutable_input()->mutable_fetch();
}

std::int64_t fetch_scalar(const substrait::FetchRel& fetch, const char* name)
{
  return fetch.GetReflection()->GetInt64(fetch, fetch.GetDescriptor()->FindFieldByName(name));
}

void set_fetch_scalar(substrait::FetchRel& fetch, const char* name, std::int64_t value)
{
  fetch.GetReflection()->SetInt64(&fetch, fetch.GetDescriptor()->FindFieldByName(name), value);
}

exchange::plan parse(const substrait::Plan& plan)
{
  return exchange::rewrite_substrait(plan.SerializeAsString());
}

}  // namespace

TEST_CASE("exchange fetch expressions normalize for the bundled importer", "[exchange_plan]")
{
  auto plan                    = fetch_plan();
  std::int64_t expected_count  = -1;
  std::int64_t expected_offset = 0;
  SECTION("one row")
  {
    fetch(plan)->mutable_count_expr()->mutable_literal()->set_i64(1);
    expected_count = 1;
  }
  SECTION("zero rows")
  {
    fetch(plan)->mutable_count_expr()->mutable_literal()->set_i32(0);
    expected_count = 0;
  }
  SECTION("offset without count")
  {
    fetch(plan)->mutable_offset_expr()->mutable_literal()->set_i16(2);
    expected_offset = 2;
  }
  SECTION("absent count means all rows") {}
  SECTION("integer null count means all rows")
  {
    fetch(plan)->mutable_count_expr()->mutable_literal()->mutable_null()->mutable_i64();
  }
  SECTION("integer null offset means no skip")
  {
    fetch(plan)->mutable_offset_expr()->mutable_literal()->mutable_null()->mutable_i32();
  }
  SECTION("explicit legacy zero is preserved")
  {
    set_fetch_scalar(*fetch(plan), "count", 0);
    expected_count = 0;
  }
  SECTION("explicit legacy positive count and offset are preserved")
  {
    set_fetch_scalar(*fetch(plan), "count", 5);
    set_fetch_scalar(*fetch(plan), "offset", 3);
    expected_count  = 5;
    expected_offset = 3;
  }
  SECTION("legacy unbounded count is preserved") { set_fetch_scalar(*fetch(plan), "count", -1); }
  SECTION("small integer count")
  {
    fetch(plan)->mutable_count_expr()->mutable_literal()->set_i8(7);
    expected_count = 7;
  }
  substrait::Plan rewritten;
  REQUIRE(rewritten.ParseFromString(parse(plan).rewritten));
  const auto& result = rewritten.relations(0).root().input().fetch();
  REQUIRE(fetch_scalar(result, "count") == expected_count);
  REQUIRE(fetch_scalar(result, "offset") == expected_offset);
  REQUIRE_FALSE(result.has_count_expr());
  REQUIRE_FALSE(result.has_offset_expr());
}

TEST_CASE("exchange fetch rejects unsupported or invalid expressions", "[exchange_plan]")
{
  auto plan = fetch_plan();
  SECTION("computed count") { fetch(plan)->mutable_count_expr()->mutable_selection(); }
  SECTION("computed offset") { fetch(plan)->mutable_offset_expr()->mutable_scalar_function(); }
  SECTION("negative modern count")
  {
    fetch(plan)->mutable_count_expr()->mutable_literal()->set_i64(-1);
  }
  SECTION("negative modern offset")
  {
    fetch(plan)->mutable_offset_expr()->mutable_literal()->set_i64(-1);
  }
  SECTION("noninteger count") { fetch(plan)->mutable_count_expr()->mutable_literal()->set_fp64(1); }
  SECTION("narrow integer overflow")
  {
    fetch(plan)->mutable_count_expr()->mutable_literal()->set_i8(128);
  }
  SECTION("noninteger null")
  {
    fetch(plan)->mutable_count_expr()->mutable_literal()->mutable_null()->mutable_string();
  }
  SECTION("type variation")
  {
    auto* literal = fetch(plan)->mutable_count_expr()->mutable_literal();
    literal->set_i64(1);
    literal->set_type_variation_reference(1);
  }
  SECTION("invalid legacy count") { set_fetch_scalar(*fetch(plan), "count", -2); }
  SECTION("invalid legacy offset") { set_fetch_scalar(*fetch(plan), "offset", -1); }
  REQUIRE_THROWS(parse(plan));
}

TEST_CASE("ordinary fetch plans preserve their original representation", "[exchange_plan]")
{
  auto plan = fetch_plan(false);
  SECTION("modern count") { fetch(plan)->mutable_count_expr()->mutable_literal()->set_i64(1); }
  SECTION("unset count") {}
  SECTION("computed count") { fetch(plan)->mutable_count_expr()->mutable_selection(); }
  auto result = parse(plan);
  REQUIRE_FALSE(result.has_exchange());
  REQUIRE(result.rewritten == plan.SerializeAsString());
}

TEST_CASE("native exchange character types require a positive width", "[exchange_plan]")
{
  auto plan  = source_plan();
  auto* type = plan.mutable_relations(0)
                 ->mutable_root()
                 ->mutable_input()
                 ->mutable_read()
                 ->mutable_base_schema()
                 ->mutable_struct_()
                 ->mutable_types(0);
  SECTION("varchar") { type->mutable_varchar()->set_length(-1); }
  SECTION("fixed char") { type->mutable_fixed_char()->set_length(0); }
  REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("length must be positive"));
}

TEST_CASE("native exchange binary columns are rejected explicitly", "[exchange_plan]")
{
  auto plan = source_plan();
  plan.mutable_relations(0)
    ->mutable_root()
    ->mutable_input()
    ->mutable_read()
    ->mutable_base_schema()
    ->mutable_struct_()
    ->mutable_types(0)
    ->mutable_binary();
  REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("binary exchange columns"));
}

TEST_CASE("exchange source metadata rewrites to a declared stream", "[exchange_plan]")
{
  auto result = parse(source_plan());
  REQUIRE(result.inputs.size() == 1);
  REQUIRE(result.inputs[0].stream_id() == 7);
  REQUIRE(result.inputs[0].address.query_id.high == 19);
  REQUIRE(result.inputs[0].address.query_id.low == 27);
  REQUIRE(result.inputs[0].expected_senders == std::set<std::uint32_t>{0, 3});
  REQUIRE(result.inputs[0].types[0].id() == sirius::type_id::BIGINT);
  substrait::Plan rewritten;
  REQUIRE(rewritten.ParseFromString(result.rewritten));
  REQUIRE(rewritten.relations(0).root().input().read().named_table().names(0) == "sirius_stream_7");
}

TEST_CASE("exchange source rewrites preserve filters and scalar projection", "[exchange_plan]")
{
  auto plan  = source_plan();
  auto* read = plan.mutable_relations(0)->mutable_root()->mutable_input()->mutable_read();
  read->mutable_filter()->mutable_literal()->set_boolean(true);
  read->mutable_projection()->mutable_select()->add_struct_items()->set_field(0);
  substrait::Plan rewritten;
  REQUIRE(rewritten.ParseFromString(parse(plan).rewritten));
  const auto& result = rewritten.relations(0).root().input().read();
  REQUIRE(result.filter().SerializeAsString() == read->filter().SerializeAsString());
  REQUIRE(result.projection().SerializeAsString() == read->projection().SerializeAsString());
}

TEST_CASE("ordinary Substrait plans keep their original declarations", "[exchange_plan]")
{
  auto plan  = source_plan();
  auto* read = plan.mutable_relations(0)->mutable_root()->mutable_input()->mutable_read();
  read->mutable_named_table()->add_names("ordinary_table");
  auto result = parse(plan);
  REQUIRE_FALSE(result.has_exchange());
  REQUIRE(result.rewritten == plan.SerializeAsString());
}

TEST_CASE("exchange sources reject repeated reads of one stream", "[exchange_plan]")
{
  auto plan               = source_plan();
  auto* input             = plan.mutable_relations(0)->mutable_root()->mutable_input();
  auto child              = *input;
  auto* cross             = input->mutable_cross();
  *cross->mutable_left()  = child;
  *cross->mutable_right() = child;
  REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("read once"));
}

TEST_CASE("exchange sink routing preserves partition addresses", "[exchange_plan]")
{
  auto plan = sink_plan(2);
  sink(plan)->mutable_targets()->SwapElements(0, 1);
  auto result = parse(plan);
  REQUIRE(result.sink);
  REQUIRE(result.sink->mode == exchange::distribution::broadcast);
  REQUIRE(result.sink->targets[0].peer == "peer_0");
  REQUIRE(result.sink->targets[1].address.fragment_id.low == 1);
  REQUIRE(result.sink->sender_id == 3);
  substrait::Plan rewritten;
  REQUIRE(rewritten.ParseFromString(result.rewritten));
  REQUIRE(rewritten.relations(0).root().input().has_read());
  REQUIRE(parse(sink_plan()).sink->mode == exchange::distribution::gather);
}

TEST_CASE("exchange hash keys preserve column references", "[exchange_plan]")
{
  auto plan   = sink_plan(2);
  auto* field = sink(plan)->mutable_scatter_by_fields()->add_fields();
  field->mutable_root_reference();
  field->mutable_direct_reference()->mutable_struct_field()->set_field(0);
  auto result = parse(plan);
  REQUIRE(result.sink->mode == exchange::distribution::hash);
  REQUIRE(result.sink->hash_columns == std::vector<int>{0});
  field->mutable_direct_reference()
    ->mutable_struct_field()
    ->mutable_child()
    ->mutable_struct_field();
  REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("top-level"));
}

TEST_CASE("exchange metadata rejects missing identity and malformed payloads", "[exchange_plan]")
{
  auto plan    = source_plan();
  auto* detail = plan.mutable_relations(0)
                   ->mutable_root()
                   ->mutable_input()
                   ->mutable_read()
                   ->mutable_extension_table()
                   ->mutable_detail();
  SECTION("missing query")
  {
    exchange::v1::ExchangeSource source;
    source.mutable_receiver_fragment_id();
    source.set_exchange_id(0);
    source.add_expected_sender_ids(0);
    detail->PackFrom(source);
    REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("identities"));
  }
  SECTION("unknown metadata version")
  {
    detail->set_type_url("type.googleapis.com/sirius.exchange.v2.ExchangeSource");
    REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("unsupported metadata"));
  }
  SECTION("invalid protobuf")
  {
    detail->set_value("\xff");
    REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("malformed"));
  }
}

TEST_CASE("exchange targets require unique complete coverage", "[exchange_plan]")
{
  auto plan = sink_plan(2);
  SECTION("duplicate partition") { sink(plan)->mutable_targets(1)->set_partition_id(0, 0); }
  SECTION("out of range partition") { sink(plan)->mutable_targets(1)->set_partition_id(0, 2); }
  SECTION("implicit all partitions") { sink(plan)->mutable_targets(1)->clear_partition_id(); }
  SECTION("missing target") { sink(plan)->mutable_targets()->RemoveLast(); }
  SECTION("duplicate destination")
  {
    *sink(plan)->mutable_targets(1)->mutable_extended() = sink(plan)->targets(0).extended();
  }
  REQUIRE_THROWS(parse(plan));
}

TEST_CASE("exchange sources cannot mix fragment or query identities", "[exchange_plan]")
{
  auto plan = sink_plan();
  exchange::v1::ExchangeSink metadata;
  REQUIRE(sink(plan)->advanced_extension().enhancement().UnpackTo(&metadata));
  SECTION("query mismatch") { metadata.mutable_query_id()->set_high(999); }
  SECTION("fragment mismatch") { metadata.mutable_sender_fragment_id()->set_low(999); }
  sink(plan)->mutable_advanced_extension()->mutable_enhancement()->PackFrom(metadata);
  REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("same query and fragment"));
}

TEST_CASE("nested exchange sinks and unsupported routing are rejected", "[exchange_plan]")
{
  auto plan = sink_plan();
  SECTION("nested exchange")
  {
    auto* input = plan.mutable_relations(0)->mutable_root()->mutable_input();
    auto child  = *input;
    *input->mutable_filter()->mutable_input() = std::move(child);
    REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("root relation"));
  }
  SECTION("round robin")
  {
    sink(plan)->mutable_round_robin();
    REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("routing mode"));
  }
  SECTION("nonzero gather bucket")
  {
    sink(plan)->mutable_single_target()->mutable_expression()->mutable_literal()->set_i32(1);
    REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("constant zero"));
  }
  SECTION("sink emit mapping")
  {
    sink(plan)->mutable_common()->mutable_emit()->add_output_mapping(0);
    REQUIRE_THROWS_WITH(parse(plan), Catch::Matchers::ContainsSubstring("emit mappings"));
  }
}

TEST_CASE("exchange source rejects invalid schema and sender declarations", "[exchange_plan]")
{
  auto plan  = source_plan();
  auto* read = plan.mutable_relations(0)->mutable_root()->mutable_input()->mutable_read();
  SECTION("schema column count") { read->mutable_base_schema()->add_names("extra"); }
  SECTION("nested type")
  {
    read->mutable_base_schema()->mutable_struct_()->mutable_types(0)->mutable_list();
  }
  SECTION("additional enhancement")
  {
    read->mutable_common()->mutable_advanced_extension()->mutable_enhancement()->set_type_url(
      "other");
  }
  SECTION("source emit mapping") { read->mutable_common()->mutable_emit()->add_output_mapping(0); }
  SECTION("invalid projection")
  {
    read->mutable_projection()->mutable_select()->add_struct_items()->set_field(1);
  }
  SECTION("no senders")
  {
    exchange::v1::ExchangeSource source;
    read->extension_table().detail().UnpackTo(&source);
    source.clear_expected_sender_ids();
    read->mutable_extension_table()->mutable_detail()->PackFrom(source);
  }
  SECTION("duplicate senders")
  {
    exchange::v1::ExchangeSource source;
    read->extension_table().detail().UnpackTo(&source);
    source.add_expected_sender_ids(0);
    read->mutable_extension_table()->mutable_detail()->PackFrom(source);
  }
  REQUIRE_THROWS(parse(plan));
}
