# Open questions

These are design decisions for the proposed API. “Answered” means the book adopts a contract.
Implementation status is a separate question. The numbers stay stable so chapters can refer to a
decision as the design changes.

| Theme | Questions |
|---|---|
| Plans and agreement | [1](#1-how-plans-travel), [9](#9-open-plan-composition), [12](#12-decision-determinism-and-replay), [18](#18-hash-scheme-stability), [25](#25-open-registries-and-versioning), [32](#32-a-velox-plan-adapter), [37](#37-remote-source-settings-and-credentials) |
| Topology and processes | [2](#2-worker-granularity), [3](#3-fixed-set-sizes), [15](#15-one-context-per-process), [23](#23-size-hints-for-unsized-sets), [34](#34-scans-over-gpu-resident-data), [35](#35-choosing-gpus-at-admission), [36](#36-a-distribution-helper-for-one-machine) |
| Lifecycle and flow | [4](#4-coordination-and-failure), [5](#5-backpressure), [20](#20-live-inputs), [21](#21-input-buffer-limits), [22](#22-dynamic-split-assignment), [28](#28-retry-api-and-who-decides), [30](#30-host-fed-split-channels) |
| Runtime values and adaptivity | [6](#6-filter-waits), [7](#7-bloom-geometry-and-kinds), [11](#11-choose-limits-and-richer-policies), [26](#26-skew-aware-exchanges), [33](#33-re-planning-beyond-the-plans-alternatives) |
| Semantics and coverage | [8](#8-nested-types-on-the-wire), [10](#10-shared-subplans-and-multicast), [13](#13-hints-and-certified-properties), [14](#14-host-registered-scan-kinds), [16](#16-expression-semantics), [17](#17-join-coverage), [19](#19-the-implicit-boundary), [29](#29-windows-over-a-global-order-prefix-value-mode), [31](#31-dialect-semantics) |
| Growing the IR | [24](#24-iteration-and-epochs), [27](#27-requirements-as-data-and-equivalence-properties) |

## 1. How plans travel

Should Sirius define how a closed plan reaches another process, or should hosts keep rebuilding
the plan with the same builder code in every process (SPMD) and rely on the fingerprint check?
Shipping plans would need versioning and fingerprints that stay stable across releases. With a
million files and a thousand workers, listing the files on every process and storing every split
inline in every plan does not scale either.

**Leaning:** keep plan shipping outside Sirius for now; SPMD plus fingerprints covers every host
in [Part IV](../part-4/04-one-ir-three-integrations.md). Large file lists travel by reference in a
[split manifest](../part-5/08-running-at-scale.md#split-manifests); splits are still fixed before
execution and still in the plan. The question returns when a process must run a plan it cannot
build itself, such as under a future Sirius coordinator. Versioning is part of question 25.
[Related designs](../appendix/related-designs.md#where-sirius-differs) contrasts this with systems
that build a plan once and ship it.

## 2. Worker granularity

A worker is bound to exactly one GPU, so moving data between GPUs is always an exchange, and an
all-to-all exchange over N workers has N^2 streams. Today's in-process engine balances work inside
one machine without exchanges: its scan manager spreads scan batches over the query's GPUs with one
shared cursor, and any ready GPU may claim a task that has no device preference. One worker per GPU
gives up that balancing. The alternative is an endpoint that is a process with a device set: the
engine owns the GPUs inside a process and the plan sees one worker per process. Do we also need
nested sets (`host[2]` of `gpu[4]`) so that aggregation can combine within a host before crossing
the network?

**Leaning:** keep one worker per GPU for now and decide with engine measurements.
`Shards::dynamic()` (question 22) gives back balancing for scans, and two-level shuffles inside a
host are part of an edge's physical shape, so neither needs a coarser worker. A hierarchy for
aggregation is a plan shape the front-end writes, with `Aggregate combine` and an extra exchange per
level; `aggregate_combine` is already in the proposed API, so no new node is needed.
[Running at scale](../part-5/08-running-at-scale.md#worker-granularity) compares the two
granularities.

The measurements reported in the merged distributed execution PR use one process per GPU.
The implementation in
[sirius-db/sirius#2337](https://github.com/sirius-db/sirius/pull/2337) runs one process per GPU with
exchanges between processes. On four GB200 GPUs at scale factor 3000, it reached 2.49 times the
speed of the single-GPU baseline over 20 queries. The single-process comparison in that PR
reached 1.56 times the baseline. This comparison changes both process layout and exchange
implementation, so it does not isolate the effect of worker granularity.

Across machines the first runs are mixed. At scale factor 30000 on four-GPU machines, going from 8
to 16 GPUs made TPC-H Q1 and Q6 about twice as fast, while TPC-H Q3, Q5 and Q9 got no faster
([#2465](https://github.com/sirius-db/sirius/issues/2465)). The report points at fixed per-peer
buffers and host-staged TCP between machines, and with less confidence at an all-to-all Bloom
filter merge.

The engine has also seen single queries run faster on one GPU than on several
([#1749](https://github.com/sirius-db/sirius/issues/1749)). With an explicit plan, that is the host
choosing a smaller set, `compute[1]` instead of `compute[4]`, visible in `explain()`. On one
machine, one process for all GPUs and one process per GPU bind the same plan;
[Running at scale](../part-5/08-running-at-scale.md#one-machine-one-process-or-one-per-gpu) gives
their trade-offs.

## 3. Fixed set sizes

A sized worker set has a fixed size in the plan. Is one plan per cluster shape enough, or is a
parametric size, resolved at `prepare`, worth having? An unsized set is not a parametric size: only
the hosting set of an instance plan may be unsized, and only when nothing in that plan depends on
its size (V24). **Leaning:** fixed sizes. Building a plan is cheap, and a fixed size keeps
fingerprints, partition-to-worker maps and validation exact; a parametric size would move part of
validation to `prepare`.

## 4. Coordination and failure

The proposal gives each execution an ID and each edge a first-contact deadline. Open plans have
no global list of participants. Several decisions still depend on who can see the whole query.
[Failure and recovery](../part-5/07-failure-cancellation-and-recovery.md#how-an-execution-ends)

- **Discovery.** One option is for the host to distribute an execution ID and opaque endpoint
  bytes. A compute node could fetch and cache its peers' endpoints. Another is to put an opaque
  peer ID in a binding and let a host callback resolve it to a transport agent. Each process
  would then depend on its resolver before first contact. [Resolver proposal](https://github.com/sirius-db/sirius/issues/1807)
- **Peer loss.** An initial deadline detects a peer that never joined. How should surviving
  workers detect a peer that joined and then disappeared? A fatal GPU error can leave that
  process unusable for further queries, while its peers may still have healthy engines.
- **Failure reporting.** A single failure has an origin. If independent workers fail
  concurrently, is there a query-wide choice of error? What can the host learn about effects
  already visible outside the query? The side-effect declarations restrict which stages can be
  retried. [Side effects](extension-points.md#side-effect-classes) · [Retry decisions](#28-retry-api-and-who-decides)
- **Multiple outputs.** When one output was drained and another closed early, which outcome
  does `wait()` report? Also, `collect()` returns a flat list of batches. Two named outputs
  can have the same schema. Should collection accept just one output, or group batches by
  output and worker?

## 5. Backpressure

Between processes, transports use receiver credits, and spilling is the buffer behind them: a batch
without credit waits in spillable memory, so a slow receiver causes spilling before it causes a
stall. Credits are part of an edge's physical shape, so they stay out of the plan and the bindings.
Buffers are still bounded, and then waits can block each other: a merging receiver
(`GatherOrder::merge` or `by_worker`) needs the next batch from one particular sender, and a decided
exchange cannot route a row until its required `Decision` arrives. Which waits must the validator
see, and what happens when every tier is full?

**Proposed check.** Within one execution, validate the waits represented in the plan
([Credits and deadlock freedom](../part-5/08-running-at-scale.md#credits-and-deadlock-freedom)).
Credits are granted per logical stream, so a stream without credit never holds up another stream on
the same connection. V1 requires an acyclic wait-for graph, so a plan whose waits could form a
cycle is rejected when it is built. Optional values never add a wait-for edge, because `wait=never`
never waits and `wait=up_to(t)` gives up after `t`. When every memory tier is full, the execution
fails with an `execution` error instead of hanging.

The plan cannot see waits between executions. A producer that holds the last admission slot can
block on a consumer waiting for that slot. A higher concurrency limit only changes how many jobs
can participate. The proposed API chains executions through completed inputs. Within an execution,
progress also depends on fair scheduling and enough space to drain or spill buffered data.

## 6. Filter waits

`WaitPolicy::up_to(t)` is a bounded wait on an optional filter: the consumer holds its input for at
most `t`, then continues with whatever the filter state is, and never fails. The deadline needs a
timer in the engine's task creator, which does not exist today, so bounded waits are engine work
(see [Runtime values in the engine](../part-5/03-runtime-values-in-the-engine.md)). What should the
default be, should there be a cap, and should a consumer wait for a filter built on another host?
**Leaning:** the default stays `never`, a context-wide cap bounds every `up_to`, and cross-host
waits are allowed because the wait is bounded and the filter is still optional.

Axiom's design for cross-fragment dynamic filters argues the other way: the consumer scan waits with
no timeout, because a filter can only fail to arrive if its build never finishes, and then the query
cannot finish either ([Runtime filters across fragments](../part-4/05-plans-from-other-systems.md#runtime-filters-across-fragments)). An
unbounded wait would make the filter required in all but name and add a wait to the
deadlock-freedom argument, so the leaning stays bounded.

## 7. Bloom geometry and kinds

`BloomGeometry::fixed` can blow up the false-positive rate on a large build. `sized_by` takes the
global row count, an integer `bits_per_key` and a byte cap, so every worker derives the same
geometry from the same merged count, but the global count costs a round trip. Two-level filters,
in-list overflow and per-split zone maps are also open. **Leaning:** `fixed` by default, `sized_by`
when the plan already observes global row counts for a `Decide`; in-list overflow keeps the
deterministic `or_else` rule, and the rest waits for measurements.

## 8. Nested types on the wire

List and struct state columns must cross exchanges in a fixed physical layout, with known limits.
**Leaning:** flatten struct states such as `avg_state` and `var_state` into their components when
they cross an exchange; keep rejecting list states such as `distinct_state` across exchanges (V21)
until their layout is fixed, and plan distinct counts as two-level groupings meanwhile.

## 9. Open-plan composition

A tool with every instance plan can reconstruct a joined plan. It needs a consistent set of
worker indices, matching signatures at both ends of every edge, and the scan ranges assigned
to each instance. The proposed signature includes whether the receiver can consume rows as they
arrive or must wait for a completed sender attempt. [Spooling](../part-5/07-failure-cancellation-and-recovery.md#durability-pipelined-or-spooled)

An independently prepared fragment has less evidence. A scan receiving a runtime filter can
check its key type and sender set. It cannot see whether a downstream outer join must preserve
rows the filter would remove. Which checks require the host to assemble the plan, and what
evidence can the host give Sirius when it prepares fragments separately?
[Validation of open plans](../part-2/07-validation.md#three-gates)

## 10. Shared subplans and multicast

A node shared by several consumers is evaluated once per worker, and sharing an `Input`, a volatile
node or an effectful node is rejected with `invalid_plan` until a multicast is designed
([Shared subplans](extension-points.md#shared-subplans)). Should the plan offer a multicast
exchange or an explicit evaluate-once node that lifts those restrictions, and how are consumers
that progress at different speeds buffered? **Leaning:** keep the rejection until a host needs
more. Then add an explicit node with its own buffering rule: fanning out a live input or a volatile
result changes what each consumer ends up with, so it belongs in the plan. Until then, a host
declares one `Input` per reader and feeds each the same batches. A `RecordBatch` is cheap to copy,
so this shares the data.

Upstream’s `exec::streaming_fragment` has the same rule: a declared input may be read by at most one
plan leaf. An upstream draft ([#2157](https://github.com/sirius-db/sirius/pull/2157)) would let a
shared subtree read one input twice by giving each extra reader its own copy of every pushed batch.

DuckDB's materialized CTEs are shared subplans. So is a decorrelated subquery: the outer relation
feeds both the subquery and the join that brings its result back. The IR must express both even on
one machine. It uses shared nodes, with no node of their own, so the restriction above also limits
which subqueries a plan can share. Whether and how to distribute a shared subplan is the
front-end's policy.

## 11. `Choose` limits and richer policies

**Partly answered.** [Choosing between
subtrees](../part-2/06-adaptive-plans.md#choosing-between-subtrees) proposes `Choose`: a Decision
picks between whole subtrees, and the stage graph stays fixed because every arm is prepared (V26).
Three points remain open:

- **Policies over several statistics.** `threshold` and `partition_count` read one merged statistic.
  Choosing which side of a join to broadcast, or between two listed join orders, needs a policy that
  compares two, such as "broadcast the smaller side if it is at most 64 MiB, otherwise hash both".
  Today's engine already compares the probe-to-build ratio
  ([Adaptivity in the engine](../part-5/04-adaptivity-in-the-engine.md#what-does-not-map-one-to-one)).
- **The cost of the arms that lose.** Every arm is prepared and admitted, so a plan with many arms
  reserves for all of them. Should admission reserve only for the most expensive arm on each set?
- **Nesting.** A `Choose` inside an arm of another multiplies the arms. Allowed, or capped?

**Leaning:** add a registered `compare` policy over global statistics of the same metric, with
integer ratios only, as the next `Decide` policy. Admit for the most expensive arm on each set, not
for every arm. Allow nesting, since each level is checked on its own.

## 12. Decision determinism and replay

Must re-running the joined plan on the same data draw the same key sample, and so the same
`Boundaries`? Within one run every worker already sees the same pooled sample.

The rest is settled. Every worker computes the same label from the same merged value
([Adaptive plans](../part-2/06-adaptive-plans.md#why-every-worker-picks-the-same-label): logical
bytes, integer or decimal policy inputs, sender-ordered samples, digests, the
[leader pattern](../part-2/06-adaptive-plans.md#deciding-on-a-leader)). To repeat the recorded decisions, use the
[resolved plan](../part-2/06-adaptive-plans.md#after-the-run-decisions-and-the-resolved-plan), from
`rewrite::resolve` over `report.decisions()` or from `report.resolved_plan()`, which also
substitutes assignments and dynamic split claims. Reproducing the rows also requires the same
input data. Results can still differ for volatile functions, unordered limits and ties.

**Leaning:** yes where it is cheap. Each worker keeps the rows with the smallest key hashes under a
fixed seed, so its sample depends only on which rows it holds, not on batch boundaries or arrival
order. Where the rows a worker holds can vary between runs (an `arbitrary` input, or
`Shards::dynamic()`), the resolved plan remains the way to replay.

## 13. Hints and certified properties

Hosts often know more than the plan says: row estimates, unique keys, columns that are never null,
and tables already laid out by key, such as bucketed files or Iceberg bucket partitions. A source
that is already partitioned could feed a co-partitioned join without an exchange. **Leaning:** row
estimates are hints that never affect validity and only feed rewrites and policies. Uniqueness and
not-null are host-certified facts that rewrites may rely on; the host is responsible for their
truth. Partitioning is declared by the source: a scan of such a source states `hashed` on its keys
under a named hash scheme, the declaration is part of the scan and its fingerprint, and the
validator checks it like any other distribution. It only lines up with an exchange that uses the
same scheme and partition count (question 18), on a sized set.

## 14. Host-registered scan kinds

`TableSource::registered` passes an opaque `ScanHandle` to a host-registered scan kind. How long
must the host keep the snapshot behind it alive, and do we need visibility callbacks or leases?
**Leaning:** the handle stays valid until `wait()` returns, and visibility and leases stay the
host's business behind the handle. Who checks a registered source against its declared schema, the
engine or the kind's reader, is part of the same question. So are how a `ScanHandle` is made, how a
scan kind is registered (question 25), and which units a registered scan lists as its shards
([Embedded in a host](../part-4/01-duckdb-extension.md#a-scan-the-host-owns)).

## 15. One Context per process

**Partly answered.** The proposal has one engine per process, and `Context::create` enforces the
limit ([How long a context lives](../part-1/03-context-and-session.md#how-long-a-context-lives)).
The upstream design is [sirius-db/sirius#2448](https://github.com/sirius-db/sirius/issues/2448),
with enforcement proposed in [#2457](https://github.com/sirius-db/sirius/issues/2457) and the public
`Context` contract in [#2460](https://github.com/sirius-db/sirius/issues/2460).

[sirius-db/sirius#2461](https://github.com/sirius-db/sirius/issues/2461) stages the execution API
and leaves concurrent executions on one engine and independent contexts per process for after its
first stages. Until then a `Context` may be shared between threads, but its executions may still run
one at a time (question 5). #2461 also names a `Session` that may arrive later as a planning
environment, with a catalog and settings, for catalog-backed or SQL preparation. Nothing in the
proposal depends on it: `check` and `prepare` live on `Context`. The published C++ API leaves the one-context limit to the caller. Its handle has unique
ownership. [Current context contract](https://github.com/sirius-db/sirius/blob/ef7eb2951a370f4293784549bc05fc46c7cc64f6/include/sirius/context/context.hpp)

Six points remain open.

- **A matching second create.** Should a second `Context::create` whose configuration matches the
  live engine share it instead of failing?
- **Sharing with a different configuration.** In upstream's design
  ([#2457](https://github.com/sirius-db/sirius/issues/2457)), integrations share through an internal
  get-or-create, which sirius-duckdb and the older FFI context use. What it does when a later
  database asks for a configuration that differs from the live engine's is not specified, and a
  public handle for explicit sharing is deferred.
- **Engine state and per-query state.** Which state belongs to the engine and which to one prepared
  plan or execution. Upstream's default
  ([#2449](https://github.com/sirius-db/sirius/issues/2449)) is that anything a GPU worker can reach
  is engine state. Admission and `max_concurrent_queries` sit on the line. **Leaning:** admission
  is engine-level, because
  [gang admission](../part-5/08-running-at-scale.md#admission-across-processes) needs one admitter
  per process that sees every query. Sorting requests by the same priority key does not make
  reservations atomic when different participants see different sets of requests. The proposal
  still needs a way to commit a common admission or withdraw partial reservations.
- **One admission unit for several instance plans.** A compute node's instance plans of one query
  under one `ExecutionId` are admitted as one unit: they all wait or all run
  ([StarRocks](../part-4/02-starrocks.md#what-prepare-binds)). `PreparedPlan::start()` starts one
  prepared plan, so how a compute node starts several as one unit, for example with one call that
  starts several prepared plans, is not designed yet.
- **Independent contexts.** Several contexts in one process would each admit queries on their own,
  while gang admission assumes one admitter per process. They would need admission across them,
  such as one admitter that every context in the process shares.
- **The configuration split.** Which configuration settings are engine-level, and which are prepare
  or query options, or settings of a later `Session`. Upstream audits today's settings in
  [#2484](https://github.com/sirius-db/sirius/issues/2484). Where the object-store settings of a
  remote scan live is [question 37](#37-remote-source-settings-and-credentials).

Bolt inside Spark makes this concrete ([Many tasks, one GPU](../part-4/05-plans-from-other-systems.md#many-tasks-one-gpu)). An executor
runs many short tasks at once. Once one engine runs executions concurrently, each task would be its
own execution on the executor's one `Context`, with its quotas as prepare options. A task that is
not admitted waits in `start` while it holds a Spark task slot. Should a host be able to give up
after a deadline and run the task on the CPU instead, and how should admission feed a host's own
concurrency target, such as Bolt's adaptive task parallelism? Both are open.

## 16. Expression semantics

**Partly answered.** Three things are settled. Sirius never converts a type implicitly, in an
expression or between the two keys of a join pair
([No implicit conversion](../part-1/04-schemas-and-expressions.md#no-implicit-conversion)). A
comparison with a null is null, and a filter keeps only true rows. Floating-point values compare
in one total order ([Nulls and NaN](../part-1/04-schemas-and-expressions.md#nulls-and-nan)).

Still open are the function catalog, the conversions `cast` accepts, which types each comparison
accepts (ordering a boolean or a `list`, for example), the precision of a decimal product, the
result of `div`, what arithmetic does on overflow, and how `and`, `or`, arithmetic and function
calls treat nulls. Whether a schema may have no fields, and whether plans and expressions have a
depth limit, are undecided too. The proposed builder folds `now()` to a literal. Reusing the plan then
reuses the timestamp from construction. A host promising the time of the current query must
supply a fresh value for each query. Should the plan have an execution-time parameter for this?
Floating-point addition is also sensitive to reduction order. Should a floating-point aggregate
guarantee the same bits across partition counts and retries? Function names resolve as follows: functions resolve
when the plan is built, in an immutable, context-free `Registry` that plan construction takes
([Closed and open lists](extension-points.md#closed-and-open-lists), question 25).
**Leaning:** nothing implicit in the open parts either. A result type follows from the operand
types alone, never from the values, and nullability follows from fields and operands, as it
already does for comparisons. Every `Registry` entry carries a
[determinism class](extension-points.md#determinism-classes).

## 17. Join coverage

**Partly answered.** Null-safe key pairs are designed: `JoinKey::null_safe` in `JoinOn::keys`
([Null-safe keys](../part-1/07-aggregates-joins-and-ordering.md#null-safe-keys)). With them,
distinct `INTERSECT` and `EXCEPT` are compositions of existing nodes
([Set operations](../part-1/07-aggregates-joins-and-ordering.md#set-operations)), and a
decorrelated subquery is a shared subplan read by ordinary joins, with no node of its own
([Shared subplans](extension-points.md#shared-subplans)). `INTERSECT ALL` and `EXCEPT ALL` need
no join: they count rows with an aggregate and end in a `Repeat`.

Three cases are open: outer joins with a preserved broadcast build, null-aware anti joins, and
distributed non-equi joins. **Leaning:** preserved broadcast builds stay rejected (V9) and use a
partitioned plan; a matched-rows bitmap carried as a new value kind is the likely extension.
Null-aware anti joins follow the mark-join pattern, and non-equi joins use a broadcast build with a
nested-loop join.

With a co-partitioned build, the mark-join pattern is `NullRouting::to_all` on the build exchange
plus a global `rows` statistic of the build
([Mark joins need the whole build](../part-2/03-exchanges.md#mark-joins-need-the-whole-build)).
A null-aware anti join needs the same row count, and V28 would then admit its build exchange as it
admits a mark join's. The pattern holds only for one plain key pair (V10), so mark joins on several
pairs, or on a mix of plain and null-safe pairs, use a broadcast or single build for now. Whether a
co-partitioned form exists for them, and what it would need, is open.

One smaller point remains around null-safe pairs. A runtime filter with a key from one lets every
null key pass (V18); a filter that also records whether the build holds a null key could drop them
when it does not.

A mark join with an extra residual predicate needs its own proof. With a null probe key, a
nonempty build and a residual that is false for every pair, the combined condition is false
for every row. A global count alone cannot decide the mark for that probe. A nullable residual
can also introduce unknown when the key comparisons themselves are null-safe. Which residuals
can a partitioned mark join evaluate with the information it receives?

## 18. Hash scheme stability

`HashScheme::v1` fixes the hash function, seed and key canonicalization, so every worker of a hash
exchange must be a Sirius worker. Canonicalization has to be exact for every key type
([Hash canonicalization in v1](extension-points.md#hash-canonicalization-in-v1)). Must `v1` stay
identical across releases and languages? Interoperability also needs the hash algorithm and seed,
byte order, null encoding, framing of variable-length keys, and shared test vectors. The
canonicalization table alone does not define a wire-compatible hash. How do other schemes, such as Spark's Murmur3 hash
with seed 42, Iceberg bucket hashing, or Velox's or Presto's hash partitioning, fit in?
**Leaning:** `v1` is frozen forever for the types it defines, and any change becomes a new scheme
with a new name, which also changes the partitioning signature. Other schemes are registered under
their own names (question 25). Hosts that run Sirius on every worker of an exchange, such as
StarRocks and Doris, need no compatible scheme; `v1` suffices. Range exchanges carry null placement
and per-key ordering in their signature, for the same reason.

A registered scheme only makes the routing compatible. A foreign worker on a Sirius edge, such as a
CPU Velox fragment between Sirius fragments under Axiom or a Velox worker under Presto, would also
need that system's exchange protocol; that is why the per-fragment levels in
[Other levels](../part-4/05-plans-from-other-systems.md#other-levels) are blocked today. Spark's partition id under Bolt is not
a scheme: Spark carries the shuffle, so the id is a function in a `Project`. A Spark-compatible
scheme would matter only if Sirius carried a Spark shuffle itself.

## 19. The implicit boundary

[Everything in the plan](../part-2/01-everything-in-the-plan.md#what-stays-implicit) splits what
stays implicit three ways: the physical shape of each edge, what happens inside one worker, and the
choices the plan leaves open where it says `arbitrary`. Is that split exactly right, and is every
item reported?
**Leaning:** that split is the boundary, and every implicit choice that affects performance appears
in the `ExecutionReport`, so nothing is both implicit and invisible.

## 20. Live inputs

This question and the next concern only the later, live-streaming stage of the execution API
([Feeding live inputs](../part-1/06-prepare-and-execute.md#feeding-live-inputs)). The first stage
binds completed inputs (`InputSource::completed`): every batch is supplied before the execution
starts, so there is no producer count, no buffer limit and no rule to start before writing.

A live `Input` ends when every producer of its channel has finished. Open points are end of stream
with many producers, a limit over an unbounded input, and inputs on sets larger than one worker.
**Leaning:** the `ProducerCount` on `InputChannel` defines completion, a satisfied `Limit` closes
the input so writers learn it on their next write, and an input on a larger set gets one channel
per worker through `BindingsBuilder::input`.

## 21. Input buffer limits

`InputChannel::create` takes a `BufferLimits` that bounds how much a channel buffers before `write`
blocks and `try_write` returns `WouldBlock`. How a `BufferLimits` is constructed is open. The
options are a byte bound (`BufferLimits::bytes(n)`), a batch bound (`BufferLimits::batches(n)`), or
`BufferLimits::defaults()` with `with_*` methods that set either or both. **Leaning:** `defaults()`
plus `with_*` methods, like every other option object in the API, with the byte bound as the
primary limit because batch sizes are the engine's business.

## 22. Dynamic split assignment

Should a worker that finishes its splits early be able to take splits a slower worker has not
started?

**Answered:** yes, through an explicit shard kind, `Shards::dynamic()`
([Running at scale](../part-5/08-running-at-scale.md#dynamic-splits)). Scans with any other shard
kind keep the assignment fixed when the plan is built; there is no implicit stealing between them.
Snowflake's file stealing, the closest relative, is described in
[Related designs](../appendix/related-designs.md#spark-trino-and-snowflake).

## 23. Size hints for unsized sets

The hosting set of an instance plan may be unsized, written `frag1[?]`, because nothing in that plan
depends on its size. Reports and explain output could still use the number of instances, for example
to print `frag1#2 of 4`. Should an unsized set carry an optional size hint? **Leaning:** no hint in
the plan. A hint that the validator ignores invites code that relies on it anyway. The host already
knows the instance count and can attach it to its own reports, and `rewrite::join_stages` recovers
the size from the instances it is given.

## 24. Iteration and epochs

Recursive CTEs, iterative algorithms and streaming all need the same edges to carry several rounds
of data, while V1 requires the graph of data and value edges to be acyclic. An `epoch` field, always
0 today, is already reserved for those rounds
([Reserved now: epochs](extension-points.md#reserved-now-epochs)). How should a plan express a
loop, and how do all workers agree that it has ended?

**Leaning:** a future `Iterate` node, whose body stays acyclic within each epoch. Whether another
round runs is a required global value, merged and decided like any other, so every worker leaves
the loop in the same epoch. Streaming, with an unbounded number of epochs, waits until a host needs
it.

## 25. Open registries and versioning

Functions, aggregate state types, `Decide` and `AssignPartitions` policies, hash schemes, scan
kinds, extension data types, value kinds and registered nodes are open for registration. Two
workers that use a registered item must agree that it is the same item, in the same version, or
they compute different answers without noticing. How is a registered item identified, and what may
differ between two processes?

**Partly answered** by [sirius-db/sirius#1750](https://github.com/sirius-db/sirius/issues/1750).
Registered items live in a `Registry`, an optional, immutable and context-free input to plan
construction ([Closed and open lists](extension-points.md#closed-and-open-lists)). Still open is
how every process's engine learns of the same registry, and how a mismatch is detected beyond what
edge signatures already cover by identity.

**Leaning:** every registered item has an identity of namespace, name, signature and version, which
the fingerprint and every edge signature carry ([Versioning](extension-points.md#versioning)).
Versions match exactly; there are no compatibility ranges until a host needs them.

[Plans from other systems](../part-4/05-plans-from-other-systems.md) leans on these lists the
most: Presto- and Spark-compatible functions and state types (question 31), foreign hash schemes
(question 18), and connector translators for the Velox plan adapter (question 32).

## 26. Skew-aware exchanges

A hash exchange sends every row of a key to one worker, so a skewed key overloads that worker.
Over-partitioning with an
[`Assignment`](../part-5/08-running-at-scale.md#skew-more-partitions-than-workers) helps when many
keys are moderately skewed, but it cannot split one hot key, because a partition still lands on one
worker. Should the plan offer an exchange that handles heavy hitters explicitly?

**Leaning:** yes, as an explicit shape in the plan. A key sample identifies the heavy keys as a
global required value; for those keys, the probe side's rows are spread `round_robin` and the build
side's rows are broadcast, while all other keys hash as usual. The output is then `arbitrary`, not
`hashed`, and the shape is legal only where a broadcast build would be (V9).

<a id="27-requirement-driven-distribute-and-equivalence-properties"></a>

## 27. Requirements as data and equivalence properties

An aggregate needs all rows of a group on one worker. A validator can check this from the
grouping columns and the input distribution. The proposal would expose each node's requirements
as data so the validator and a front-end planner can use the same rules. [Node requirements](extension-points.md#requirements-are-data)

Two cases need more than the names of the distribution keys.

- **Equivalent columns.** After an inner join on `a == b`, hashing on `a` also implies hashing on
  `b`. Outer joins can pad one side with nulls, so this inference depends on join type. A filter
  can also fix a column to a constant. The proposal favors tracking equivalent join keys first.
- **Replicas and row counts.** Broadcast one row to four workers and give each copy a different
  `random()` value. The four rows are no longer identical, but gathering them still yields four
  rows for one logical input row. Calling this distribution `arbitrary` loses that fact. The
  same problem arises when `Choose` can return either replicated or partitioned rows, or when
  replicas select different rows tied at a `TopN` cutoff. Before such output feeds a gather,
  an aggregate or a shared result, the validator needs proof of how many copies it can consume.
  Should the IR track row multiplicity separately from placement, provide an operation that
  selects one replica, or reject plans whose multiplicity cannot be proved?

The replica rule is unresolved. Plans in the examples use replication where each worker
intentionally needs a copy, such as the build side of a broadcast join.

## 28. Retry: API and who decides

A [retryable](../part-5/07-failure-cancellation-and-recovery.md#retrying-a-stage-instance) stage
instance may re-run under a new attempt, possibly re-bound to a different endpoint under a new
binding epoch. Who decides to retry, how do they learn that an instance failed and that retry is
allowed, and what is the call?

**Leaning:** the host or launcher decides, because only it knows its cluster and budget
([Who decides](../part-5/07-failure-cancellation-and-recovery.md#who-decides)). `Execution` reports
per-instance failures as `InstanceFailed` events from `next_event`, each with the error, the
attempt and whether retry is allowed, and offers `Execution::retry_stage`. Sirius never retries on
its own, and the `Output` stage is never retried after its first batch reached the host. These
names are a sketch ([API at a glance](api-at-a-glance.md#failure-events-and-retry-sketch)); the
exact API, how a host opts in to hear failures before they become fatal, and how long a failed
instance waits for the host's answer are open. A committed spool also needs a failure domain:
can the next attempt read it after its worker or host is lost? Rerunning only the failed instance
requires its inputs to remain available and the consumers to agree on the attempt they read. A binding names a worker, while retry
names a stage instance. If two stages share the worker, moving only one instance needs a rule
for finding the other's committed spool and rejecting messages from the superseded attempt.

## 29. Windows over a global order (prefix value mode)

A [`Window`](node-catalog.md#window) without keys treats all rows as one window group, so it
requires `single`: one worker holds every row, orders them and computes every value. Today's engine
does the same on one GPU. The whole form has another plan already: an ungrouped two-phase
aggregate whose one row is broadcast and joined to every row. The ranking and running forms have
none. A running total over a whole table, or a `row_number` over a global order, should be able to
spread over a worker set. How should the plan express that?

**Leaning:** a `Window` without keys also accepts a `ranged` input whose range keys are its order
keys. Worker i then holds interval i of the order, and a range exchange never splits equal keys, so
peers stay on one worker. Each worker can order its own rows, but it also needs a summary of every
row before its interval: the row count, the number of distinct order key values and the running
state of each measure. That summary travels on a value edge with a new `ValueExchange` mode,
`prefix`, next to today's `merge`: receiver k ends up with the merge of the shares of senders 0 to
k-1. The summary is a required value, so each worker holds its output until it arrives.

Three coverage gaps sit next to this question, none with a leaning yet. The window functions and
frames the IR does not define, such as `lag`, `lead`, `ntile`, bounded frames and frames that share
a value among peers, cannot be built, so a host keeps its own plan for them. A window group larger
than one GPU's memory fails the execution; evaluating one out of core is the engine's work
([Lowering a `Window`](../part-5/01-from-plan-to-pipelines.md#lowering-a-window)). Generators,
such as Spark's `explode` and Velox's `UnnestNode`, have no node. The
[Velox and Spark plans](../part-4/05-plans-from-other-systems.md#gaps) meet the first
and the third.

Grouping sets are not part of this question: they are a parameter of `Aggregate`
([Grouping sets](../part-1/07-aggregates-joins-and-ordering.md#grouping-sets)), which is what
Spark's `Expand` and Velox's `GroupIdNode` become when they sit directly below their aggregation.

## 30. Host-fed split channels

A Sirius scan's splits are fixed when the plan is built: as per-worker lists, or as one fixed list
that the workers of a sized set claim at run time under `shards=dynamic` (question 22). Some hosts
work differently. A Presto coordinator, and the distributed runtime that Axiom's design documents
describe, feed splits into a running source task as the connector discovers them, and may add tasks
as they go ([Other levels](../part-4/05-plans-from-other-systems.md#other-levels)). Spark does not need this, because a Spark task knows all
its splits when it starts. Should a plan be able to receive splits while it runs?

The options are to enumerate every split before building, as an Axiom runner does, with a
[split manifest](../part-5/08-running-at-scale.md#split-manifests) when the list is long; or a split
channel, bound at prepare like an `Input`, that the host writes splits into and then finishes. A
channel weakens [rule 4](../part-2/01-everything-in-the-plan.md#the-ownership-rule): the plan would
no longer hold the splits a worker reads, only the promise that the host supplies them, and the
fingerprint would not cover them.

**Leaning:** enumerate before building until a host needs streaming. If one does, a split channel
becomes a shard kind printed in the plan, like `shards=dynamic`: its output is `arbitrary`, it
cannot be used where a later node relies on which worker read which split, and the report and the
resolved plan record every split each worker read.

## 31. Dialect semantics

A host that falls back per query or per task needs Sirius to compute exactly what its own engine
would. Presto and Spark differ from the built-in semantics in four areas
([Dialect semantics](../part-4/05-plans-from-other-systems.md#dialect-semantics)): hashing, for
partition ids and Bloom filter probes; decimal precision and scale rules; integer `sum`, where
Presto, Spark and StarRocks return `bigint` and Sirius returns `decimal(38,0)`; and aggregate state
layouts, where a partial aggregate whose final half may run on the host's CPU engine must produce
that engine's layout.

Two tools exist. A translator cast fixes a result that has the right value in a different type. A
registered function or state type under the host's namespace, such as `spark.hash`, covers a value
that differs. Which items belong where? How does a measure name a registered state type? And how
does a host declare an `Input` whose fields have state types, such as the reduce side of a partial
aggregate split by a Spark shuffle, or a StarRocks fragment that receives partial states from
another? The API can read a state type through `DataType::state()` but cannot declare one today.

**Leaning.** Use a cast when only the result type differs. A cast after a wider sum can miss
an intermediate overflow, so it cannot implement Spark's checked sum in general. Use a
host-compatible function for hashing, Bloom probes or arithmetic when their behavior differs.
If partial aggregate states cross into a foreign engine, both sides need the same state layout
and merge semantics. [Dialect example](../part-4/05-plans-from-other-systems.md#dialect-semantics)

## 32. A Velox plan adapter

[Plans from other systems](../part-4/05-plans-from-other-systems.md#the-velox-plan-adapter) proposes `sirius::velox`, a C++
library on top of the builder API. It turns a tree of Velox plan nodes, or Axiom's multi-fragment
plan, into builder calls and returns an ordinary `Plan`, or `unsupported` naming the first Velox
plan node it cannot translate. Velox plans can also come from Velox's own serialized form, read back
through Velox's registry. Four things are open:

- **Scope.** Which Velox nodes, functions and connectors it covers. Hive tables over Parquet come
  first, through a registered connector translator.
- **Home.** The Sirius repository, Axiom or Velox.
- **Coupling.** How tightly it follows Velox versions and Velox's serialized form.
- **Round trip.** Whether a Sirius plan should also convert back into Velox plan nodes, for example
  for fallback.

**Leaning:** a separate library in the Sirius repository, built against one pinned Velox version,
that only reads Velox plans. Fallback keeps the host's original plan, so no round trip is needed.
Bolt is a Velox fork with its own namespace, so it would need its own adapter or a shared core.
The Substrait converter, `sirius::substrait`, raises the same four questions; it has no engine to
pin to, only a version of the specification.

## 33. Re-planning beyond the plan's alternatives

A decision can choose among listed join orders. It cannot introduce a new one when row counts
turn out very differently from the estimates. Should a host be able to materialize an
intermediate result, inspect its size and build a new plan for the remaining work?

One option would keep partitioned outputs on workers across executions. The second plan would
read them with a declared distribution. The host could choose a new join order, at the cost of
a pipeline break, a second preparation, and rules for retention, cleanup and worker loss.
Diagnostics would have to connect the two plans.

The book favors keeping runtime choices within one plan for now. A host can already *express*
a boundary as an `Output` followed by a completed `Input` in another plan, though it then owns
the intermediate data. Retained worker-side results would also be useful for caching.
[Spool durability](../part-5/07-failure-cancellation-and-recovery.md#durability-pipelined-or-spooled) ·
[Certified source properties](#13-hints-and-certified-properties)

## 34. Scans over GPU-resident data

A GPU-tier `pin_table` places chunks on devices before a plan binds its workers. A split may
therefore be assigned to a worker on a different GPU. The result can be correct while paying for
the copy that pinning was meant to avoid.

| Option | What the host or engine must do | Cost |
|---|---|---|
| Build from residency | The host obtains splits per endpoint, uses `explicit_splits`, and binds the corresponding workers there. Count nonlocal reads in the report. | Requires placement at build time. Eviction or later placement changes can invalidate the locality assumption. |
| Prefer local dynamic claims | Each worker claims its resident splits first, then unclaimed work. | Flexible under load, but a worker may still copy another's chunk. Only works if downstream operators accept any assignment. |
| Resolve a resident shard kind at prepare | Bind each split using actual cache residency and worker placement. | The plan fingerprint would omit a decision that affects which worker reads which split. |

A table pinned by key raises a separate correctness question. Skipping an exchange requires proof
that the source has the same key canonicalization, hash scheme, partition count and assignment
as its consumer. Residency alone proves none of these. Which component reports residency if
the DuckDB extension supplies the scan connector also remains open.
[Partition-aware pinning](https://github.com/sirius-db/sirius/issues/1018) ·
[Connector boundary](https://github.com/sirius-db/sirius/issues/2545)

## 35. Choosing GPUs at admission

The in-process engine can cap a query's GPUs with `gpus_per_query` and further narrow them
using `admission_bytes_per_gpu`. The selected devices are a prefix of the sorted list. Two small
queries can therefore compete on the first GPU while others are idle.
[GPU selection](https://github.com/sirius-db/sirius/blob/ef7eb2951a370f4293784549bc05fc46c7cc64f6/src/sirius_engine.cpp) ·
[GPU admission issue](https://github.com/sirius-db/sirius/issues/1749)

With explicit workers the plan fixes the count, for example `compute[2]`. The open choice is when
local bindings pick the two devices.

- At **prepare**, the host can name the devices and the engine checks them once. Prefetch can
  start early, but the host has to account for load.
- At **admission**, the engine could resolve `Bindings::local()` against current capacity.
  Explicit endpoint bindings would stay fixed. Checks and prefetch that depend on a device
  would have to wait or cover every candidate. This option works within one process.

Pinned splits make the choice observable in transfer costs. [GPU-resident scans](#34-scans-over-gpu-resident-data)

## 36. A distribution helper for one machine

For single-process multi-GPU queries the engine can insert partitions, merges and broadcasts.
The proposed IR makes each exchange visible. Who should translate a single-worker plan for
several local GPUs? [Current wrapping passes](../part-5/05-plan-to-plan-rewrites.md#todays-multi-gpu-wrapping-moves-to-the-front-end)

| Option | Consequence |
|---|---|
| Let each front end place exchanges | The host owns partition counts and broadcast thresholds, but each single-node host must implement the policy. |
| Offer a one-machine rewrite | A common helper would read node requirements and emit a printable, validated plan. Its API must specify which decisions the caller supplies and which it can defer to a `Decide`. |
| Keep implicit wrapping on one machine | Preserves the existing execution path. GPU-to-GPU movement would remain invisible to the proposed validator, leaving local and distributed execution with different contracts. |

The choice is open. The helper under discussion is scoped to a single machine. [Node requirements](#27-requirements-as-data-and-equivalence-properties)

## 37. Remote-source settings and credentials

A remote scan needs a location, endpoint and credentials. The existing FFI context holds
immutable configurations by URI prefix. A host can replace one scope independently. Binding
captures the current set and selects the longest matching prefix. The C API also provides a
builder for one immutable object-store configuration. Where should a caller supply these
settings in the proposed execution API?
[Current FFI contract](https://github.com/sirius-db/sirius/blob/ef7eb2951a370f4293784549bc05fc46c7cc64f6/include/sirius/ffi.hpp) · [C builder](https://github.com/sirius-db/sirius/pull/2609) ·
[Execution API issue](https://github.com/sirius-db/sirius/issues/2461)

**Leaning:** the plan carries only the location, and the caller passes the settings to `prepare`.

- **The settings registry is an immutable value.** It is a list of object-store configurations,
  each with a URI prefix, and is unrelated to the `Registry` of plan construction. Adding or
  dropping an entry returns a new registry and leaves the old one unchanged.
- **`prepare` takes the registry.** For each scan it uses the entry with the longest matching
  prefix, and the prepared plan keeps that choice. A location with no matching entry fails
  `prepare`. A host rotates credentials by passing a new registry to the next `prepare`; a plan
  that is already prepared never sees the change.
- **Credential scope.** A process has one engine and several hosts can share it,
  so a registry stored there would share their credentials. Any connection reuse must respect the effective credentials and TLS settings.
- **Nothing secret is a plan fact.** Settings and credentials never enter the plan or its
  fingerprint, so two processes with different credentials for one location still agree on the
  plan. A URI is a location, not a credential: put no secret in it.
- **Printouts redact.** `explain()` and error messages print a URI without its user-info, query
  and fragment, in case a secret slipped in.

Still open: how the registry reaches `prepare` (an argument, `PrepareOptions` or `Bindings`), the
error code for a location with no entry, and whether a later planning session holds a default
registry.
