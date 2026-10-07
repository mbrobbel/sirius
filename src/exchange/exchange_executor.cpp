/*
 * Copyright 2026, Sirius Contributors.
 * SPDX-License-Identifier: Apache-2.0
 */

#include "exchange/exchange_executor.hpp"

#include "cudf/cudf_utils.hpp"
#include "data/data_batch_utils.hpp"
#include "data/sirius_converter_registry.hpp"
#include "exec/stream_session.hpp"
#include "helper/numeric_narrowing.hpp"
#include "sirius/exception.hpp"
#include "sirius_context.hpp"

#include <cudf/contiguous_split.hpp>
#include <cudf/null_mask.hpp>
#include <cudf/table/table_view.hpp>
#include <cudf/utilities/type_dispatcher.hpp>

#include <rmm/cuda_device.hpp>
#include <rmm/cuda_stream.hpp>
#include <rmm/device_buffer.hpp>

#include <cucascade/cuda/event.hpp>
#include <cucascade/memory/reservation_aware_resource_adaptor.hpp>
#include <nixl.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <condition_variable>
#include <deque>
#include <limits>
#include <map>
#include <mutex>
#include <optional>
#include <set>
#include <span>
#include <string_view>
#include <thread>
#include <type_traits>
#include <utility>

namespace sirius::exchange {
namespace {

constexpr std::size_t min_staging_bytes    = 1U << 20;
constexpr std::size_t max_metadata_bytes   = 1U << 20;
constexpr std::size_t max_control_bytes    = 2U << 20;
constexpr std::size_t max_columns          = 4096;
constexpr std::size_t control_buffer_bytes = 512;
constexpr std::string_view protocol_magic  = "SIRIUS-X1";
using clock_type                           = std::chrono::steady_clock;
using gpu_space                            = cucascade::memory::memory_space;
using gpu_allocator                        = cucascade::memory::reservation_aware_resource_adaptor;

[[noreturn]] void fail(const std::string& message)
{
  throw sirius::invalid_input_exception("NIXL exchange: " + message);
}

void check(nixl_status_t status, std::string_view operation)
{
  if (status != NIXL_SUCCESS) {
    fail(std::string(operation) + ": " + nixlEnumStrings::statusStr(status));
  }
}

void check_cuda(cudaError_t status, std::string_view operation)
{
  if (status != cudaSuccess) { fail(std::string(operation) + ": " + cudaGetErrorString(status)); }
}

std::string error_message(std::exception_ptr error)
{
  try {
    std::rethrow_exception(error);
  } catch (const std::exception& e) {
    return std::string(e.what()).substr(0, 4096);
  } catch (...) {
    return "unknown exchange failure";
  }
}

enum class message_kind : std::uint8_t { offer = 1, ready, chunk, ack, eos, eos_ack, error };

/// All integers have a fixed width and little-endian encoding; strings may contain NUL bytes.
class writer {
 public:
  writer() : bytes(protocol_magic) {}
  template <typename T>
  void integer(T value)
  {
    static_assert(std::is_unsigned_v<T>);
    for (std::size_t i = 0; i < sizeof(T); ++i) {
      bytes.push_back(static_cast<char>(value & 255));
      value >>= 8;
    }
  }
  void blob(std::string_view value)
  {
    if (value.size() > max_control_bytes) { fail("control field exceeds size limit"); }
    integer<std::uint32_t>(value.size());
    bytes.append(value);
  }
  std::string bytes;
};

class reader {
 public:
  explicit reader(std::string_view bytes) : _bytes(bytes)
  {
    if (bytes.size() > max_control_bytes || !bytes.starts_with(protocol_magic)) {
      fail("invalid control frame version or size");
    }
    _bytes.remove_prefix(protocol_magic.size());
  }
  template <typename T>
  T integer()
  {
    static_assert(std::is_unsigned_v<T>);
    if (_bytes.size() < sizeof(T)) { fail("truncated control frame"); }
    T value{};
    for (std::size_t i = 0; i < sizeof(T); ++i) {
      value |= static_cast<T>(static_cast<unsigned char>(_bytes[i])) << (8 * i);
    }
    _bytes.remove_prefix(sizeof(T));
    return value;
  }
  std::string blob(std::size_t limit)
  {
    const auto size = integer<std::uint32_t>();
    if (size > limit || size > _bytes.size()) { fail("invalid control field length"); }
    std::string value(_bytes.substr(0, size));
    _bytes.remove_prefix(size);
    return value;
  }
  void end() const
  {
    if (!_bytes.empty()) { fail("unexpected trailing control data"); }
  }

 private:
  std::string_view _bytes;
};

struct identity {
  route address;
  std::uint32_t sender{};
  std::uint64_t sequence{};
  auto operator<=>(const identity&) const = default;
};

writer frame(message_kind kind, const identity& id)
{
  writer out;
  out.integer<std::uint8_t>(static_cast<std::uint8_t>(kind));
  out.integer(id.address.query_id.high);
  out.integer(id.address.query_id.low);
  out.integer(id.address.fragment_id.high);
  out.integer(id.address.fragment_id.low);
  out.integer(id.address.exchange_id);
  out.integer(id.sender);
  out.integer(id.sequence);
  return out;
}

identity read_identity(reader& in)
{
  identity id;
  id.address.query_id.high    = in.integer<std::uint64_t>();
  id.address.query_id.low     = in.integer<std::uint64_t>();
  id.address.fragment_id.high = in.integer<std::uint64_t>();
  id.address.fragment_id.low  = in.integer<std::uint64_t>();
  id.address.exchange_id      = in.integer<std::uint32_t>();
  id.sender                   = in.integer<std::uint32_t>();
  id.sequence                 = in.integer<std::uint64_t>();
  return id;
}

std::vector<std::string> type_names(const std::vector<logical_type>& types)
{
  std::vector<std::string> names;
  names.reserve(types.size());
  for (const auto& type : types) {
    names.push_back(type.to_string());
  }
  return names;
}

/// A short-lived allocation scope; live buffers remain charged after the reservation is reset.
class allocation_scope {
 public:
  allocation_scope(gpu_space& space, rmm::cuda_stream_view stream, std::size_t bytes)
    : _stream(stream)
  {
    auto reservation = space.make_reservation_or_null(std::max(bytes, std::size_t{256}));
    if (!reservation) { return; }
    auto* allocator = reservation->get_memory_resource_of<cucascade::memory::Tier::GPU>();
    if (allocator->attach_reservation_to_tracker(
          stream,
          std::move(reservation),
          std::make_unique<cucascade::memory::increase_reservation_limit_policy>(1.0, false))) {
      _allocator = allocator;
    }
  }
  ~allocation_scope()
  {
    if (_allocator) { _allocator->reset_stream_reservation(_stream); }
  }
  explicit operator bool() const { return _allocator != nullptr; }

 private:
  gpu_allocator* _allocator{};
  rmm::cuda_stream_view _stream;
};

struct peer {
  std::uint64_t send_address{};
  std::uint64_t control_address{};
  std::size_t staging_size{};
  std::uint32_t device{};
  std::string metadata;
};

struct sender_state {
  std::string peer;
  std::uint64_t next_sequence{};
  bool busy{};
  bool closed{};
};

struct input_state {
  input spec;
  std::vector<std::string> schema;
  std::map<std::uint32_t, sender_state> senders;
  bool registered{};
};

struct output_state {
  destination target;
  std::size_t stream_id{};
  std::uint64_t next_sequence{};
  bool eos_sent{};
  bool closed{};
};

struct send_batch {
  enum class phase { prepare, waiting_ready, ready_chunk, waiting_ack };
  std::size_t output_index{};
  identity id;
  std::string peer_name;
  std::shared_ptr<cucascade::data_batch> batch;
  std::optional<cucascade::read_only_data_batch> locked;
  std::vector<std::unique_ptr<cudf::column>> restored_columns;
  std::unique_ptr<cudf::chunked_pack> packer;
  std::size_t total{};
  std::size_t offset{};
  std::size_t chunk_size{};
  std::size_t pending_bytes{};
  phase state{phase::prepare};
};

struct receive_batch {
  identity id;
  std::string peer_name;
  std::vector<std::uint8_t> metadata;
  std::size_t total{};
  std::size_t received{};
  std::size_t chunk_size{};
  std::size_t pending_bytes{};
  std::shared_ptr<rmm::device_buffer> buffer;
  nixlXferReqH* transfer{};
};

}  // namespace

struct exchange_executor::impl {
  duckdb::SiriusContext& context;
  std::string name;
  std::size_t staging_bytes;
  std::chrono::milliseconds timeout;
  gpu_space* space{};
  std::unique_ptr<rmm::cuda_stream> stream;
  std::unique_ptr<rmm::device_buffer> send_staging;
  std::unique_ptr<rmm::device_buffer> receive_staging;
  std::unique_ptr<rmm::device_buffer> control_buffer;
  std::unique_ptr<nixlAgent> agent;
  nixl_reg_dlist_t registered{VRAM_SEG};
  bool memory_registered{};
  std::map<std::string, peer> peers;
  std::string local_metadata;
  bool failed{};
  std::array<nixlXferReqH*, 2> retained_requests{};

  std::mutex mutex;
  std::condition_variable wake;
  std::thread worker;
  std::atomic<bool> stop_requested{false};
  std::atomic<bool> discard_inputs{false};
  std::atomic<bool> run_started{false};
  std::exception_ptr requested_error;
  std::exception_ptr result_error;
  exec::stream_session* session{};
  plan active_plan;
  std::vector<logical_type> output_types;
  std::vector<std::string> output_schema;
  std::map<route, input_state> inputs;
  std::vector<output_state> outputs;
  std::unique_ptr<send_batch> sending;
  std::deque<receive_batch> offers;
  std::optional<receive_batch> receiving;
  bool receive_reservation_blocked{};
  std::deque<std::pair<std::string, writer>> terminal_messages;
  nixlXferReqH* terminal_transfer{};
  std::string error_peer;
  std::size_t next_output{};
  clock_type::time_point last_progress;

  impl(duckdb::SiriusContext& ctx,
       std::string agent_name,
       std::size_t bytes,
       std::chrono::milliseconds deadline)
    : context(ctx), name(std::move(agent_name)), staging_bytes(bytes), timeout(deadline)
  {
    if (name.empty() || name.size() > 256) { fail("agent name must contain 1 to 256 bytes"); }
    if (bytes < min_staging_bytes ||
        bytes > (std::numeric_limits<std::int64_t>::max() - control_buffer_bytes) / 2) {
      fail("staging buffer must be at least 1 MiB and fit the GPU memory budget");
    }
    if (timeout.count() <= 0) { fail("timeout must be positive"); }
    auto spaces =
      context.get_memory_manager().get_memory_spaces_for_tier(cucascade::memory::Tier::GPU);
    if (spaces.empty()) { fail("no GPU memory space is configured"); }
    space = context.get_memory_manager().get_memory_space(cucascade::memory::Tier::GPU,
                                                          spaces.front()->get_device_id());
    rmm::cuda_set_device_raii device{rmm::cuda_device_id{space->get_device_id()}};
    stream = std::make_unique<rmm::cuda_stream>();
    {
      allocation_scope reservation(*space, stream->view(), bytes * 2 + control_buffer_bytes);
      if (!reservation) { fail("cannot reserve the GPU staging buffers"); }
      send_staging =
        std::make_unique<rmm::device_buffer>(bytes, stream->view(), space->get_default_allocator());
      receive_staging =
        std::make_unique<rmm::device_buffer>(bytes, stream->view(), space->get_default_allocator());
      control_buffer = std::make_unique<rmm::device_buffer>(
        control_buffer_bytes, stream->view(), space->get_default_allocator());
      check_cuda(cudaMemsetAsync(control_buffer->data(), 0, control_buffer_bytes, stream->value()),
                 "initialize control buffer");
    }
    stream->synchronize();
    nixlAgentConfig config;
    config.useProgThread = true;
    config.syncMode      = nixl_thread_sync_t::NIXL_THREAD_SYNC_STRICT;
    agent                = std::make_unique<nixlAgent>(name, config);
    nixlBackendH* backend{};
    check(agent->createBackend("UCX", {}, backend), "create UCX backend");
    registered.addDesc(nixlBlobDesc(
      reinterpret_cast<std::uintptr_t>(send_staging->data()), bytes, space->get_device_id(), ""));
    registered.addDesc(nixlBlobDesc(reinterpret_cast<std::uintptr_t>(receive_staging->data()),
                                    bytes,
                                    space->get_device_id(),
                                    ""));
    registered.addDesc(nixlBlobDesc(reinterpret_cast<std::uintptr_t>(control_buffer->data()),
                                    control_buffer_bytes,
                                    space->get_device_id(),
                                    ""));
    check(agent->registerMem(registered), "register GPU staging buffers");
    memory_registered = true;
    try {
      std::string opaque;
      check(agent->getLocalMD(opaque), "export agent metadata");
      writer out;
      out.blob(name);
      out.integer<std::uint64_t>(reinterpret_cast<std::uintptr_t>(send_staging->data()));
      out.integer<std::uint64_t>(reinterpret_cast<std::uintptr_t>(control_buffer->data()));
      out.integer<std::uint64_t>(bytes);
      out.integer<std::uint32_t>(space->get_device_id());
      out.blob(opaque);
      local_metadata = std::move(out.bytes);
    } catch (...) {
      agent->deregisterMem(registered);
      memory_registered = false;
      throw;
    }
  }

  ~impl()
  {
    cancel(std::make_exception_ptr(sirius::invalid_input_exception("exchange context closed")));
    rmm::cuda_set_device_raii device{rmm::cuda_device_id{space->get_device_id()}};
    stream->synchronize_no_throw();
    // A failed transport retains staging until its UCX workers have been destroyed.
    for (auto* request : retained_requests) {
      if (request) { agent->releaseXferReq(request); }
    }
    if (memory_registered && !failed) { agent->deregisterMem(registered); }
    agent.reset();
    control_buffer.reset();
    receive_staging.reset();
    send_staging.reset();
    stream.reset();
  }

  std::string add_peer(const std::string& serialized)
  {
    std::lock_guard guard(mutex);
    if (worker.joinable()) { fail("peers must be connected before attaching a fragment"); }
    reader in(serialized);
    auto peer_name = in.blob(256);
    peer remote;
    remote.send_address    = in.integer<std::uint64_t>();
    remote.control_address = in.integer<std::uint64_t>();
    remote.staging_size    = in.integer<std::uint64_t>();
    remote.device          = in.integer<std::uint32_t>();
    auto opaque            = in.blob(max_metadata_bytes);
    in.end();
    if (peer_name.empty() || peer_name == name || remote.send_address == 0 ||
        remote.control_address == 0 ||
        remote.control_address > std::numeric_limits<std::uint64_t>::max() - control_buffer_bytes ||
        remote.staging_size < min_staging_bytes ||
        remote.send_address > std::numeric_limits<std::uint64_t>::max() - remote.staging_size) {
      fail("invalid peer metadata");
    }
    if (auto found = peers.find(peer_name); found != peers.end()) {
      if (found->second.metadata != serialized) {
        fail("peer name already has different metadata");
      }
      return peer_name;
    }
    std::string loaded_name;
    check(agent->loadRemoteMD(opaque, loaded_name), "import peer metadata");
    if (loaded_name != peer_name) {
      agent->invalidateRemoteMD(loaded_name);
      fail("peer name disagrees with NIXL metadata");
    }
    check(agent->makeConnection(peer_name), "connect peer");
    remote.metadata = serialized;
    peers.emplace(peer_name, std::move(remote));
    return peer_name;
  }

  void attach(const plan& spec,
              exec::stream_session& active_session,
              const std::vector<logical_type>& schema)
  {
    std::lock_guard guard(mutex);
    if (worker.joinable()) { fail("a fragment is already attached"); }
    if (failed) {
      fail("exchange context is unusable after a transport failure; create a new context");
    }
    const auto registered_inputs = active_session.input_streams();
    std::map<route, input_state> new_inputs;
    for (const auto& source : spec.inputs) {
      input_state state{source, type_names(source.types), {}};
      state.registered =
        std::find(registered_inputs.begin(), registered_inputs.end(), source.stream_id()) !=
        registered_inputs.end();
      if (source.expected_senders.empty()) { fail("input has no declared senders"); }
      for (auto sender : source.expected_senders) {
        state.senders.emplace(sender, sender_state{});
      }
      if (!new_inputs.emplace(source.address, std::move(state)).second) {
        fail("duplicate input route");
      }
    }
    std::vector<output_state> new_outputs;
    if (spec.sink) {
      if (schema.empty() || schema.size() > max_columns) { fail("invalid output column count"); }
      std::set<std::pair<std::string, route>> destinations;
      for (const auto& target : spec.sink->targets) {
        if (!peers.contains(target.peer)) { fail("unknown output peer '" + target.peer + "'"); }
        if (!destinations.emplace(target.peer, target.address).second) {
          fail("duplicate output destination");
        }
        new_outputs.push_back(output_state{target, new_outputs.size()});
      }
    }
    active_plan   = spec;
    output_types  = schema;
    output_schema = type_names(schema);
    inputs        = std::move(new_inputs);
    outputs       = std::move(new_outputs);
    next_output   = 0;
    error_peer.clear();
    receive_reservation_blocked = false;
    requested_error             = nullptr;
    result_error                = nullptr;
    stop_requested.store(false);
    discard_inputs.store(false);
    run_started.store(false);
    session       = &active_session;
    last_progress = clock_type::now();
    try {
      worker = std::thread([this] { run(); });
    } catch (...) {
      session = nullptr;
      throw;
    }
  }

  void finish()
  {
    if (!worker.joinable()) { fail("no fragment is attached"); }
    discard_inputs.store(true);
    wake.notify_all();
    worker.join();
    if (result_error) { std::rethrow_exception(result_error); }
  }

  void start()
  {
    if (!worker.joinable()) { fail("no fragment is attached"); }
    if (run_started.exchange(true)) { fail("fragment execution already started"); }
    wake.notify_all();
  }

  void cancel(std::exception_ptr error) noexcept
  {
    try {
      {
        std::lock_guard guard(mutex);
        if (!worker.joinable()) { return; }
        requested_error = error;
        stop_requested.store(true);
      }
      wake.notify_all();
      worker.join();
    } catch (...) {
      // Destructors must not replace the original query failure.
    }
  }

  void notify(const std::string& remote, const writer& message)
  {
    if (message.bytes.size() > max_control_bytes) { fail("control frame exceeds size limit"); }
    check(agent->genNotif(remote, message.bytes), "send control message");
  }

  bool progress_terminal()
  {
    if (terminal_transfer) {
      const auto status = agent->getXferStatus(terminal_transfer);
      if (status == NIXL_IN_PROG) { return false; }
      check(status, "complete terminal notification");
      check(agent->releaseXferReq(terminal_transfer), "release terminal notification");
      terminal_transfer = nullptr;
      return true;
    }
    if (terminal_messages.empty()) { return false; }
    const auto& [remote, message] = terminal_messages.front();
    const auto& remote_peer       = peers.at(remote);
    nixl_xfer_dlist_t local(VRAM_SEG), remote_desc(VRAM_SEG);
    local.addDesc(nixlBasicDesc(
      reinterpret_cast<std::uintptr_t>(control_buffer->data()) + 256, 1, space->get_device_id()));
    remote_desc.addDesc(nixlBasicDesc(remote_peer.control_address, 1, remote_peer.device));
    nixl_opt_args_t options;
    options.notif = message.bytes;
    // A transfer-bound notification stays tracked until the UCX send completes.
    check(agent->createXferReq(NIXL_READ, local, remote_desc, remote, terminal_transfer, &options),
          "prepare terminal notification");
    const auto status = agent->postXferReq(terminal_transfer);
    if (status != NIXL_SUCCESS && status != NIXL_IN_PROG) {
      check(status, "send terminal notification");
    }
    terminal_messages.pop_front();
    return true;
  }

  void drain_failed_transfers() noexcept
  {
    if (!failed) { return; }
    const auto start     = clock_type::now();
    const auto limit     = std::min(timeout, std::chrono::milliseconds(1000));
    bool terminal_failed = false;
    while (std::chrono::duration_cast<std::chrono::milliseconds>(clock_type::now() - start) <
           limit) {
      if (receiving && receiving->transfer) {
        const auto status = agent->getXferStatus(receiving->transfer);
        if (status == NIXL_SUCCESS && agent->releaseXferReq(receiving->transfer) == NIXL_SUCCESS) {
          receiving->transfer = nullptr;
        }
      }
      if (!terminal_failed) {
        try {
          progress_terminal();
        } catch (...) {
          terminal_failed = true;
        }
      }
      if ((!receiving || !receiving->transfer) &&
          (terminal_failed || (!terminal_transfer && terminal_messages.empty()))) {
        break;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    // NIXL's UCX cancellation does not drain canceled requests. Keep every possibly
    // active request and its registered buffers alive until the agent is destroyed.
    if (receiving && receiving->transfer) {
      retained_requests[0] = receiving->transfer;
      receiving->transfer  = nullptr;
    }
    if (terminal_transfer) {
      retained_requests[1] = terminal_transfer;
      terminal_transfer    = nullptr;
    }
    terminal_messages.clear();
  }

  input_state& resolve_input(const identity& id, const std::string& remote)
  {
    auto found = inputs.find(id.address);
    if (found == inputs.end()) { fail("message addresses an undeclared input route"); }
    auto sender = found->second.senders.find(id.sender);
    if (sender == found->second.senders.end()) { fail("message comes from an undeclared sender"); }
    if (!sender->second.peer.empty() && sender->second.peer != remote) {
      fail("sender identity changed peer");
    }
    if (sender->second.closed || sender->second.next_sequence != id.sequence) {
      fail("unexpected sender sequence or data after EOS");
    }
    sender->second.peer = remote;
    return found->second;
  }

  void handle_offer(const std::string& remote, const identity& id, reader& in)
  {
    auto& source = resolve_input(id, remote);
    auto& sender = source.senders.at(id.sender);
    if (sender.busy) { fail("sender offered overlapping batches"); }
    receive_batch batch;
    batch.id         = id;
    batch.peer_name  = remote;
    batch.total      = in.integer<std::uint64_t>();
    batch.chunk_size = in.integer<std::uint64_t>();
    auto metadata    = in.blob(max_metadata_bytes);
    batch.metadata.assign(metadata.begin(), metadata.end());
    const auto count = in.integer<std::uint32_t>();
    if (count != source.schema.size() || count > max_columns) {
      fail("input schema column count differs");
    }
    for (const auto& expected : source.schema) {
      if (in.blob(4096) != expected) { fail("input schema does not match its declared type"); }
    }
    in.end();
    if ((!discard_inputs.load() && source.registered && batch.total > space->get_max_memory()) ||
        batch.chunk_size < min_staging_bytes ||
        batch.chunk_size > std::min(staging_bytes, peers.at(remote).staging_size)) {
      fail("invalid packed batch or chunk size");
    }
    cudf::packed_metadata_view packed(batch.metadata);
    if (packed.num_columns() != static_cast<cudf::size_type>(source.schema.size())) {
      fail("packed metadata column count differs from the schema");
    }
    for (cudf::size_type i = 0; i < packed.num_columns(); ++i) {
      if (packed.column(i).type() != sirius::get_cudf_type(source.spec.types[i])) {
        fail("packed column type differs from the declared schema");
      }
    }
    sender.busy = true;
    offers.push_back(std::move(batch));
  }

  void handle_chunk(const std::string& remote, const identity& id, reader& in)
  {
    const auto offset = in.integer<std::uint64_t>();
    const auto bytes  = in.integer<std::uint64_t>();
    in.end();
    if (!receiving || receiving->id != id || receiving->peer_name != remote ||
        receiving->transfer) {
      fail("chunk has no matching granted receive buffer");
    }
    auto& batch = *receiving;
    if (offset != batch.received || bytes > batch.chunk_size ||
        bytes > batch.total - batch.received || (bytes == 0 && batch.total != 0)) {
      fail("invalid chunk range");
    }
    batch.pending_bytes = bytes;
    if (bytes == 0) {
      complete_chunk();
      return;
    }
    nixl_xfer_dlist_t local(VRAM_SEG), remote_desc(VRAM_SEG);
    local.addDesc(nixlBasicDesc(
      reinterpret_cast<std::uintptr_t>(receive_staging->data()), bytes, space->get_device_id()));
    const auto& remote_peer = peers.at(remote);
    remote_desc.addDesc(nixlBasicDesc(remote_peer.send_address, bytes, remote_peer.device));
    check(agent->createXferReq(NIXL_READ, local, remote_desc, remote, batch.transfer),
          "prepare chunk read");
    const auto status = agent->postXferReq(batch.transfer);
    if (status != NIXL_IN_PROG && status != NIXL_SUCCESS) { check(status, "start chunk read"); }
  }

  void complete_chunk()
  {
    auto& batch        = *receiving;
    auto& source       = inputs.at(batch.id.address);
    const bool discard = discard_inputs.load() || !source.registered;
    if (batch.pending_bytes && !discard) {
      check_cuda(cudaMemcpyAsync(static_cast<std::uint8_t*>(batch.buffer->data()) + batch.received,
                                 receive_staging->data(),
                                 batch.pending_bytes,
                                 cudaMemcpyDeviceToDevice,
                                 stream->value()),
                 "copy received chunk");
      stream->synchronize();
    }
    batch.received += batch.pending_bytes;
    auto acknowledgement = frame(message_kind::ack, batch.id);
    acknowledgement.integer<std::uint64_t>(batch.received);
    if (batch.received == batch.total) {
      if (!discard) {
        auto view         = cudf::unpack(batch.metadata.data(),
                                 static_cast<const std::uint8_t*>(batch.buffer->data()));
        auto output_batch = sirius::make_data_batch_from_view(view,
                                                              batch.buffer,
                                                              batch.total,
                                                              *space,
                                                              stream->view(),
                                                              telemetry::batch_telemetry_info{});
        if (!session->push(source.spec.stream_id(), std::move(output_batch))) {
          fail("receiver rejected a batch after termination");
        }
      }
      auto& sender = source.senders.at(batch.id.sender);
      sender.busy  = false;
      ++sender.next_sequence;
      notify(batch.peer_name, acknowledgement);
      receiving.reset();
    } else {
      notify(batch.peer_name, acknowledgement);
    }
  }

  void handle(const std::string& remote, const std::string& bytes)
  {
    if (!peers.contains(remote)) { fail("message from an unconnected peer"); }
    reader in(bytes);
    const auto kind = static_cast<message_kind>(in.integer<std::uint8_t>());
    const auto id   = read_identity(in);
    const bool current_route =
      inputs.contains(id.address) ||
      std::any_of(outputs.begin(), outputs.end(), [&](const auto& output) {
        return output.target.address == id.address && output.target.peer == remote;
      });
    // Prior queries may still have control notifications queued on a persistent agent.
    if (!current_route) { return; }
    switch (kind) {
      case message_kind::offer: handle_offer(remote, id, in); break;
      case message_kind::chunk: handle_chunk(remote, id, in); break;
      case message_kind::ready:
        in.end();
        if (!sending || sending->id != id || sending->peer_name != remote ||
            sending->state != send_batch::phase::waiting_ready) {
          fail("unexpected receive grant");
        }
        sending->state = send_batch::phase::ready_chunk;
        break;
      case message_kind::ack: {
        const auto offset = in.integer<std::uint64_t>();
        in.end();
        if (!sending || sending->id != id || sending->peer_name != remote ||
            sending->state != send_batch::phase::waiting_ack ||
            offset != sending->offset + sending->pending_bytes) {
          fail("unexpected chunk acknowledgement");
        }
        sending->offset = offset;
        if (offset == sending->total) {
          ++outputs[sending->output_index].next_sequence;
          sending.reset();
        } else {
          sending->state = send_batch::phase::ready_chunk;
        }
        break;
      }
      case message_kind::eos: {
        const auto count = in.integer<std::uint32_t>();
        auto& source     = resolve_input(id, remote);
        if (count != source.schema.size()) { fail("EOS schema column count differs"); }
        for (const auto& expected : source.schema) {
          if (in.blob(4096) != expected) { fail("EOS schema does not match the input"); }
        }
        in.end();
        auto& sender = source.senders.at(id.sender);
        if (sender.busy) { fail("EOS arrived before its final batch completed"); }
        if (source.registered) { session->close_input(source.spec.stream_id(), id.sender); }
        sender.closed = true;
        terminal_messages.emplace_back(remote, frame(message_kind::eos_ack, id));
        break;
      }
      case message_kind::eos_ack: {
        in.end();
        auto found = std::find_if(outputs.begin(), outputs.end(), [&](const auto& output) {
          return output.target.peer == remote && output.target.address == id.address;
        });
        if (found == outputs.end() || !found->eos_sent || found->closed ||
            id.sequence != found->next_sequence || id.sender != active_plan.sink->sender_id) {
          fail("unexpected EOS acknowledgement");
        }
        found->closed = true;
        break;
      }
      case message_kind::error: {
        auto error = in.blob(4096);
        in.end();
        const auto source          = inputs.find(id.address);
        const bool addresses_input = source != inputs.end() &&
                                     source->second.senders.contains(id.sender) &&
                                     (source->second.senders.at(id.sender).peer.empty() ||
                                      source->second.senders.at(id.sender).peer == remote);
        const bool addresses_output =
          active_plan.sink && id.sender == active_plan.sink->sender_id &&
          std::any_of(outputs.begin(), outputs.end(), [&](const auto& output) {
            return output.target.address == id.address && output.target.peer == remote;
          });
        // Late notifications from a prior query can outlive its fragment.
        if (!addresses_input && !addresses_output) { return; }
        error_peer = remote;
        fail("peer '" + remote + "' failed: " + error);
      }
      default: fail("unknown control message kind");
    }
    last_progress = clock_type::now();
  }

  bool start_receive()
  {
    receive_reservation_blocked = false;
    if (receiving || offers.empty()) { return false; }
    for (auto it = offers.begin(); it != offers.end(); ++it) {
      if (!discard_inputs.load() && inputs.at(it->id.address).registered) {
        allocation_scope reservation(*space, stream->view(), it->total);
        if (!reservation) { continue; }
        it->buffer = std::make_shared<rmm::device_buffer>(
          it->total, stream->view(), space->get_default_allocator());
      }
      receiving.emplace(std::move(*it));
      offers.erase(it);
      notify(receiving->peer_name, frame(message_kind::ready, receiving->id));
      return true;
    }
    receive_reservation_blocked = true;
    return false;
  }

  bool progress_receive()
  {
    if (!receiving || !receiving->transfer) { return false; }
    const auto status = agent->getXferStatus(receiving->transfer);
    if (status == NIXL_IN_PROG) { return false; }
    check(status, "complete chunk read");
    check(agent->releaseXferReq(receiving->transfer), "release chunk read");
    receiving->transfer = nullptr;
    complete_chunk();
    return true;
  }

  bool select_output()
  {
    if (sending) { return false; }
    for (std::size_t n = 0; n < outputs.size(); ++n) {
      const auto index = next_output++ % outputs.size();
      auto& output     = outputs[index];
      if (output.eos_sent) { continue; }
      auto batch = session->pull(output.stream_id);
      const identity id{output.target.address, active_plan.sink->sender_id, output.next_sequence};
      if (batch) {
        sending               = std::make_unique<send_batch>();
        sending->output_index = index;
        sending->id           = id;
        sending->peer_name    = output.target.peer;
        sending->batch        = std::move(*batch);
        sending->chunk_size   = std::min(staging_bytes, peers.at(output.target.peer).staging_size);
        return true;
      }
      if (session->drained(output.stream_id)) {
        auto end = frame(message_kind::eos, id);
        end.integer<std::uint32_t>(output_schema.size());
        for (const auto& type : output_schema) {
          end.blob(type);
        }
        notify(output.target.peer, end);
        output.eos_sent = true;
        return true;
      }
    }
    return false;
  }

  bool prepare_send()
  {
    auto& batch = *sending;
    auto locked = batch.batch->try_to_read_only();
    if (!locked) { return false; }
    if (locked->get_memory_space() != space) {
      const auto bytes = sirius::peak_materialization_bytes(locked->get_data());
      locked.reset();
      auto mutable_batch = batch.batch->try_to_mutable();
      if (!mutable_batch) { return false; }
      allocation_scope reservation(*space, stream->view(), bytes);
      if (!reservation) { return false; }
      if (auto event = mutable_batch->get_data()->get_writer_event(); event != nullptr) {
        cucascade::cuda::cuda_event_view{event}.wait(stream->view());
      }
      mutable_batch->convert_to<cucascade::gpu_table_representation>(
        sirius::converter_registry::get(), space, stream->view());
      mutable_batch.reset();
      locked = batch.batch->try_to_read_only();
      if (!locked) { return false; }
    }
    auto event = locked->get_data()->get_writer_event();
    if (!event) { fail("GPU output batch lacks a writer event"); }
    cucascade::cuda::cuda_event_view{event}.wait(stream->view());
    auto view = sirius::get_cudf_table_view(*locked);
    if (view.num_columns() != static_cast<cudf::size_type>(output_types.size())) {
      fail("output column count differs from the plan");
    }
    std::size_t reservation_bytes = min_staging_bytes;
    for (cudf::size_type i = 0; i < view.num_columns(); ++i) {
      const auto column   = view.column(i);
      const auto declared = sirius::get_cudf_type(output_types[i]);
      if (column.type() == declared) { continue; }
      if (!sirius::can_restore_to(column.type(), declared)) {
        fail("output physical type contradicts its schema");
      }
      const auto data_bytes = static_cast<std::size_t>(column.size()) * cudf::size_of(declared);
      reservation_bytes =
        sirius::memory::saturating_add(reservation_bytes, (data_bytes + 255) / 256 * 256);
      if (column.nullable()) {
        reservation_bytes = sirius::memory::saturating_add(
          reservation_bytes,
          (cudf::bitmask_allocation_size_bytes(column.size()) + 255) / 256 * 256);
      }
    }
    // Restored columns coexist with the input. cuDF scratch can grow this reservation;
    // the resource checks every growth against the configured GPU memory limit.
    allocation_scope reservation(*space, stream->view(), reservation_bytes);
    if (!reservation) { return false; }
    std::vector<cudf::column_view> columns;
    for (cudf::size_type i = 0; i < view.num_columns(); ++i) {
      auto column         = view.column(i);
      const auto declared = sirius::get_cudf_type(output_types[i]);
      if (column.type() != declared) {
        batch.restored_columns.push_back(sirius::cast_through_rep(
          column, declared, stream->view(), space->get_default_allocator()));
        column = batch.restored_columns.back()->view();
      }
      columns.push_back(column);
    }
    batch.locked = std::move(locked);
    batch.packer = cudf::chunked_pack::create(
      cudf::table_view(columns), batch.chunk_size, stream->view(), space->get_default_allocator());
    batch.total   = batch.packer->get_total_contiguous_size();
    auto metadata = batch.packer->build_metadata();
    if (metadata->size() > max_metadata_bytes) { fail("packed table metadata exceeds limit"); }
    auto offer = frame(message_kind::offer, batch.id);
    offer.integer<std::uint64_t>(batch.total);
    offer.integer<std::uint64_t>(batch.chunk_size);
    offer.blob(std::string_view(reinterpret_cast<const char*>(metadata->data()), metadata->size()));
    offer.integer<std::uint32_t>(output_schema.size());
    for (const auto& type : output_schema) {
      offer.blob(type);
    }
    notify(batch.peer_name, offer);
    batch.state = send_batch::phase::waiting_ready;
    return true;
  }

  bool progress_send()
  {
    if (!sending) { return false; }
    auto& batch = *sending;
    if (batch.state == send_batch::phase::prepare) { return prepare_send(); }
    if (batch.state != send_batch::phase::ready_chunk) { return false; }
    batch.pending_bytes = batch.packer->has_next()
                            ? batch.packer->next(cudf::device_span<std::uint8_t>(
                                static_cast<std::uint8_t*>(send_staging->data()), batch.chunk_size))
                            : 0;
    if ((batch.pending_bytes == 0 && batch.total != 0) ||
        batch.pending_bytes > batch.total - batch.offset) {
      fail("cuDF packer produced an invalid chunk size");
    }
    stream->synchronize();
    auto chunk = frame(message_kind::chunk, batch.id);
    chunk.integer<std::uint64_t>(batch.offset);
    chunk.integer<std::uint64_t>(batch.pending_bytes);
    notify(batch.peer_name, chunk);
    batch.state = send_batch::phase::waiting_ack;
    return true;
  }

  bool complete() const
  {
    if (sending || receiving || !offers.empty() || terminal_transfer ||
        !terminal_messages.empty()) {
      return false;
    }
    for (const auto& output : outputs) {
      if (!output.closed) { return false; }
    }
    for (const auto& [address, source] : inputs) {
      for (const auto& [id, sender] : source.senders) {
        if (!sender.closed) { return false; }
      }
    }
    return true;
  }

  bool waiting_for_peer() const
  {
    if (discard_inputs.load()) { return !complete(); }
    if (sending && (sending->state == send_batch::phase::waiting_ready ||
                    sending->state == send_batch::phase::waiting_ack)) {
      return true;
    }
    if (terminal_transfer || !terminal_messages.empty()) { return true; }
    for (const auto& output : outputs) {
      if (output.eos_sent && !output.closed) { return true; }
    }
    for (const auto& [address, source] : inputs) {
      for (const auto& [id, sender] : source.senders) {
        if (!sender.closed) { return true; }
      }
    }
    return false;
  }

  void poison(std::exception_ptr error) noexcept
  {
    for (const auto& [address, source] : inputs) {
      try {
        if (source.registered) { session->fail_input(source.spec.stream_id(), error); }
      } catch (...) {
      }
      for (const auto& [sender_id, sender] : source.senders) {
        if (sender.peer.empty() || sender.peer == error_peer) { continue; }
        try {
          auto message =
            frame(message_kind::error, identity{address, sender_id, sender.next_sequence});
          message.blob(error_message(error));
          terminal_messages.emplace_back(sender.peer, std::move(message));
        } catch (...) {
        }
      }
    }
    for (const auto& output : outputs) {
      try {
        session->fail_output(output.stream_id, error);
      } catch (...) {
      }
      if (output.target.peer == error_peer) { continue; }
      try {
        auto message =
          frame(message_kind::error,
                identity{output.target.address, active_plan.sink->sender_id, output.next_sequence});
        message.blob(error_message(error));
        terminal_messages.emplace_back(output.target.peer, std::move(message));
      } catch (...) {
      }
    }
  }

  void run() noexcept
  {
    rmm::cuda_set_device_raii device{rmm::cuda_device_id{space->get_device_id()}};
    try {
      bool discarded_queued_inputs = false;
      bool deadline_armed          = false;
      while (true) {
        if (stop_requested.load()) {
          std::lock_guard guard(mutex);
          if (requested_error) { std::rethrow_exception(requested_error); }
          fail("fragment canceled");
        }
        if (!deadline_armed && run_started.load()) {
          last_progress  = clock_type::now();
          deadline_armed = true;
        }
        if (discard_inputs.load() && !discarded_queued_inputs) {
          for (const auto& [address, source] : inputs) {
            if (source.registered) { session->discard_input(source.spec.stream_id()); }
          }
          if (receiving) { receiving->buffer.reset(); }
          discarded_queued_inputs = true;
        }
        if (complete() && discarded_queued_inputs) { break; }
        nixl_notifs_t notifications;
        check(agent->getNotifs(notifications), "poll control messages");
        for (const auto& [remote, messages] : notifications) {
          for (const auto& message : messages) {
            handle(remote, message);
          }
        }
        bool progressed = progress_terminal();
        progressed      = progress_receive() || progressed;
        progressed      = start_receive() || progressed;
        progressed      = select_output() || progressed;
        progressed      = progress_send() || progressed;
        if (progressed) { last_progress = clock_type::now(); }
        if (deadline_armed && waiting_for_peer() &&
            std::chrono::duration_cast<std::chrono::milliseconds>(clock_type::now() -
                                                                  last_progress) > timeout) {
          if (receive_reservation_blocked) {
            fail("timed out waiting for GPU memory to receive an exchange batch");
          }
          fail("timed out waiting for exchange progress");
        }
        if (!progressed && notifications.empty()) {
          std::unique_lock guard(mutex);
          wake.wait_for(
            guard, std::chrono::milliseconds(1), [this] { return stop_requested.load(); });
        }
      }
    } catch (...) {
      result_error = std::current_exception();
      failed       = true;
      poison(result_error);
    }
    drain_failed_transfers();
    stream->synchronize_no_throw();
    receiving.reset();
    offers.clear();
    sending.reset();
    session = nullptr;
  }
};

exchange_executor::exchange_executor(duckdb::SiriusContext& context,
                                     std::string name,
                                     std::size_t staging_bytes,
                                     std::chrono::milliseconds timeout)
  : _impl(std::make_unique<impl>(context, std::move(name), staging_bytes, timeout))
{
}
exchange_executor::~exchange_executor() = default;
std::string exchange_executor::metadata() const { return _impl->local_metadata; }
std::string exchange_executor::add_peer(const std::string& metadata)
{
  return _impl->add_peer(metadata);
}
void exchange_executor::attach(const plan& spec,
                               exec::stream_session& session,
                               const std::vector<logical_type>& output_types)
{
  _impl->attach(spec, session, output_types);
}
void exchange_executor::finish() { _impl->finish(); }
void exchange_executor::start() { _impl->start(); }
void exchange_executor::cancel(std::exception_ptr error) noexcept { _impl->cancel(error); }

}  // namespace sirius::exchange
