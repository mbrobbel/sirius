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

#include "ffi/cucascade_adapter.hpp"

#include <stdexcept>
#include <utility>

namespace sirius::ffi {

struct DataBatch::Impl {
  explicit Impl(std::shared_ptr<cucascade::data_batch> native_batch)
    : batch(std::move(native_batch))
  {
    if (!batch) { throw std::invalid_argument("cannot wrap a null cuCascade data batch"); }
  }

  std::shared_ptr<cucascade::data_batch> batch;
};

DataBatch::DataBatch(std::unique_ptr<Impl> impl) : impl_(std::move(impl)) {}

DataBatch::~DataBatch() = default;

std::uint64_t DataBatch::id() const noexcept { return impl_->batch->get_batch_id(); }

struct DataRepository::Impl {
  explicit Impl(std::shared_ptr<cucascade::shared_data_repository> native_repository)
    : repository(std::move(native_repository))
  {
    if (!repository) { throw std::invalid_argument("cannot wrap a null cuCascade repository"); }
  }

  std::shared_ptr<cucascade::shared_data_repository> repository;
};

DataRepository::DataRepository(std::unique_ptr<Impl> impl) : impl_(std::move(impl)) {}

DataRepository::~DataRepository() = default;

void DataRepository::push(std::unique_ptr<DataBatch> batch, std::size_t partition_idx)
{
  if (!batch) { throw std::invalid_argument("cannot push a null data batch handle"); }
  impl_->repository->add_data_batch(std::move(batch->impl_->batch), partition_idx);
}

std::unique_ptr<DataBatch> DataRepository::pop_next(std::size_t partition_idx)
{
  auto batch = impl_->repository->pop_next_data_batch(partition_idx);
  if (!batch) { return nullptr; }
  return detail::CuCascadeAdapter::wrap_data_batch(std::move(batch));
}

std::size_t DataRepository::size(std::size_t partition_idx) const
{
  return impl_->repository->size(partition_idx);
}

std::size_t DataRepository::total_size() const { return impl_->repository->total_size(); }

std::size_t DataRepository::num_partitions() const
{
  return impl_->repository->num_partitions();
}

void DataRepository::set_num_partitions(std::size_t new_num_partitions)
{
  impl_->repository->set_num_partitions(new_num_partitions);
}

std::unique_ptr<DataRepository> make_data_repository()
{
  return detail::CuCascadeAdapter::wrap_data_repository(
    std::make_shared<cucascade::shared_data_repository>());
}

namespace detail {

std::unique_ptr<DataBatch> CuCascadeAdapter::wrap_data_batch(
  std::shared_ptr<cucascade::data_batch> batch)
{
  return std::unique_ptr<DataBatch>(
    new DataBatch(std::make_unique<DataBatch::Impl>(std::move(batch))));
}

std::shared_ptr<cucascade::data_batch> CuCascadeAdapter::native_data_batch(const DataBatch& batch)
{
  return batch.impl_->batch;
}

std::unique_ptr<DataRepository> CuCascadeAdapter::wrap_data_repository(
  std::shared_ptr<cucascade::shared_data_repository> repository)
{
  return std::unique_ptr<DataRepository>(
    new DataRepository(std::make_unique<DataRepository::Impl>(std::move(repository))));
}

std::shared_ptr<cucascade::shared_data_repository> CuCascadeAdapter::native_data_repository(
  const DataRepository& repository)
{
  return repository.impl_->repository;
}

}  // namespace detail
}  // namespace sirius::ffi
