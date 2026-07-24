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

// CPU-only coverage for the Sirius-owned cuCascade FFI adapter.

#include "catch.hpp"
#include "ffi/cucascade_adapter.hpp"

#include <cucascade/data/common.hpp>
#include <cucascade/memory/config.hpp>
#include <cucascade/memory/null_device_memory_resource.hpp>

#include <cuda/memory_resource>

#include <memory>

namespace {

struct test_memory_space_holder {
  std::shared_ptr<cucascade::memory::memory_space> space;

  test_memory_space_holder()
  {
    cucascade::memory::host_memory_space_config config;
    config.numa_id                    = -1;
    config.memory_capacity            = 1024;
    config.initial_number_pools       = 0;
    config.reservation_limit_fraction = 1.0;
    config.mr_factory_fn = [](int, std::size_t) {
      return cuda::mr::any_resource<cuda::mr::device_accessible>{
        cucascade::memory::null_device_memory_resource{}};
    };
    space = std::make_shared<cucascade::memory::memory_space>(config);
  }
};

class test_representation : private test_memory_space_holder,
                            public cucascade::idata_representation {
 public:
  explicit test_representation(std::size_t size)
    : test_memory_space_holder(), cucascade::idata_representation(*space), size_(size)
  {
  }

  std::size_t get_size_in_bytes() const override { return size_; }
  std::size_t get_uncompressed_data_size_in_bytes() const override { return size_; }

  std::unique_ptr<cucascade::idata_representation> clone(
    rmm::cuda_stream_view /*stream*/) override
  {
    return std::make_unique<test_representation>(size_);
  }

 private:
  std::size_t size_;
};

}  // namespace

TEST_CASE("cuCascade FFI adapter transfers batches through a shared repository",
          "[ffi][cucascade_adapter]")
{
  using sirius::ffi::detail::CuCascadeAdapter;

  auto native_repository = std::make_shared<cucascade::shared_data_repository>();
  auto repository        = CuCascadeAdapter::wrap_data_repository(native_repository);
  REQUIRE(CuCascadeAdapter::native_data_repository(*repository) == native_repository);

  repository->set_num_partitions(2);
  REQUIRE(repository->num_partitions() == 2);
  REQUIRE(repository->total_size() == 0);

  auto native_batch =
    cucascade::data_batch::make(42, std::make_unique<test_representation>(128));
  auto batch = CuCascadeAdapter::wrap_data_batch(native_batch);
  REQUIRE(batch->id() == 42);
  REQUIRE(CuCascadeAdapter::native_data_batch(*batch) == native_batch);

  repository->push(std::move(batch), 1);
  REQUIRE(repository->size(1) == 1);
  REQUIRE(repository->total_size() == 1);

  auto popped = repository->pop_next(1);
  REQUIRE(popped);
  REQUIRE(popped->id() == 42);
  REQUIRE(CuCascadeAdapter::native_data_batch(*popped) == native_batch);
  REQUIRE(repository->size(1) == 0);
  REQUIRE_FALSE(repository->pop_next(1));
}

TEST_CASE("cuCascade FFI adapter rejects invalid handles", "[ffi][cucascade_adapter]")
{
  using sirius::ffi::detail::CuCascadeAdapter;

  REQUIRE_THROWS_AS(CuCascadeAdapter::wrap_data_batch(nullptr), std::invalid_argument);
  REQUIRE_THROWS_AS(CuCascadeAdapter::wrap_data_repository(nullptr), std::invalid_argument);

  auto repository = sirius::ffi::make_data_repository();
  REQUIRE_THROWS_AS(repository->push(nullptr, 0), std::invalid_argument);
  REQUIRE_THROWS_AS(repository->size(1), std::out_of_range);
}
