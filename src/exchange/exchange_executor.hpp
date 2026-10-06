/*
 * Copyright 2026, Sirius Contributors.
 * SPDX-License-Identifier: Apache-2.0
 */

#pragma once

#include "exchange/exchange_plan.hpp"

#include <chrono>
#include <cstddef>
#include <exception>
#include <memory>
#include <string>
#include <vector>

namespace duckdb {
class SiriusContext;
}
namespace sirius::exec {
class stream_session;
}

namespace sirius::exchange {

/// Context-owned NIXL agent and transport worker. The context must outlive this object.
class exchange_executor {
 public:
  exchange_executor(duckdb::SiriusContext& context,
                    std::string name,
                    std::size_t staging_bytes,
                    std::chrono::milliseconds timeout);
  ~exchange_executor();
  exchange_executor(const exchange_executor&)            = delete;
  exchange_executor& operator=(const exchange_executor&) = delete;

  [[nodiscard]] std::string metadata() const;
  /// Import a peer's opaque metadata before attaching a fragment; returns its agent name.
  std::string add_peer(const std::string& metadata);

  /// Borrow the built session until finish() or cancel() returns. Only one fragment at a time.
  void attach(const plan& spec,
              exec::stream_session& session,
              const std::vector<sirius::logical_type>& output_types);
  /// Arm peer-progress deadlines immediately before local engine execution.
  void start();
  /// After local execution succeeds, discard unused input, wait for acknowledged output EOS
  /// and every input sender, then release the borrowed session.
  void finish();
  /// Poison the session, notify peers, and wait until the worker releases the borrowed session.
  void cancel(std::exception_ptr error) noexcept;

 private:
  struct impl;
  std::unique_ptr<impl> _impl;
};

}  // namespace sirius::exchange
