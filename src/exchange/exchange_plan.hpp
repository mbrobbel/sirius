/*
 * Copyright 2026, Sirius Contributors.
 * SPDX-License-Identifier: Apache-2.0
 */

#pragma once

#include "helper/logical_type.hpp"

#include <compare>
#include <cstdint>
#include <optional>
#include <set>
#include <string>
#include <vector>

namespace sirius::exchange {

struct id128 {
  std::uint64_t high{};
  std::uint64_t low{};
  auto operator<=>(const id128&) const = default;
};

struct route {
  id128 query_id;
  id128 fragment_id;
  std::uint32_t exchange_id{};
  auto operator<=>(const route&) const = default;
};

struct input {
  route address;
  std::vector<std::string> names;
  std::vector<sirius::logical_type> types;
  std::set<std::uint32_t> expected_senders;

  [[nodiscard]] std::uint64_t stream_id() const { return address.exchange_id; }
};

struct destination {
  route address;
  std::string peer;
};

enum class distribution { gather, hash, broadcast };

struct output {
  id128 query_id;
  id128 fragment_id;
  std::uint32_t sender_id{};
  distribution mode{distribution::gather};
  std::vector<int> hash_columns;
  /// Positional: targets[i] receives partition i, via local output stream i.
  std::vector<destination> targets;
};

struct plan {
  std::string rewritten;
  std::vector<input> inputs;
  std::optional<output> sink;

  [[nodiscard]] bool has_exchange() const { return sink.has_value() || !inputs.empty(); }
};

/// Parse exchange declarations and rewrite their boundaries for DuckDB's Substrait importer.
/// Reject unsupported exchange semantics before any fragment is started.
plan rewrite_substrait(const std::string& serialized);

}  // namespace sirius::exchange
