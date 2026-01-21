/*
 * Copyright 2025, Sirius Contributors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#pragma once

// Temporarily disable spdlog due to fmt version conflicts between DuckDB (fmt 6.x)
// and vcpkg's spdlog (expects fmt 12.x). Using no-op macros for now.
// TODO: Re-enable when the fmt version conflict is resolved.

#define SIRIUS_LOG_TRACE(...) ((void)0)
#define SIRIUS_LOG_DEBUG(...) ((void)0)
#define SIRIUS_LOG_INFO(...)  ((void)0)
#define SIRIUS_LOG_WARN(...)  ((void)0)
#define SIRIUS_LOG_ERROR(...) ((void)0)
#define SIRIUS_LOG_FATAL(...) ((void)0)

#ifndef __CUDACC__

#include <chrono>
#include <cstdlib>
#include <memory>
#include <optional>
#include <string>

namespace duckdb {

inline constexpr int SIRIUS_LOG_FLUSH_SEC = 3;

inline std::optional<std::string> GetEnvVar(const std::string& name)
{
  const char* val = std::getenv(name.c_str());
  if (val) {
    return std::string(val);
  } else {
    return std::nullopt;
  }
}

inline std::string GetLogDir()
{
  auto log_dir_str = GetEnvVar("SIRIUS_LOG_DIR");
  if (log_dir_str.has_value()) { return *log_dir_str; }
  return SIRIUS_DEFAULT_LOG_DIR;
}

// No-op logger initialization - spdlog disabled due to fmt conflicts
inline void InitGlobalLogger(std::string log_file = "")
{
  (void)log_file;  // Unused
}

}  // namespace duckdb

#endif  // __CUDACC__
