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

#include "sirius_ffi.hpp"

#include <cucascade/data/data_batch.hpp>
#include <cucascade/data/data_repository.hpp>

#include <memory>

namespace sirius::ffi::detail {

/// Internal boundary between Sirius's opaque FFI handles and cuCascade.
///
/// Native integration code uses these conversions; Rust includes only
/// `sirius_ffi.hpp` and never sees a cuCascade type.
struct CuCascadeAdapter {
  [[nodiscard]] static std::unique_ptr<DataBatch> wrap_data_batch(
    std::shared_ptr<cucascade::data_batch> batch);
  [[nodiscard]] static std::shared_ptr<cucascade::data_batch> native_data_batch(
    const DataBatch& batch);

  [[nodiscard]] static std::unique_ptr<DataRepository> wrap_data_repository(
    std::shared_ptr<cucascade::shared_data_repository> repository);
  [[nodiscard]] static std::shared_ptr<cucascade::shared_data_repository> native_data_repository(
    const DataRepository& repository);
};

}  // namespace sirius::ffi::detail
