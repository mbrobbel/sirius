# Plans from other systems: Velox and Substrait

> [!TIP]
> **TL;DR**
>
> - Velox plan nodes and Substrait are plan descriptions, not hosts. A converter library turns
>   one into builder calls and returns an ordinary `Plan`.
> - The description supplies operators, types and functions. The converter's caller supplies what
>   it lacks: worker sets, scan splits, and often the exchanges.
> - Axiom's multi-fragment plan arrives with every exchange chosen, so it becomes one closed plan:
>   fragments become stages, shuffles become `Exchange` nodes.
> - A Substrait plan usually has no exchanges and runs on `local[1]`; one with exchange relations
>   maps them onto sets the caller names.
> - Bolt under Spark is the smallest case: one closed single-worker plan per task, with Spark
>   carrying every edge between tasks.
> - A plan the converter cannot translate is `unsupported`, before anything runs. The host's
>   dialect (functions, hashing, overflow, state layouts) is the main gap.

The first three chapters of this part describe engines that Sirius plugs into: each keeps its
catalog, optimizer and scheduler, and builds Sirius plans from its own. This chapter covers the
other kind of integration. A **plan description** is an agreed-upon way to write a plan down, and
any system that produces one can reach Sirius through a converter. The book looks at two:

- **Velox plan nodes**, the physical plans that [Velox](https://github.com/facebookincubator/velox)
  and its derivatives run. [Axiom](https://github.com/facebookincubator/axiom) produces them for a
  whole distributed query, and Apache Gluten produces them for one Spark task at a time.
- **[Substrait](https://substrait.io)**, a cross-language specification for relational plans.

The Plan IR stays the only entry point into execution. A converter is a library on top of the
builder API, outside the core Sirius API. Its result is a `Plan` you can print, check and rewrite
like any other, so [rule 5](../part-2/01-everything-in-the-plan.md#the-ownership-rule) (builders
may help; plans may not hide) holds. The facts about other projects come from their public
repositories and specifications.

## What a description carries

A Sirius plan states more than either description does. The converter's caller fills the
difference through options.

| A Sirius plan needs | Velox plan nodes | Substrait |
|---|---|---|
| Relational operators and expressions | Yes | Yes |
| Types and nullability | Velox types | Substrait types, each with nullability |
| Function identity | A name in Velox's Presto or Spark function library | An extension URI, a name and a signature |
| Exchanges between workers | In Axiom's multi-fragment plan: every one. In a single fragment: none | Optional exchange relations; most producers emit none |
| Worker sets and their sizes | Task counts per fragment, some left to the runtime | None |
| Scan splits per worker | Supplied by the host next to the plan | File lists with byte ranges, or a table name to resolve |
| Partial aggregate state types | Velox's own intermediate layouts | An aggregation phase per measure, no layout |
| Runtime filters and adaptive decisions | None | None |

Whatever the converter derives or the caller supplies ends up in the plan, where `explain()` shows
it and the validator checks it.

## Velox plans

### The Velox plan adapter

The converter for Velox is `sirius::velox`, an adapter library. It is C++ only, because Velox is.
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

The adapter places no exchanges by default. Asked to put an exchange-free plan on `compute[N]`, it
returns `unsupported`. Where the adapter lives and what it covers is
[open question 32](../reference/open-questions.md#32-a-velox-plan-adapter).

### Mapping Velox's vocabulary

| Velox | Sirius Plan IR |
|---|---|
| A plan fragment | A stage. It is derived by cutting at exchanges, never declared |
| `PartitionedOutputNode` on keys, with its `ExchangeNode` | `Exchange::hash(keys, T)` with `HashScheme::v1`. Both ends are Sirius, so the hash need not match Velox's |
| `PartitionedOutputNode` broadcast, arbitrary, single | `Exchange::broadcast(T)`, `Exchange::round_robin(T)`, `Exchange::gather(head)` |
| `MergeExchangeNode` | `Exchange::gather(head, GatherOrder::merge(keys))` over sorted input |
| `LocalPartitionNode`, `LocalMergeNode` | Dropped. Movement between threads inside one worker is the engine's |
| `AggregationNode` single, partial, intermediate, final | `aggregate`, `aggregate_partial`, `aggregate_combine`, `aggregate_final`, with Sirius state types. A partial and final pair with only a local exchange between them becomes one `aggregate` |
| `GroupIdNode` directly below an `AggregationNode` | `aggregate_sets`: the grouping sets become the `sets` list and the group id column the grouping id, cast to Velox's `BIGINT` in a `Project` |
| `WindowNode` | `Window`, for the three forms the IR defines |
| `HashJoinNode` | `Join`. The mode is derived: `broadcast_build` over a broadcast build, `co_partitioned` over two matching hash exchanges, `single` on one worker |
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

Because the plan arrives with every exchange chosen, the adapter needs only the two sets:

| Axiom | Sirius Plan IR |
|---|---|
| `MultiFragmentPlan` | One closed, joined `Plan` |
| `kSource` and `kFixed` fragments | `compute[N]`, with N the GPU count the caller planned for (`maxRemotePartitions = N`) |
| `kSingle` | `head[1]` |
| The link from an `ExchangeNode` to the fragment that feeds it | The edge of one `Exchange` node |
| One `TaskStats` per fragment | The `ExecutionReport`, one stage per fragment |

Take Q7 from [A multi-stage query](../part-3/07-a-multi-stage-query.md#the-question), planned for
four GPUs. Axiom broadcasts the filtered customers and hash-partitions both sides of the big join
on the order key, which gives five fragments (sketched with Velox class names):

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
`Aggregate` is legal because its input is hashed on a group key (V6). Had the adapter
mistranslated one, `Plan::create` would reject the plan with `invalid_plan`.

The caller is an Axiom `Runner` subclass. It prepares the plan with `Bindings::local()`, reads the
one `Output` on `head#0`, and turns each batch into a Velox `RowVector` through the Arrow C data
interface. Fallback happens once, before anything runs: if the adapter or `prepare` returns
`unsupported`, or `runtime_unavailable`, Axiom's own runner executes the whole query.

### Runtime filters across fragments

Axiom's plan is static. The Plan IR can add what it cannot say, and the first addition is a
runtime filter that crosses fragments. Velox pushes a join's build keys into the probe-side scan
only inside one fragment. Axiom has a design for filters across fragments that matches the value
edges of [Runtime filters](../part-2/05-runtime-filters.md):

| Axiom's design | Plan IR |
|---|---|
| The optimizer places each filter: producing join, consuming scan, key mapping | `RuntimeFilterBuild` and `RuntimeFilterApply`, placed when the plan is built |
| Bloom size fixed from the build estimate, so per-task filters combine exactly | One fixed `BloomGeometry` for every share (V13) |
| Partitioned build: the runner collects every task's filter and ORs them | `scope=partial` or `partition(xK)`, merged by a `ValueExchange` |
| Broadcast build: the runner takes any one task's filter | `scope=global` on `replicated` input; no merge |
| The consumer scan blocks until the filter arrives, with no timeout | `wait=up_to(t)`, a bounded wait, or `wait=never` |

In Sirius the merge is a node the validator checks, and every wait is bounded, so a slow build
costs at most `t` of scan time. Whether an unbounded wait should exist is
[open question 6](../reference/open-questions.md#6-filter-waits). The caller adds the filters by
running `rewrite::insert_runtime_filters` on the translated plan; the result is the filtered Q7 of
Part III.

The second addition is an adaptive broadcast. Axiom chooses broadcast from an estimate. Where the
estimate is close to the limit, the caller can build the join with
`JoinOptions::with_adaptive_broadcast`, which leaves the choice to measured bytes
([Adaptive plans](../part-2/06-adaptive-plans.md)). The join output is then `arbitrary`, so an
aggregate above it must be two-phase (V6).

## Substrait

Substrait describes a plan as a tree of relations over typed expressions, in a form any language
can produce. A producer needs no Sirius code: it emits a Substrait plan, and the converter,
`sirius::substrait`, turns it into builder calls. Like the Velox adapter it sits on top of the
builder API and returns an ordinary `Plan`, or `unsupported` naming the relation it stopped at. Its
scope is part of [open question 32](../reference/open-questions.md#32-a-velox-plan-adapter).

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
| `FilterRel`, `ProjectRel` | `Filter`, `Project` |
| `AggregateRel` with one grouping | `Aggregate` |
| `AggregateRel` with several groupings | `Aggregate` with grouping sets |
| `JoinRel` | `Join`: equality terms of the condition become key pairs, the rest the residual |
| `SortRel`, `FetchRel` | `Sort`, `Limit`; a fetch directly above a sort becomes `TopN` |
| `SetRel` | `Union`, or the [set-operation compositions](../part-1/07-aggregates-joins-and-ordering.md#set-operations) |
| `ConsistentPartitionWindowRel` | `Window`, for the three forms the IR defines |
| `ExchangeRel`: scatter by fields, single bucket, round robin, broadcast | `Exchange::hash`, `gather`, `round_robin`, `broadcast`, onto sets the caller names |
| The emit list of any relation | A `Project` that selects and orders the columns |
| A type with its nullability | A `DataType` and a required or nullable `Field` |
| A function reference: extension URI, name and signature | A function looked up in the caller's `Registry` |
| The plan's root names | The `Output`'s column names |

### What Substrait leaves to the caller

- **Where the plan runs.** Substrait has no worker sets. A plan without exchange relations becomes
  a plan on `local[1]`. To run it on several GPUs the caller writes the exchanges, as
  sirius-duckdb does for DuckDB's plans
  ([Embedded in a host](01-duckdb-extension.md)), or produces Substrait that already carries them.
- **Agreement between workers.** An exchange relation says which fields to scatter by. It does not
  say which hash function, how many partitions or where nulls go. The converter writes
  `HashScheme::v1` and a partition count from its options, so those facts are in the Sirius plan
  even though the description lacks them.
- **Aggregate state.** A measure carries an aggregation phase (initial to intermediate,
  intermediate to result) but no state layout. The converter maps the phases to
  `aggregate_partial` and `aggregate_final` with Sirius state types, which works when both halves
  are in the same Substrait plan.
- **Function semantics.** An extension URI names a function; it does not define overflow or
  rounding. Each function the converter accepts maps to a built-in or registered function with the
  same behavior, and anything else is `unsupported`
  ([Dialect semantics](#dialect-semantics)).
- **Types.** Substrait types without a `DataType` (time, timestamp with time zone, interval, map,
  binary) are `unsupported` ([Additive later](../reference/extension-points.md#additive-later)).
- **Runtime filters and adaptive choices.** Substrait has neither. The caller adds them to the
  translated plan with the rewrites and builders of Part II.

## One task at a time: Bolt under Spark

[Bolt](https://github.com/bytedance/bolt) is a C++ execution library derived from Velox, with no
optimizer or scheduler. Its main use is as a native backend for Spark through
[Apache Gluten](https://github.com/apache/gluten). The Spark driver plans the query, cuts it into
stages at every shuffle, and sends each executor one *task* at a time: one partition of one stage.
Gluten builds the task's plan nodes and attaches the file splits Spark assigned to it.

That task plan is the adapter's first kind of input: one fragment with no remote exchanges. It
becomes one closed plan on `local[1]`. If the whole task translates, Sirius runs it on the GPU in
place of the Bolt task, and its output goes to the same places through Arrow. Spark keeps its
shuffle, adaptive re-planning, retries and speculation: none of them can tell which engine
produced a task's output.

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

A task Sirius cannot run goes to Bolt's CPU path, and nothing else changes. The translator stops
at the first node, type or function it cannot translate, and `Context::check` answers for the
engine build. Every task of a stage has the same plan apart from its splits, so the host can cache
the answer per stage.

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
| Integer `sum` | Returns `bigint`; overflow raises an error under ANSI mode and wraps without it | Returns `decimal(38,0)` | ANSI mode: a checked cast to `int64`. Otherwise a registered Spark-compatible `sum` |

Two patterns cover the table.

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
Spark-compatible state types that either engine can read. The first can ship first. Which dialect
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

None of these needs a new structural concept in the IR.

| Gap | In this design | Where it is tracked |
|---|---|---|
| Types: time, timestamp with time zone, interval, map, binary | No `DataType` yet; such a plan is `unsupported`. Spark's Bloom filter literal needs the binary type | [Additive later](../reference/extension-points.md#additive-later) |
| Window functions and frames outside the three `Window` forms; generators | No form | [Open question 29](../reference/open-questions.md#29-windows-over-a-global-order-prefix-value-mode) |
| Writes | No write node | [Additive later](../reference/extension-points.md#additive-later) |
| Dialects: function names and behavior, hashing, overflow, decimal rules, aggregate state layouts | Translator casts or registered functions; until they exist, the plan or task falls back | Open questions [16](../reference/open-questions.md#16-expression-semantics) and [31](../reference/open-questions.md#31-dialect-semantics) |
| State-typed inputs | The API cannot declare an `Input` with state types, which a reduce task that receives Sirius states needs | [Open question 31](../reference/open-questions.md#31-dialect-semantics) |
| Null-aware joins that also replicate one arbitrary row | `NullRouting::to_all` covers the nulls; a global build row count through `JoinOptions::with_build_rows` gives the same fact | [Open question 17](../reference/open-questions.md#17-join-coverage) |
| Bucketed tables read without a shuffle | The converter returns `unsupported` unless asked to insert a hash exchange | [Open question 13](../reference/open-questions.md#13-hints-and-certified-properties) |
| Splits streamed into running tasks | The caller drains every split source before building the plan | [Open question 30](../reference/open-questions.md#30-host-fed-split-channels) |
| Live inputs | Shuffle reads and host iterators bind as completed inputs | Open questions [20](../reference/open-questions.md) and [21](../reference/open-questions.md#21-input-buffer-limits) |
| Many executions per GPU | Executions on one engine may run one at a time | [Open question 15](../reference/open-questions.md#15-one-context-per-process) |
| Scope and home of the converters | `sirius::velox` and `sirius::substrait`, libraries outside the core Sirius API | [Open question 32](../reference/open-questions.md#32-a-velox-plan-adapter) |
