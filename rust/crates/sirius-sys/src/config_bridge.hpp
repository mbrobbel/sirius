// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#pragma once

#include <sirius/context/config_builder.hpp>

#include <memory>
#include <string>

namespace sirius::rust_bridge {

struct BuilderResult;
struct ConfigResult;

std::unique_ptr<ContextConfigBuilder> config_builder_defaults();
BuilderResult config_builder_from_yaml(const std::string& path);
std::unique_ptr<ContextConfigBuilder> config_builder_copy(const ContextConfigBuilder& builder);
ConfigResult config_build(const ContextConfigBuilder& builder);
std::unique_ptr<ContextConfig> config_copy(const ContextConfig& config);

}  // namespace sirius::rust_bridge
