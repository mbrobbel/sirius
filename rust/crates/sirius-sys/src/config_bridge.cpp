// Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
#include "config_bridge.hpp"

#include "sirius-sys/src/config.rs.h"

#include <stdexcept>

namespace sirius::rust_bridge {
namespace {

ConfigErrorCode error_code(ErrorCode code)
{
  switch (code) {
    case ErrorCode::configuration_io: return ConfigErrorCode::Io;
    case ErrorCode::malformed_yaml: return ConfigErrorCode::MalformedYaml;
    case ErrorCode::invalid_configuration: return ConfigErrorCode::InvalidConfiguration;
    default: break;
  }
  throw std::runtime_error("Unknown Sirius configuration error code");
}

}  // namespace

std::unique_ptr<ContextConfigBuilder> config_builder_defaults()
{
  return std::make_unique<ContextConfigBuilder>();
}

BuilderResult config_builder_from_yaml(const std::string& path)
{
  // A native path cannot contain NUL; reject it before filesystem calls truncate it.
  if (path.find('\0') != std::string::npos) {
    return {nullptr, ConfigErrorCode::InvalidPath, "Configuration path contains a NUL byte"};
  }
  auto result = ContextConfigBuilder::from_yaml(std::filesystem::path(path));
  if (!result) {
    return {nullptr, error_code(result.error().code), rust::String::lossy(result.error().message)};
  }
  return {std::make_unique<ContextConfigBuilder>(*result), {}, {}};
}

std::unique_ptr<ContextConfigBuilder> config_builder_copy(const ContextConfigBuilder& builder)
{
  return std::make_unique<ContextConfigBuilder>(builder);
}

ConfigResult config_build(const ContextConfigBuilder& builder)
{
  auto result = builder.build();
  if (!result) {
    return {nullptr, error_code(result.error().code), rust::String::lossy(result.error().message)};
  }
  return {std::make_unique<ContextConfig>(*result), {}, {}};
}

std::unique_ptr<ContextConfig> config_copy(const ContextConfig& config)
{
  return std::make_unique<ContextConfig>(config);
}

}  // namespace sirius::rust_bridge
