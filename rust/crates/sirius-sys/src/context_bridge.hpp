// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#pragma once

#include <sirius/context/context.hpp>

namespace sirius::rust_bridge {

struct ContextResult;
ContextResult context_create(const ContextConfig& config);

}  // namespace sirius::rust_bridge
