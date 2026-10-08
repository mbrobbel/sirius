// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#include "context_bridge.hpp"

#include "sirius-sys/src/context.rs.h"

#include <stdexcept>
#include <utility>

namespace sirius::rust_bridge {

ContextResult context_create(const ContextConfig& config)
{
  auto result = Context::create(config);
  if (result) { return {std::move(*result), {}}; }
  if (result.error().code != ErrorCode::context_initialization) {
    throw std::runtime_error("Unexpected Sirius context error code");
  }
  return {nullptr, rust::String::lossy(result.error().message)};
}

}  // namespace sirius::rust_bridge
