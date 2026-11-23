# Plans from other systems: Velox and Substrait

A plan from another engine can tell Sirius to join two tables and still leave a crucial
question open. What does the host expect if a 64-bit sum overflows? Translation is safe only
when Sirius can preserve the host's result, including errors, ordering and the layout of
values exchanged with the host.

This chapter sketches two adapters. The first takes physical plan nodes from Velox or a
Velox-based system. The second takes Substrait, which describes relational plans independently
of an engine. Upstream already accepts Substrait through the older execution bridge. A
consumer built on the public physical plan builder is tracked separately. The adapter names
and signatures below are proposals. [Velox](https://github.com/facebookincubator/velox) · [Substrait](https://substrait.io/) · [Sirius consumer issue](https://github.com/sirius-db/sirius/issues/2192)

## What a description carries

A Sirius plan states more than either description does. The converter's caller fills the
difference through options.

| A Sirius plan needs | Velox plan nodes | Substrait |
|---|---|---|
| Relational operators and expressions | Yes | Yes |
| Types and nullability | Velox types, with nullability supplied or inferred separately | Substrait types with declared nullability |
| Function identity | A name in Velox's Presto or Spark function library | An extension identifier and function declaration |
| Exchanges between workers | Axiom's multi-fragment plan connects both ends. A single fragment can name a remote edge without its peer | Optional exchange relations |
| Worker sets and their sizes | Counts or constraints from the fragment scheduler | An exchange can specify partition count and targets. It has no Sirius worker-set identity. |
| Scan splits per worker | Supplied by the host next to the plan | File lists with byte ranges, or a table name to resolve |
| Partial aggregate state types | Velox's own intermediate layouts | An aggregation phase per measure. A decomposable function declares its intermediate output type |
| Runtime filters and adaptive decisions | Depends on the producer | Hints may refer to a saved Bloom filter. They do not specify the Sirius merge, wait or decision policy. |

Whatever the converter derives or the caller supplies ends up in the plan, where `explain()` shows
it and the validator checks it.

## Velox plans

### The Velox plan adapter

The proposed `sirius::velox` adapter would expose a C++ interface to Velox plan nodes.
It reads Velox plan nodes and makes builder calls. If a node cannot be translated, it returns an
`unsupported` error that names the node by its Velox plan node id.

It accepts three kinds of input:

1. **One plan fragment**, a `core::PlanNode` tree with no remote exchanges. It becomes a plan on
   `local[1]`, or one instance plan.
2. **Axiom's multi-fragment plan.** The fragments and the links between them become one closed
   joined plan.
3. **A serialized Velox plan** from a file, a test corpus or another Velox-based system. Velox
   reads it back into plan nodes through its own registry, and it is then input 1 or 2. The
   serialized form carries no splits and changes with Velox, so the adapter is pinned to the Velox
   version it was built against.

```cpp
// C++ only: the sirius::velox adapter reading Axiom's plan.
#include <sirius/sirius.hpp>
#include <sirius/velox.hpp>
namespace axo = facebook::axiom::optimizer;

sirius::Result<sirius::Plan> to_sirius(const axo::PlanAndStats& planned,
                                       const sirius::WorkerSet& compute,
                                       const sirius::WorkerSet& head,
                                       sirius::velox::ScanSplits splits) {
  auto options = sirius::velox::AdapterOptions::on(compute, head)
                     .with_connector("hive", sirius::velox::hive_parquet())
                     .with_splits(std::move(splits));   // per TableScanNode id: every split
  return sirius::velox::translate(*planned.plan, std::move(options));
}
```

Table handles and splits are connector-specific, so each connector needs a translator registered
with the adapter. Hive tables over Parquet come first: a Hive table handle becomes a
`TableSource::parquet`, and a split (a file path, a start offset and a length) becomes a Sirius
`Split` with a byte range.

`ScanSplits` maps each `TableScanNode` id to the full list of splits its connector enumerated. A
Sirius scan's splits are fixed when the plan is built, so the caller drains every split source
first. By default the adapter deals each list over the scan's set with `Shards::explicit_splits`;
with `with_dynamic_splits()` it uses `Shards::dynamic()`.

The example adapter defaults to one worker for an exchange-free plan. Distributing that plan requires the caller to choose the exchanges. Where the adapter lives and what it covers is
[open question 32](../reference/open-questions.md#32-a-velox-plan-adapter).

### Mapping Velox's vocabulary

| Velox | Sirius Plan IR |
|---|---|
| A fragment bounded by exchanges | A stage when the fragment boundaries match Sirius exchange boundaries |
| `PartitionedOutputNode` on keys, with its `ExchangeNode` | `Exchange::hash(keys, T)` with `HashScheme::v1`. Both ends are Sirius, so the hash need not match Velox's |
| `PartitionedOutputNode` broadcast, arbitrary, single | `Exchange::broadcast(T)`, `Exchange::round_robin(T)`, `Exchange::gather(head)` |
| `MergeExchangeNode` | `Exchange::gather(head, GatherOrder::merge(keys))` over sorted input |
| `LocalPartitionNode`, `LocalMergeNode` | Worker-local routing can be omitted if no consumer relies on it. A local merge that establishes ordering must preserve that order |
| `AggregationNode` single, partial, intermediate, final | `aggregate`, `aggregate_partial`, `aggregate_combine`, `aggregate_final`, with Sirius state types. A partial and final pair with only a local exchange between them becomes one `aggregate` |
| `GroupIdNode` directly below an `AggregationNode` | `aggregate_sets`: preserve the grouping-set order and cast the zero-based ordinal to Velox’s `BIGINT`. SQL’s `GROUPING` bitmask is a separate projection |
| `WindowNode` | `Window`, for the three forms the IR defines |
| `HashJoinNode` | `Join` for supported key, residual and null semantics. Its mode is derived from the inputs: `broadcast_build` over a broadcast build, `co_partitioned` over two matching hash exchanges, `single` on one worker |
| `TopNNode`, `LimitNode`, `OrderByNode` | `TopN`, `Limit`, `Sort` per worker, a gather, and the final step on `head[1]` |
| `TopNRowNumberNode` that emits no row number | `top_n_per_key` |
| `TableScanNode` with its splits | `Relation::scan` over a `TableSource`, with `explicit_splits` or `dynamic` shards |
| Filters pushed into the table handle | A `Filter` above the `Scan`; the caller may run `rewrite::push_filters_into_scans` before preparing |
| `ValuesNode` | `Relation::values` |
| A `ValueStreamNode` fed by the host | `Input`, bound as a completed input |

### A multi-fragment plan from Axiom

Axiom is a set of C++ components for building front ends to Velox: SQL and DataFrame front ends,
a logical plan, a cost-based optimizer and connectors. Its optimizer decides where exchanges go
and how each one partitions. The output is a `MultiFragmentPlan`: a tree of fragments of Velox
plan nodes, each with a type that says how many tasks run it (`kSource`, `kFixed`, `kSingle`).
Execution is left to a pluggable `Runner`; the open-source one runs every fragment in one process.

Two worker sets suffice for the illustrated plan. Fragment counts, partition assignments and the number of outputs must still agree.

| Axiom | Sirius Plan IR |
|---|---|
| `MultiFragmentPlan` | One closed, joined `Plan` |
| `kSource` and `kFixed` fragments | `compute[N]`, with N the GPU count the caller planned for (`maxRemotePartitions = N`) |
| `kSingle` | `head[1]` |
| The link from an `ExchangeNode` to the fragment that feeds it | The edge of one `Exchange` node |
| One `TaskStats` per fragment | The `ExecutionReport`, one stage per fragment |

Take Q7 from [A multi-stage query](../part-3/07-a-multi-stage-query.md#the-question), planned for
four GPUs. Suppose the optimizer broadcasts the filtered customers and hashes both sides of the second join on order key. This gives five illustrative fragments:

```text
A  kSource            PartitionedOutputNode broadcast
                      └─ TableScanNode customer, filter c_mktsegment = 'BUILDING'
B  kSource            PartitionedOutputNode kPartitioned(o_orderkey), 4 partitions
                      └─ HashJoinNode o_custkey = c_custkey
                         ├─ TableScanNode orders, filter o_orderdate < 1995-03-15
                         └─ ExchangeNode <- A
C  kSource            PartitionedOutputNode kPartitioned(l_orderkey), 4 partitions
                      └─ TableScanNode lineitem, filter l_shipdate > 1995-03-15
D  kFixed, 4 tasks    PartitionedOutputNode single
                      └─ TopNNode 10, partial
                         └─ AggregationNode single, keys l_orderkey, o_orderdate, o_shippriority
                            └─ ProjectNode rev = l_extendedprice * (1 - l_discount)
                               └─ HashJoinNode l_orderkey = o_orderkey
                                  ├─ ExchangeNode <- C
                                  └─ ExchangeNode <- B
E  kSingle            TopNNode 10, final
                      └─ ExchangeNode <- D
```

The adapter walks the fragments from the leaves up and produces the joined plan of
[Q7 in Part III](../part-3/07-a-multi-stage-query.md), with three differences that are Axiom's
choices:

- The revenue projection sits above the join, so the lineitem exchange carries `l_extendedprice`
  and `l_discount` instead of `rev`.
- The shards are `explicit_splits`, because the caller enumerated the splits through Axiom's
  connector.
- There are no runtime filters.

`rewrite::split` cuts that plan at its four exchanges, the places where Axiom cut its fragments.
Every distribution fact is Axiom's, and the validator checks each: the join is `co_partitioned`
because both exchanges hash an int64 key into the same four partitions (V8), and the single-phase
`Aggregate` is legal because its input is hashed on a group key (V6). A structurally invalid translation fails validation. A translator that chooses the wrong column consistently can still produce a valid plan with the wrong meaning.

The caller is an Axiom `Runner` subclass. It prepares the plan with `Bindings::local()`, reads the
one `Output` on `head#0`, and turns each batch into a Velox `RowVector` through the Arrow C data
interface. Fallback happens once, before anything runs: if the adapter or `prepare` returns
`unsupported`, or `runtime_unavailable`, Axiom's own runner executes the whole query.

### Runtime filters across fragments

Consider adding a filter to the join between line items and orders. If each task receives
the complete build side, a filter built by one task covers all orders. If the build side is
partitioned, one task's filter covers only its partition. Pruning the lineitem scan before its
exchange requires contributions from every build partition.

The proposed translator could add `RuntimeFilterBuild` at the orders input, merge compatible
Bloom filters with a `ValueExchange`, and apply the merged filter at the lineitem scan.
`wait=never` lets that scan start immediately. A bounded wait gives the build a head start.
Neither choice changes which rows the join is allowed to return. [Runtime filters](../part-2/05-runtime-filters.md)

An adaptive broadcast would defer a second decision. Where the
estimate is close to the limit, the caller can build the join with
`JoinOptions::with_adaptive_broadcast`, which leaves the choice to measured bytes
([Adaptive plans](../part-2/06-adaptive-plans.md)). The join output is then `arbitrary`, so an
aggregate above it must be two-phase (V6).

## Substrait

The proposed `sirius::substrait` adapter translates supported relations to a Sirius plan and identifies the first unsupported relation. Its scope is part of [open question 32](../reference/open-questions.md#32-a-velox-plan-adapter).

```cpp
#include <sirius/sirius.hpp>
#include <sirius/substrait.hpp>

// bytes: a serialized Substrait plan. tables: host code that resolves a table name.
sirius::Result<sirius::Plan> from_substrait(std::span<const std::byte> bytes,
                                            const sirius::WorkerSet& local,
                                            sirius::substrait::Tables tables) {
  auto options = sirius::substrait::AdapterOptions::on(local).with_tables(std::move(tables));
  return sirius::substrait::translate(bytes, std::move(options));
}
```
```rust
// bytes: a serialized Substrait plan. tables: host code that resolves a table name.
fn from_substrait(bytes: &[u8], local: &WorkerSet,
                  tables: sirius_substrait::Tables) -> sirius::Result<Plan> {
    let options = sirius_substrait::AdapterOptions::on(local).with_tables(tables);
    Ok(sirius_substrait::translate(bytes, options)?)
}
```

### Mapping Substrait's relations

| Substrait | Sirius Plan IR |
|---|---|
| `ReadRel` over a named table | `Relation::scan` over the `TableSource` the caller's table resolver returns |
| `ReadRel` over local files, each with a start and a length | `Relation::scan` with `Shards::explicit_splits` |
| `ReadRel` over a virtual table | `Relation::values` |
| A filter or projection inside a `ReadRel` | A `Filter` or `Project` above the `Scan` |
| `FilterRel`, `ProjectRel` | `Filter`; for a projection, pass through the input fields, append the computed fields, then apply any emit mapping |
| `AggregateRel` with one grouping | `Aggregate` |
| `AggregateRel` with several groupings | `Aggregate` with grouping sets |
| `JoinRel` | `Join`: equality terms of the condition become key pairs, the rest the residual |
| `SortRel`, `FetchRel` | `Sort`, `Limit`; an ordered fetch with an offset must preserve the skipped rows, for example with a top-(offset + count) followed by the slice |
| `SetRel` | `Union`, or the [set-operation compositions](../part-1/07-aggregates-joins-and-ordering.md#set-operations) |
| `ConsistentPartitionWindowRel` | `Window`, for the three forms the IR defines |
| `ExchangeRel` | Translate supported scatter, broadcast and round-robin semantics with their specified count and targets. A bucket expression may choose a different target for each row. Only a constant single destination is a gather |
| The emit list of any relation | A `Project` that selects and orders the columns |
| A type with its nullability | A `DataType` and a required or nullable `Field` |
| A function reference resolved through its extension declaration | A function looked up in the caller's `Registry` |
| The plan's root names | Name the projected fields in order before constructing the output |

### What Substrait leaves to the caller

- **Where the plan runs.** Substrait has no worker sets. The example adapter maps an exchange-free plan to `local[1]`. A caller targeting several GPUs must choose a distribution when the incoming plan leaves it open.
- **Agreement between workers.** Substrait's `ExchangeRel` has a partition count and can map
  partition IDs to targets. Its scatter-by-fields form names fields without fixing a hash
  algorithm. Map the declared routing to worker sets and bindings. Reject a route the adapter
  cannot reproduce, including an exact round robin if the Sirius exchange only balances
  approximately. [Substrait exchange definition](https://github.com/substrait-io/substrait/blob/main/proto/substrait/algebra.proto#L475-L525)

- **Aggregate state.** A decomposable Substrait aggregate declares its intermediate output type.
  The call specifies the phase. If an intermediate result is visible to another engine, the
  converter must produce the type and values that engine expects. When Sirius owns the partial,
  merge and final operations and the state is private to them, it can use its own representation.
  [Substrait aggregate functions](https://substrait.io/expressions/aggregate_functions/)
- **Function semantics.** Resolve the function and its options against the referenced extension, including overflow and rounding behavior. Each function the converter accepts maps to a built-in or registered function with the
  same behavior, and anything else is `unsupported`
  ([Dialect semantics](#dialect-semantics)).
- **Types.** Substrait types without a `DataType` (time, timestamp with time zone, interval, map,
  binary) are `unsupported` ([Additive later](../reference/extension-points.md#additive-later)).
- **Runtime filters and adaptive choices.** Substrait has hints for saving and loading computations such as Bloom filters. The adapter still needs scope and completion rules to turn a hint into a safe filter edge. A host can add Sirius decision policies explicitly.

## One task at a time: Bolt under Spark

[Bolt](https://github.com/bytedance/bolt) is a C++ execution library derived from Velox, with no
optimizer or scheduler. Its main use is as a native backend for Spark through
[Apache Gluten](https://github.com/apache/gluten). Spark divides the work into stages at shuffle boundaries. A task processes one partition of a stage. An executor may run several tasks concurrently.
Gluten builds the task's plan nodes and attaches the file splits Spark assigned to it.

That task plan is the adapter's first kind of input: one fragment with no remote exchanges. It
becomes one closed plan on `local[1]`. If the whole task translates, Sirius runs it on the GPU in
place of the Bolt task, and its output goes to the same places through Arrow. Spark keeps its
shuffle, adaptive re-planning, retries and speculation. Mixed execution is safe only if either engine produces an output the next stage can read with the same meaning.

| Bolt or Spark | Plan IR |
|---|---|
| One Spark task | One closed plan on `local[1]`, prepared and started once |
| The task attempt id | An `ExecutionId` the host encodes from the application id and the attempt id |
| The task's file splits | `Shards::explicit_splits`, with one list for worker 0 |
| A shuffle read, a broadcast variable, a JVM iterator | `Input`, bound as a completed input |
| The hash column and Bolt's shuffle writer | A `Project` that computes the partition id, then `Output`; the writer stays the host's |
| A runtime filter, `might_contain(<bloom bytes>, xxhash64(k))` | A `Filter` that probes a constant Bloom filter; no value edge |
| Shuffles, adaptive re-planning, retries, speculation | The host's; nothing in the plan |

Take Q4 from
[Partitioned join and runtime filters](../part-3/04-partitioned-join-and-runtime-filters.md),
with a shuffled join on the order key and 200 shuffle partitions. One map task of the lineitem
stage translates to:

```text
Plan "q4_lineitem_task" (joined)
worker sets: local[1]
Output "to_shuffle" on local[1]
└─ Project [pid = spark.pmod(spark.hash(l_orderkey), 200), l_orderkey, l_extendedprice,
   │        l_discount]
   └─ Filter spark.might_contain(<binary 2MiB>, spark.xxhash64(l_orderkey))
      └─ Scan lineitem [l_orderkey, l_extendedprice, l_discount]  on local[1]
                       shards=explicit_splits
```

Everything this task must agree on with other tasks is in the `Project`: its `pid` column decides
which reduce task receives a row. Spark's shuffle moves the row, so the edge between a map task
and a reduce task is a
[host-carried edge](../part-2/04-stages-and-bindings.md#host-carried-edges). The reduce task is a
plan with two `Input` leaves, a `Join` in `mode=single` and a single-phase `Aggregate`, because
every row of both inputs is on its one worker.

The Bloom filter arrives resolved. Spark's driver evaluates it before the lineitem stage runs, so
the task sees a constant, and the plan has no value edge, scope or wait.

If a task fails the support check before starting, the host can use its existing CPU path. The translator checks the operators, types and function semantics, and `Context::check` checks engine support. A host can cache checks shared by tasks, but each task's bindings and device still need validation.

### Dialect semantics

A Sirius task must produce exactly what Bolt's CPU path would for the same task. Otherwise a stage
whose tasks ran on two engines writes inconsistent shuffle data. A host's *dialect* (its
functions, types and overflow rules) differs from Sirius's built-in one in small ways that matter.
The same holds for Presto's dialect behind Axiom, and for any Substrait producer.

| Area | Spark | Sirius built-in | Translator |
|---|---|---|---|
| Partition ids | `pmod(murmur3(keys, 42), n)`, hashing each key by its Spark type | `HashScheme::v1` | `spark.hash` and `spark.pmod` in a `Project`; never a hash exchange |
| Runtime filter probes | `xxhash64` with seed 42, Spark's Bloom filter format | The `bloom` filter kind, engine-defined | `spark.xxhash64`, `spark.might_contain` |
| Decimal arithmetic | Operands widened, precision and scale derived by Spark's rules | Operands of one type and [no implicit conversion](../part-1/04-schemas-and-expressions.md#no-implicit-conversion) | Explicit casts to Spark's operand and result types |
| Integer `sum` | Returns `bigint`; overflow raises an error under ANSI mode and wraps without it | Returns `decimal(38,0)` | Use an aggregate with Spark's update and merge semantics, or prove every intermediate fits |

A final cast cannot reproduce an intermediate overflow. On one task, adding 1 to the largest
signed 64-bit value overflows even if a later row contributes −1. A wider accumulator would
finish with an in-range total and miss the ANSI error. [Spark sum implementation](https://github.com/apache/spark/blob/master/sql/catalyst/src/main/scala/org/apache/spark/sql/catalyst/expressions/aggregate/Sum.scala)

- **Translator casts.** When Sirius computes the right values in a different type, the translator
  adds a `Project` with a cast.
- **Registered functions.** When the values themselves differ, as with Murmur3, `xxhash64`, Bloom
  probes or a wrapping sum, the function must be the host's, registered under its own namespace in
  the translator's `Registry`
  ([Closed and open lists](../reference/extension-points.md#closed-and-open-lists)). Its identity
  is in the fingerprint, and an identity this engine build does not implement fails `check` (V21).

Aggregate state needs the same care. When Spark puts a shuffle between a partial and a final
aggregate, the state columns cross an edge Sirius does not own, and Spark's buffers are not
Sirius's [state types](../reference/node-catalog.md#aggregate-state-types). Either both halves run
on Sirius, which gives up per-task fallback for those two stages, or the states are registered
Spark-compatible state types that either engine can read. Using Sirius states at both ends would also require a way to declare a state-typed input after the shuffle. Which dialect
items become registered functions and state types is
[open question 31](../reference/open-questions.md#31-dialect-semantics).

### Many tasks, one GPU

A Spark executor runs several tasks at once, and each may want the same GPU.

- **One `Context` per executor process**, shared by every task in it. Each task prepares its own
  plan. Concurrent executions on one engine, which this needs, are part of
  [open question 15](../reference/open-questions.md#15-one-context-per-process).
- **Admission in `start`.** A task's plan has no peers, so `start()` returns when this process
  admits the execution. A task that is not admitted waits there while its Spark slot stays busy.
- **Quotas as prepare options.** A task's share of GPU memory and spill space is a
  `PrepareOptions` setting, never a plan fact
  ([Quotas and priority](../part-5/08-running-at-scale.md#quotas-and-priority)).

When Spark kills a task, the host calls `request_cancel()` on the execution.

## Other levels

A converter can meet a system at other levels than the two above.
[Beyond three hosts](04-one-ir-three-integrations.md#beyond-three-hosts) puts them side by side.

- **Open instance plans under a foreign runtime.** A worker that receives one task of a fragment,
  from a distributed Axiom runtime or a Presto coordinator, could build an open instance plan for
  it, as a [StarRocks](02-starrocks.md) compute node does. Two things block this. Every worker on
  a Sirius exchange must be a Sirius worker, because `HashScheme::v1` is not Velox's or Presto's
  hash and Sirius does not speak their exchange protocols
  ([open question 18](../reference/open-questions.md#18-hash-scheme-stability)). Those runtimes
  also stream splits into running tasks, while a Sirius scan fixes its splits when the plan is
  built ([open question 30](../reference/open-questions.md#30-host-fed-split-channels)).
- **A pushdown connector.** Axiom offers each subtree whose tables belong to one connector to that
  connector. A "sirius" connector would translate the subtree, run it as a closed plan when its
  virtual table is scanned, and return the rows. The offered subtree carries no distribution
  decisions, so it runs on `local[1]` unless the connector writes the exchanges.
- **Operator by operator.** Velox and Bolt let a driver adapter replace operators inside a task,
  and Velox's cuDF backend works this way. A Sirius adapter there would pay a copy at every
  boundary, see half a join at a time, and keep nothing distributed or adaptive. For a GPU under
  an existing Velox task, the cuDF backend is the shorter path
  ([The Velox family](../appendix/related-designs.md#the-velox-family)).

## Gaps

A translator can report these cases before submitting a task.

| Gap | In this design | Where it is tracked |
|---|---|---|
| Types: time, timestamp with time zone, interval, map, binary | No `DataType` yet; such a plan is `unsupported`. Spark's Bloom filter literal needs the binary type | [Additive later](../reference/extension-points.md#additive-later) |
| Window functions and frames outside the three `Window` forms; generators | No form | [Open question 29](../reference/open-questions.md#29-windows-over-a-global-order-prefix-value-mode) |
| Writes | The proposed public IR has no write node. Upstream SQL supports [Parquet output](https://github.com/sirius-db/sirius/pull/2575). | [Additive later](../reference/extension-points.md#additive-later) |
| Dialects: function names and behavior, hashing, overflow, decimal rules, aggregate state layouts | Translator casts or registered functions; until they exist, the plan or task falls back | Open questions [16](../reference/open-questions.md#16-expression-semantics) and [31](../reference/open-questions.md#31-dialect-semantics) |
| State-typed inputs | The API cannot declare an `Input` with state types, which a reduce task that receives Sirius states needs | [Open question 31](../reference/open-questions.md#31-dialect-semantics) |
| Null-aware joins that also replicate one arbitrary row | `NullRouting::to_all` covers the nulls; a global build row count through `JoinOptions::with_build_rows` gives the same fact | [Open question 17](../reference/open-questions.md#17-join-coverage) |
| Bucketed tables read without a shuffle | The converter returns `unsupported` unless asked to insert a hash exchange | [Open question 13](../reference/open-questions.md#13-hints-and-certified-properties) |
| Splits streamed into running tasks | The caller drains every split source before building the plan | [Open question 30](../reference/open-questions.md#30-host-fed-split-channels) |
| Live inputs | Shuffle reads and host iterators bind as completed inputs | Open questions [20](../reference/open-questions.md) and [21](../reference/open-questions.md#21-input-buffer-limits) |
| Many executions per GPU | Executions on one engine may run one at a time | [Open question 15](../reference/open-questions.md#15-one-context-per-process) |
| Scope and home of the converters | `sirius::velox` and `sirius::substrait`, libraries outside the core Sirius API | [Open question 32](../reference/open-questions.md#32-a-velox-plan-adapter) |
