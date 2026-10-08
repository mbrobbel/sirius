// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#include "context_bridge.hpp"

#include "sirius-sys/src/context.rs.h"

#include <stdexcept>
#include <utility>

namespace sirius::rust_bridge {

ContextResult context_create(const ContextConfig& config)
{
  auto result = Context::create(config);
  if (result) { return {std::move(*result), {}, {}}; }
  auto code = ContextErrorCode::Initialization;
  switch (result.error().code) {
    case ErrorCode::context_in_use: code = ContextErrorCode::InUse; break;
    case ErrorCode::context_initialization: break;
    default: throw std::runtime_error("Unexpected Sirius context error code");
  }
  return {nullptr, code, rust::String::lossy(result.error().message)};
}

}  // namespace sirius::rust_bridge
