# Public query execution API proposal

**Status:** draft for discussion. Types and examples below describe a proposed API,
not implemented contracts or finalized signatures.

## Scope

Introduce an API for externally supplied plans, initially Substrait, through the C ABI
with C++ and Rust wrappers. Prefer immutable descriptions and distinct types for
preparation, execution, and streaming capabilities.

Start with materialized execution, then add live streaming, backpressure, and
cancellation. Keep existing integrations working during migration. SQL preparation,
reusable prepared templates, and multiple independent contexts are follow-ups. The
initial implementation retains the one-active-context-per-process restriction.

## Why C underneath C++ and Rust?

The C ABI provides one implementation boundary for all consumers:

- Keep C++ standard-library types, exceptions, and compiler-specific object layouts
  out of the binary interface.
- Let C++ provide RAII and `std::expected`, and Rust provide ownership, `Result`, and
  audited `Send`/`Sync` contracts.
- Keep execution semantics and error handling consistent across languages.
- Allow implementation components to move between C++ and Rust without changing
  the consumer-facing boundary.

Binary packaging and runtime dependency compatibility remain separate concerns.

## Public model

| Type | Responsibility |
|---|---|
| `Context` | Engine resources and scheduling services. |
| `Session` | Planning environment, including catalog and settings, when needed. |
| `Plan` | Immutable, parsed plan description. |
| `PreparedPlan` | Plan bound to inputs, schemas, resources, and output routing. |
| `Execution` | Control and observe one submitted execution. |
| `InputSource` | An owning consumer endpoint, bound into a prepared plan. |
| `InputWriter` | Feed batches through one producer capability. |
| `BatchReader` | Consume one output stream. |
| `Schema` / `RecordBatch` | Owned typed data with Arrow interoperability. |
| `CancellationHandle` | Request cancellation independently of execution ownership. |
| `ExecutionReport` | Immutable terminal execution information and statistics. |

```text
Serialized plan → Plan → PreparedPlan → Execution → Terminal outcome
                         ↑                 ↕
                    Input bindings    Writers / readers
                    Output routing
```

Parsing produces an owned, structurally valid `Plan`. Preparation resolves schemas,
functions, supported operators, resource references, and output routing. Success
produces a complete `PreparedPlan`; partial preparation stays private and cleans up
on failure.

Starting consumes the prepared plan and returns an execution with its output
readers. Initially, prepared plans are single-use. The proposed contract consumes
them on success or operational failure; retries require fresh preparation.

Execution exposes progress, cancellation, and completion operations that remain
meaningful after workers finish. Public objects do not require callers to invoke
initialization and build methods in a particular order. Future I/O, allocation, and
device failures remain normal execution errors.

Input declarations and routing settings become owned domain objects during
preparation. Resolve names and partition keys once. Output routing is an explicit
choice such as single destination, broadcast, or hash partitioning, with valid
cardinality and keys established during construction and binding.

Introduce `Session` when it has a concrete planning responsibility. Prefer immutable
settings where practical; sharing a context between sessions does not imply
concurrent query execution.

## C++ API sketch

These declarations illustrate the eventual API, including live streaming. They are
not a standalone header or a promise that every method ships in the first stage.
Supporting declarations, implementation storage, and ordinary move/destructor
boilerplate are omitted. `ContextConfig` and `Error` refer to the public configuration
and error types.

Owned plans, schemas, and reports are immutable and copyable. Input sources,
writers, readers, prepared plans, and execution control are movable and noncopyable;
they have no public empty constructor. The `&&` qualifiers below make consuming
operations visible at call sites. C++ still requires a moved-from contract.

### Plans, binding, and execution

```cpp
namespace sirius {

template <class T>
using Result = std::expected<T, Error>;

class Schema {
 public:
  // Borrow the foreign schema during the call; return an owned description.
  static Result<Schema> from_arrow(const ArrowSchema& schema);
};

class RecordBatch {
 public:
  // Adopt foreign buffers and their release callback without copying them.
  // Success clears the source array's release callback; failure preserves ownership.
  static Result<RecordBatch> from_arrow(ArrowArray& array, const Schema& schema);
  static Result<RecordBatch> from_arrow_device(
      ArrowDeviceArray& array, const Schema& schema);
};

class Plan {
 public:
  static Result<Plan> from_substrait(std::span<const std::byte> bytes);
};

class ProducerCount {
 public:
  static Result<ProducerCount> from_size(std::size_t count); // nonzero
};

class InputBindings {
 public:
  static Result<InputBindings> one(std::string name, InputSource source);
  static Result<InputBindings> create(std::vector<NamedInput> inputs);
};

class OutputRouting {
 public:
  static OutputRouting single();
  static Result<OutputRouting> broadcast(Destinations destinations);
  static Result<OutputRouting> hash_partitioned(
      Destinations destinations, PartitionKeys keys);
};

class Context {
 public:
  static Result<std::unique_ptr<Context>> create(const ContextConfig& config);
  Result<Session> create_session(const SessionOptions& options) const;
};

class Session {
 public:
  // Consume input endpoints; retain any plan resources needed after return.
  Result<PreparedPlan> prepare(
      const Plan& plan, InputBindings inputs, OutputRouting outputs) const;
};

class PreparedPlan {
 public:
  Result<StartedExecution> start() &&;
};

class CancellationHandle {
 public:
  void request_cancel() const noexcept;
};

class Execution {
 public:
  ExecutionProgress progress() const;
  CancellationHandle cancellation() const;
  Result<ExecutionReport> wait() &&; // blocks; consumes execution control
};

struct StartedExecution {
  Execution execution;
  std::vector<BatchReader> outputs;
};

} // namespace sirius
```

`NamedInput` pairs an owned input name with an `InputSource`. Binding construction
rejects duplicate names; preparation resolves those names against the plan.
Destinations, partition keys, and buffer limits are owned domain values whose
construction details remain to be designed. `single()` yields exactly one reader;
other routing modes establish an explicit destination-to-reader mapping at start.

Preparation consumes supplied input endpoints even on operational failure. Starting
consumes its prepared plan, and waiting consumes execution control on both success
and failure. Cancellation handles remain safe to use after completion.

The first materialized API also needs an input factory accepting completed data and
an execution operation returning completed outputs. Those should have distinct
signatures rather than returning a live writer that must secretly be finished
before `start()` works. Their exact types remain an open design item.

### Channels and streaming outcomes

```cpp
namespace sirius {

struct InputChannel {
  static Result<InputChannel> create(
      const Schema& schema, ProducerCount producers, BufferLimits limits);

  InputSource source;
  std::vector<InputWriter> writers; // exactly the requested producer count
};

struct Accepted {};
struct WouldBlock { RecordBatch batch; };
struct WriteFailure { Error error; RecordBatch batch; };
using TryWriteResult = std::variant<Accepted, WouldBlock, WriteFailure>;

class InputWriter {
 public:
  // Blocking acceptance; failure returns the unaccepted batch.
  std::expected<void, WriteFailure> write(RecordBatch batch);
  TryWriteResult try_write(RecordBatch batch);
  Result<void> finish() &&;
};

struct Pending {};
struct End {};
using ReadPoll = std::variant<RecordBatch, Pending, End, Error>;

class BatchReader {
 public:
  Result<std::optional<RecordBatch>> next(); // blocks; nullopt means clean EOF
  ReadPoll poll();                          // never waits for a batch
};

} // namespace sirius
```

Each write either accepts the whole batch or returns it. Successful acceptance
transfers ownership to the channel; it does not mean the batch has been processed.
A later execution failure is reported through the execution outcome and affected
endpoints. Finishing consumes the writer even if the receiver has already closed.

`next()` and `poll()` use the same terminal contract. After clean EOF, reads continue
to report EOF; after a stream failure, reads continue to report failure. Reading EOF
from one output does not establish overall execution success: callers must also
observe the execution outcome.

EOF here is a protocol outcome, not an initialization flag. No endpoint exposes
`initialize()`, `is_initialized()`, or a separate operation to make it usable.

## Input channels and execution ownership

### Two ends of one channel

Creating an input channel produces one `InputSource` and one or more `InputWriter`s.
The channel has an immutable schema and a fixed producer set. Each writer already
identifies its channel and producer; writes do not repeatedly supply stream or
sender IDs.

An `InputSource` is consumed when binding it into a prepared plan. Its schema can be
shared, but its consumer endpoint cannot be attached to two independent executions.
Multiple consumers require explicit broadcast or another routing operation.

```mermaid
flowchart LR
    A[Producer A owns InputWriter] --> Channel[Input channel]
    B[Producer B owns InputWriter] --> Channel
    Source[InputSource] -->|consumed during preparation| Prepared[PreparedPlan]
    Prepared -->|consumed by start| Execution[Execution]
    Channel -->|bound consumer endpoint| Execution
    Execution --> Reader[BatchReader]
```

Channel creation establishes the schema, producer capabilities, and queue limits
without requiring a context. Preparation binds its consumer endpoint to execution
resources; starting activates consumption. Queued batches retain their own storage.
If a future channel needs engine-managed spilling or memory reservations at creation,
make that resource dependency explicit in its factory arguments.

### Example: producers feeding an execution

This C++ example uses the proposed signatures above. The caller constructs a valid
producer count (for example, `ProducerCount::from_size(2)`) and buffer limits.
`BatchSources` and `pump_and_wait` belong to the embedding application, not Sirius.

```cpp
Result<ExecutionReport> execute_streamed(
    const Session& session,
    std::span<const std::byte> bytes,
    const Schema& schema,
    ProducerCount producers,
    BufferLimits limits,
    BatchSources batches)
{
  auto plan = Plan::from_substrait(bytes);
  if (!plan) { return std::unexpected(std::move(plan.error())); }

  auto channel = InputChannel::create(schema, producers, limits);
  if (!channel) { return std::unexpected(std::move(channel.error())); }

  auto inputs = InputBindings::one("orders", std::move(channel->source));
  if (!inputs) { return std::unexpected(std::move(inputs.error())); }

  auto prepared = session.prepare(*plan, std::move(*inputs), OutputRouting::single());
  if (!prepared) { return std::unexpected(std::move(prepared.error())); }

  auto started = std::move(*prepared).start();
  if (!started) { return std::unexpected(std::move(started.error())); }

  return pump_and_wait(
      std::move(*started), std::move(channel->writers), std::move(batches));
}
```

`pump_and_wait` owns the started execution and endpoints. It assigns one writer to
each producer, writes batches, consumes each successful producer with
`std::move(writer).finish()`, and drains output readers concurrently. Once input
production and output draining complete, it obtains the report through
`std::move(execution).wait()`.

On producer or consumer failure, the helper requests cancellation before joining
blocked tasks. It also handles partial task-launch failure. This coordination is
shown as an application helper because the public API should not prescribe a
thread pool or async runtime. Any eventual convenience runner must implement the
same cleanup guarantees.

The name `"orders"` identifies a plan input during preparation. Subsequent writes
operate directly on the bound channel. Moving a capability transfers ownership;
it does not copy queued data.

### Resource lifetimes

The consumer endpoint moves through `InputSource → PreparedPlan → Execution`.
Writers remain independently owned by producer tasks throughout that transition.

| Owner | What it retains |
|---|---|
| `InputSource` | Consumer endpoint and channel resources before preparation. |
| `PreparedPlan` | Bound consumer endpoints and resources needed to start. |
| `Execution` | Active consumers, worker lifetime, and cancellation/join responsibility. |
| `InputWriter` | Producer endpoint and resources needed to write or release pending data. |
| Returned batch | Resources needed to access and release its buffers. |

A writer must not keep an abandoned execution running. It retains channel storage
and necessary resource leases, without retaining execution control. Destroying the
execution cancels channels and joins workers; surviving writers observe receiver
closure. Avoid ownership cycles between execution control and channel storage.

| Event | Proposed behavior |
|---|---|
| Unbound source is dropped | Close the consumer endpoint; writers observe receiver closure. |
| Preparation fails after accepting the source | Release the consumer endpoint and wake affected writers. |
| Prepared plan is dropped without starting | Close its bound consumer endpoints. |
| Starting fails | Release acquired execution resources and close affected channels. |
| Writer finishes | Mark that producer complete and consume its capability. |
| Unfinished writer is dropped | Abort the input rather than report successful EOF. |
| Execution is cancelled or destroyed | Wake blocked endpoints and terminate workers safely. |
| Execution finishes early, such as for a limit | Close unused inputs; producers observe closure even if execution succeeded. |

Receiver closure does not necessarily mean query failure. The execution's terminal
outcome remains authoritative. Clean input EOF requires every declared producer to
finish and all queued batches to be consumed.

### Buffering and backpressure

Blocking reads distinguish `Batch`, `End`, and `Error`; nonblocking polling also
distinguishes `Pending`. An empty queue is not EOF. Document repeated reads after a
terminal outcome.

A write reports acceptance, backpressure, or failure. If ownership was not accepted,
return the batch. Document batch ownership for every outcome.

Live channels may buffer data before execution starts, subject to capacity. Blocking
writes can stall without a running consumer, so examples should start execution
before launching blocking producers. Waiting for completion may also block while
inputs remain open or outputs remain undrained.

The first materialized implementation uses completed input data with a sealed
producer side. Do not implement materialization by filling a bounded live channel
before starting its consumer.

## Arrow interoperability

Use the Arrow C Data Interface for CPU schema and batch interchange. Evaluate the
[Arrow C Device Data Interface](https://arrow.apache.org/docs/format/CDeviceDataInterface.html)
for GPU-resident batches, including device identity and producer synchronization.
It supports sharing compatible device buffers between libraries without a host
copy; the specification currently marks it experimental.

Define supported types, layouts, and devices; borrowing versus ownership transfer;
synchronization before device access; buffer and release-callback lifetimes; and
when conversion or device transfer is required. Foreign callbacks may be
thread-affine and need an explicit contract before Rust wrappers can be `Send`.

`RecordBatch::from_arrow` and `from_arrow_device` adopt supplied buffers and release
callbacks independently of a Sirius context. They check the supported representation
and schema before accepting ownership. Adoption does not copy data onto Sirius's
GPU or allocate from its engine memory pools; foreign batches may exist before a
context does.

Execution establishes device compatibility and synchronizes before accessing device
buffers. Required conversions or transfers follow an explicit policy. Any future
operation copying data into engine-owned memory must expose its resource dependency.
Arrow adapters also export reader outputs. Local Sirius-to-Sirius transfer should
accept native owned batches, preserving GPU buffers where possible without an
unnecessary export/import cycle. Cancellation and backpressure remain Sirius API
contracts.

## Cancellation and parent lifetimes

Cancellation requests are concurrent and idempotent, wake blocked endpoints, and do
not imply immediate termination. Dropping an execution requests cancellation and
waits for safe worker termination; destruction may block. Teardown must not require
the caller to continue draining outputs. Detached execution is not the default.

**Decision required:** adopt retained runtime ownership or explicit parent-borrowing
requirements. Retained ownership is the preferred proposal:

- Sessions and executions retain required engine resources.
- Endpoints retain channel storage.
- Exported batches retain resources needed for safe release.

Dropping a public context handle may therefore not immediately destroy the engine.
The one-active-context restriction applies until the retained runtime is destroyed.
Agree on this lifetime change before implementation, and apply the same model
across C, C++, and Rust.

## Language and thread contracts

Use distinct opaque C handles, owning C++ wrappers, and consuming Rust transitions.
In C, consuming operations clear the caller's handle slot when accepting ownership;
required-argument checks precede transfer. Document failure ownership and prohibit
stale aliases to consumed handles. C++ needs explicit moved-from contracts; Rust
can consume `self` to enforce lifecycle transitions.

Use the shared public error/status model, with operation-specific outcomes where
ownership requires them, such as returning an unaccepted batch.

The following thread contracts are targets requiring an implementation audit:

| Types | Intended contract |
|---|---|
| Owned plans, schemas, reports | Immutable and shareable: `Send + Sync`. |
| Context | Shared services after audit; execution may still serialize. |
| Session, prepared plan, execution | Transferable ownership; initially no concurrent calls on one object. |
| Writer or reader | One caller per endpoint; distinct endpoints may operate concurrently. |
| Cancellation handle | Concurrent cancellation requests. |
| Foreign batches | Depends on buffer, callback, and device ownership contracts. |

Every public type and method must document ownership and parent lifetime, thread
transfer and concurrent use, destruction threads, argument consumption and failure
ownership, blocking behavior, and callback/reentrancy rules. Moving ownership
between threads includes destruction on the destination thread. An opaque pointer
or internal mutex alone does not establish thread safety.

## Mapping the existing StarRocks integration

### Current service path

The [StarRocks engine adapter](../experimental/starrocks/src/engine.rs) sends
serialized Substrait to a dedicated engine thread, calls `execute_substrait`, and
collects the full result into owned Arrow batches. The dedicated thread keeps the
current `!Send`/`!Sync` context on one thread.

The first migration can preserve this arrangement:

```text
execute_substrait(bytes)
    → parse → prepare → execute → collect output
```

Later, the adapter can drain a `BatchReader` incrementally into its response
mechanism. Changing the public API alone does not make the service stream results.

### Existing fragment FFI

The separate [fragment FFI](../include/sirius/ffi.hpp) expresses related concepts
through declarations, IDs, and lifecycle-sensitive methods:

| Current FFI operation | Proposed equivalent |
|---|---|
| `declare_input_column(stream_id, name, type)` | Construct an owned schema and input channel. |
| `declare_input_sender(stream_id, sender_id)` | Create explicit producer writer capabilities. |
| Implicit sender `0` | Explicitly request one writer. |
| Plan references `sirius_stream_<id>` | Resolve the plan input through preparation bindings. |
| `build(plan)` | Parse a plan, then prepare complete bindings and routing. |
| `relay_from(source, output_id, input_id, sender_id)` | Transfer completed output batches to the corresponding input and finish that producer. |
| `close_input(stream_id, sender_id)` | Consume that producer's writer with `finish()`. |
| `run()` | Start/execute a prepared plan. |
| `result_to_arrow()` | Read output through an Arrow adapter. |
| Output and partition declarations | Construct explicit output routing. |

The current [relay implementation](../src/exec/streaming_fragment.cpp) requires the
source to have completed, moves queued batch references, and closes the specified
sender. The receiver cannot run until every input is closed. Preserve that ordering
in the materialized implementation:

```text
Execute producer → obtain completed output → bind/transfer completed input → execute consumer
```

Completed output needs independent ownership so releasing the producer's execution
handle does not invalidate transferred batches. The integration adapter retains
mappings from StarRocks input IDs to plan bindings and sender IDs to writers.
Network routing and duplicate-message handling remain adapter responsibilities.

Internally, [batch_stream](../src/exec/batch_stream.hpp) provides queue, sender
completion, and wakeup behavior. However,
[stream_session](../src/exec/stream_session.hpp) routes through non-owning operator
pointers. Public endpoints require channel lifetime to be separated from operator
lifetime. This internal router is not the proposed public planning `Session`.

## Engine work and delivery

### 1. Ownership and internal lifecycle

- [ ] Settle retained runtime, endpoint, and batch lifetimes.
- [ ] Separate parsed plans, bound preparation, and mutable execution state.
- [ ] Define failure cleanup, cancellation, and terminal outcomes.
- [ ] Audit resource snapshots and thread transfer.
- [ ] Preserve existing integrations.

Prepared plans must retain stable resource snapshots or report stale-resource
errors when starting. They must not silently execute against changed bindings.

### 2. Plan-first materialized execution

- [ ] Parse Substrait into owned plans.
- [ ] Prepare explicit input bindings and output routing.
- [ ] Execute with completed inputs and materialized outputs.
- [ ] Add C ABI, C++ wrappers, and Rust wrappers.
- [ ] Establish Arrow import/export contracts and supported representations.
- [ ] Publish examples and ownership/thread-safety documentation.

### 3. Live streaming

- [ ] Add live writers and readers.
- [ ] Implement bounded backpressure and batch ownership outcomes.
- [ ] Implement cancellation and endpoint teardown.
- [ ] Support external producers feeding active executions.
- [ ] Resolve scheduling before enabling live local execution chaining.

The current exclusive query slot can deadlock if a consumer occupies it while
waiting for another Sirius execution to produce its input. Initially, chain local
executions through completed, potentially spillable results. Live chaining requires
a shared execution graph or sufficient execution-state isolation and scheduling.

### Later follow-ups

Reusable prepared templates, SQL preparation, additional session/catalog
capabilities, concurrent execution, and independent contexts.

## Verification

Alongside functional execution, cover:

- Cross-thread transfer and destruction; concurrent independent endpoints.
- Blocked-reader/writer cancellation and dropped endpoints.
- Preparation, partial-start, and late execution failures.
- Batch ownership after rejected writes and early consumer completion.
- Exported batches surviving reader or execution release.
- Arrow device synchronization and release callbacks.
- Stale prepared resources and local producer/consumer scheduling.
- Rust trait checks and C/C++ ownership-contract tests.

Each stage ships matching C, C++, and Rust documentation and examples for the
capabilities it implements.
