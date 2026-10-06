/*
 * Copyright 2026, Sirius Contributors.
 * SPDX-License-Identifier: Apache-2.0
 */

#pragma once

#include "op/sirius_physical_streaming_sink.hpp"
#include "op/sirius_physical_streaming_source.hpp"

#include <utility>

namespace sirius::op {

/// Network input; the exchange executor feeds the inherited sender-tracked stream.
class sirius_physical_exchange_source final : public sirius_physical_streaming_source {
 public:
  static constexpr auto TYPE = SiriusPhysicalOperatorType::EXCHANGE_SOURCE;

  sirius_physical_exchange_source(
    duckdb::vector<sirius::logical_type> types,
    std::size_t estimated_cardinality,
    std::shared_ptr<cucascade::shared_data_repository> repository,
    std::set<exec::sender_id_t> expected_senders,
    std::shared_ptr<transparent::read_view_registry> read_views = nullptr,
    scan::scan_contract_id contract_id                          = 0)
    : sirius_physical_streaming_source(std::move(types),
                                       estimated_cardinality,
                                       std::move(repository),
                                       std::move(expected_senders),
                                       std::move(read_views),
                                       contract_id)
  {
    type = TYPE;
  }
};

/// Network output; routing uses the streaming sink's GPU partitioning and lifecycle.
class sirius_physical_exchange_sink final : public sirius_physical_streaming_sink {
 public:
  static constexpr auto TYPE = SiriusPhysicalOperatorType::EXCHANGE_SINK;

  sirius_physical_exchange_sink(duckdb::vector<sirius::logical_type> types,
                                std::size_t estimated_cardinality,
                                std::shared_ptr<cucascade::shared_data_repository> repository)
    : sirius_physical_streaming_sink(std::move(types), estimated_cardinality, std::move(repository))
  {
    type = TYPE;
  }

  sirius_physical_exchange_sink(
    duckdb::vector<sirius::logical_type> types,
    std::size_t estimated_cardinality,
    std::vector<std::shared_ptr<cucascade::shared_data_repository>> repositories,
    partition_spec partitioning)
    : sirius_physical_streaming_sink(
        std::move(types), estimated_cardinality, std::move(repositories), std::move(partitioning))
  {
    type = TYPE;
  }
};

}  // namespace sirius::op
