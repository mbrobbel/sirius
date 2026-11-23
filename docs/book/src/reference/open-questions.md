# Open questions

The plan owns every fact two workers must agree on and every property later nodes rely on; the
engine owns how each edge is carried out. These questions are what that division leaves open: some
concern what lives *around* the plan (lifecycle, failure, coordination), others how far the plan
vocabulary should grow. Chapters refer to them by number. A question settled elsewhere is marked
**Answered** or **Partly answered**, links there, and keeps its number. Each entry states the
question, the options and the current leaning, if there is one. A leaning is not a decision.

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

On one machine, the first measurements support the leaning. The distributed prototype in
[sirius-db/sirius#2337](https://github.com/sirius-db/sirius/pull/2337) runs one process per GPU with
exchanges between processes. On four GB200 GPUs at scale factor 3000, it reached 2.49 times the
speed of one GPU over 20 queries, against 1.56 times for today's single-process engine with its
balancing inside the machine.

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

[Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#how-an-execution-ends)
settles the core (outcomes, fail-stop along edges, the first-contact deadline, single-use execution
ids, message keys, opt-in stage retry). Still open: who distributes endpoints and execution ids,
how a process finds its peers, whether liveness after first contact needs heartbeats, and how hosts
learn whether a failed execution had side effects.

**Leaning:** the host keeps distributing execution ids, and peers keep `Endpoint` bytes. The bytes
need not pass through a coordinator: a StarRocks compute node fetches them from its peers and caches
them, so the frontend ships only each instance's id and backend address and carries no Sirius
fields. Endpoint bytes stay opaque to hosts. They carry at least a transport agent address that
peers can reach and the identity of the device. Buffer addresses and peer-access facts, such as
whether two GPUs can reach each other directly, can wait for a hello at first contact.

Sirius owns readiness, the first-contact deadline (a prepare option) and liveness inside
`Execution`. A transport failure fails one execution, not the engine. An error gains a
classification: whether it is retryable and whether side effects may have happened, derived from
the side-effect classes of the nodes that ran
([Extension points](extension-points.md#side-effect-classes)). A fatal device error makes the
engine unavailable for the whole process, so `runtime_unavailable` is classified as not retryable
within that process (question 15). Which code that process's peers see is open, given that the
origin's error is carried unchanged. Who decides a retry, and through which call, is question 28.

[sirius-db/sirius#1807](https://github.com/sirius-db/sirius/issues/1807) records an alternative for
finding peers: a peer is named by an opaque id that the embedding backend resolves to a transport
agent, through a resolver the host supplies. Then no host handles endpoint bytes, but every process
depends on a host callback before it can reach a peer, and every process's resolver must map the
same id to the same agent. Either way, the number of transport agents per context is the engine's
choice.

## 5. Backpressure

Between processes, transports use receiver credits, and spilling is the buffer behind them: a batch
without credit waits in spillable memory, so a slow receiver causes spilling before it causes a
stall. Credits are part of an edge's physical shape, so they stay out of the plan and the bindings.
Buffers are still bounded, and then waits can block each other: a merging receiver
(`GatherOrder::merge` or `by_worker`) needs the next batch from one particular sender, and a decided
exchange cannot route a row until its required `Decision` arrives. Which waits must the validator
see, and what happens when every tier is full?

**Leaning:** within one execution, deadlock freedom is a property of the plan
([Credits and deadlock freedom](../part-5/08-running-at-scale.md#credits-and-deadlock-freedom)).
Credits are granted per logical stream, so a stream without credit never holds up another stream on
the same connection. V1 requires an acyclic wait-for graph, so a plan whose waits could form a
cycle is rejected when it is built. Optional values never add a wait-for edge, because `wait=never`
never waits and `wait=up_to(t)` gives up after `t`. When every memory tier is full, the execution
fails with an `execution` error instead of hanging.

The plan cannot see waits between executions. Until one engine runs executions concurrently
(question 15), a host must not feed an `Input` from another live Sirius execution in the same
process: the second execution may wait for the slot the first one holds. Chain executions through
completed, spillable results instead: run the producer to completion, then bind its batches to the
consumer as a completed input (question 33).

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

`rewrite::join_stages` composes open instance plans back into a joined plan. It groups them by
hosting set and needs one instance per worker of each set it recovers; an unsized set becomes sized
by the number of instances supplied, whose indices must be 0 to n-1. Its exact rules are open: how
edge signatures must match, and how a plan states whether a stage boundary is materialized or
pipelined (the StarRocks prototype materializes at every exchange).
**Leaning:** every exchange and value id must have sinks in exactly one producing set and sources in
each consuming set, all with equal edge signatures. Materialization is the exchange's durability
property: under `durability=spooled` a receiver consumes a sender attempt's output only after that
attempt completed, and durability is part of the edge signature (see
[Durability](../part-5/07-failure-cancellation-and-recovery.md#durability-pipelined-or-spooled)).

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

Today's `exec::streaming_fragment` has the same rule: a declared input may be read by at most one
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
[leader pattern](../part-2/06-adaptive-plans.md#deciding-on-a-leader)). A report is enough to
replay a run: replay runs the
[resolved plan](../part-2/06-adaptive-plans.md#after-the-run-decisions-and-the-resolved-plan), from
`rewrite::resolve` over `report.decisions()` or from `report.resolved_plan()`, which also
substitutes assignments and dynamic split claims.

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
with the enforcement in [#2457](https://github.com/sirius-db/sirius/issues/2457) and the public
`Context` contract in [#2460](https://github.com/sirius-db/sirius/issues/2460).

[sirius-db/sirius#2461](https://github.com/sirius-db/sirius/issues/2461) stages the execution API
and leaves concurrent executions on one engine and independent contexts per process for after its
first stages. Until then a `Context` may be shared between threads, but its executions may still run
one at a time (question 5). #2461 also names a `Session` that may arrive later as a planning
environment, with a catalog and settings, for catalog-backed or SQL preparation. Nothing in the
proposal depends on it: `check` and `prepare` live on `Context`. Six points remain open:

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
  per process that sees every query.
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
depth limit, are undecided too. How the catalog is extended is partly answered: functions resolve
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

## 18. Hash scheme stability

`HashScheme::v1` fixes the hash function, seed and key canonicalization, so every worker of a hash
exchange must be a Sirius worker. Canonicalization has to be exact for every key type
([Hash canonicalization in v1](extension-points.md#hash-canonicalization-in-v1)). Must `v1` stay
identical across releases and languages, and how do other schemes, such as Spark's Murmur3 hash
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

To reject a plan whose exchange is missing or misplaced, the validator must know what each node
needs from its inputs, such as rows hashed on the grouping keys before a final aggregate. Today
that knowledge is code for the built-in nodes, so a node added through the node contract
([Extension points](extension-points.md#the-node-contract)) would need changes in the validator.
Should every node declare its required input distributions and orderings as data
([Requirements are data](extension-points.md#requirements-are-data)), which the validator checks
and any front-end's planner can read? And should the property model track equivalent columns and
constants, so that a relation hashed on `l_orderkey` also counts as hashed on `o_orderkey` after an
inner join on those keys, and a column fixed by a filter such as `x == 5` can be dropped from a key
list when a requirement is checked?

**Leaning:** yes to requirements as data. The validator checks the declarations, and its typed
error names the unmet requirement and the exchange that would meet it. A front-end's planner reads
the same declarations to place its exchanges, so the planner and validation cannot disagree on what
a node needs. Sirius ships no pass that inserts exchanges: the front-end writes them, where
DataFusion and Spark use a pass inside their own optimizers
([Related designs](../appendix/related-designs.md#what-sirius-borrows)). A helper scoped to one
machine is [question 36](#36-a-distribution-helper-for-one-machine). Equivalence classes generalize
what the inner join already does with its two key lists, and they must respect join types, because
an outer join's unmatched rows carry NULL on the other side. Constants come later.

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
instance waits for the host's answer are open.

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

**Leaning:** casts wherever they give identical values, such as an integer sum under ANSI mode cast
back to int64 with an overflow check; registered functions for hashing, Bloom probes and wrapping
arithmetic; and registered state types only where the host's other engine must read Sirius states.
Until then, both halves of such an aggregate run on Sirius. Every registration follows question 25.

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

Decided exchanges, decided partitions and `Choose` pick only among alternatives written into the
plan before it runs. Some choices cannot be listed in advance at a sensible size: a six-way join
has hundreds of join orders, an estimate off by a factor of a thousand can make every listed
alternative wrong, and the best plan for the rest of a query can depend on sizes that exist only
after its first stages ran. Spark and Ballista re-plan at stage boundaries for this reason
([Related designs](../appendix/related-designs.md#spark-trino-and-snowflake)). Should Sirius
support re-planning, and how? There are two approaches.

**A. Explicit plan chains.** The host cuts the query where everything below the cut is
materialized. It runs plan 1, whose results stay on the workers as retained, partitioned outputs. It
reads the row and logical byte counts from plan 1's report, and builds plan 2, which reads those
results as sources that declare their partitioning. Sirius never re-plans: the host does,
between immutable plans.

| Pros | Cons |
|---|---|
| Every plan is still immutable, validated and printable before it runs, so none of the IR's guarantees change. | It needs results that outlive an execution: retention, memory and spill limits, cleanup, and what happens when a worker that holds them fails. |
| Any re-optimization is possible, including join orders nobody listed, because a planner builds plan 2 from real sizes. | Each cut is a full barrier. Nothing above it starts until everything below it has finished, so pipelining across the cut is lost. |
| The record is the chain of resolved plans, and each can be replayed. | Every cut adds a round trip to the host, a plan build, a `prepare` and new handshakes. |
| Sirius stays free of planning. The re-planning lives in the host, such as Distributed Sirius or Axiom, where the optimizer already is. | A query is no longer one plan, so tools that read one plan (`explain()`, fingerprints, the report) must understand chains. Hosts that already re-plan, such as StarRocks, and Spark through Bolt, gain nothing. |

**B. No re-planning in Sirius.** Every runtime choice is a `Decide` or a `Choose` in one plan.
Anything that cannot be listed is decided by the host's optimizer before the run. A host that wants
to re-plan anyway does it outside Sirius, for example by ending one execution at an `Output`,
running it to completion, and binding its batches into the next plan as a completed `Input`,
subject to the cross-execution rule in [question 5](#5-backpressure).

| Pros | Cons |
|---|---|
| One query is one plan: the simplest model, and every runtime choice stays a plan value ([rule 3](../part-2/01-everything-in-the-plan.md#the-ownership-rule)). | Alternatives must be listed, and every arm is prepared and admitted, so their number cannot grow far (question 11). |
| No result lifetimes, no new API, and no failure handling across executions. | A misestimate outside the listed alternatives cannot be corrected while the query runs. |
| Every decision is made without a coordinator and recorded in one report. | A host that re-plans anyway moves intermediate results through its own `Output` and `Input`, through host memory, which costs far more than leaving them on the workers. |

A third variant, a plan with a hole that the host fills in while it runs, is left out: the plan
would no longer be known before it starts, a property both approaches keep.

**Leaning:** B for now; A stays possible. What A needs first, results that stay on the workers and
that a later plan can read with a declared partitioning, is useful on its own, for result caching
and shared intermediate results. It builds on spooled exchanges
([Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#durability-pipelined-or-spooled))
and on sources that declare their partitioning ([question 13](#13-hints-and-certified-properties)).
Revisit A when a host wants mid-query re-optimization and the listed alternatives are not enough.

## 34. Scans over GPU-resident data

Today `pin_table` loads a table into GPU memory before any query reads it. It deals the table's
chunks round robin over the GPUs, and a scan of a pinned table runs each chunk on the GPU that
already holds it, so the scan never copies. In the plan, a scan's splits are assigned to workers
when the scan is built, and the bindings later say which GPU each worker runs on. A pinned chunk
sits on one GPU before either is known. If the plan gives it to a worker bound to another GPU, the
engine copies it there: the result is correct, but the point of pinning is lost. How should a scan
over GPU-resident data be written? There are three approaches.

**A. Residency as input to the builder.** The context reports where its cached chunks live, as
splits per endpoint. The host builds the scan with `explicit_splits`, so that worker *k* reads what
one GPU holds, and binds worker *k* to that GPU. `prepare` counts the splits that must be copied
and records them in the report.

| Pros | Cons |
|---|---|
| The plan still says which worker reads which split, and nothing new enters the IR. | The plan depends on the cache when it is built. An eviction or a new pin between building and running turns resident reads into copies. |
| A mismatch between plan and cache is visible in the report, never silent. | The host must know the bindings while it builds the plan, which couples the two steps the book keeps apart. |
| It works the same for one process and for several. | It cannot follow GPUs that admission picks late ([question 35](#35-choosing-gpus-at-admission)). |

**B. Dynamic claims that prefer resident splits.** The scan uses `Shards::dynamic()`
([question 22](#22-dynamic-split-assignment)). Each worker claims the splits resident on its own
GPU first, then any split no one has claimed.

| Pros | Cons |
|---|---|
| No new shard kind. It is already legal, because the scan's output is `arbitrary(W)`. | It only works where no later node relies on which worker reads which split. |
| It follows cache changes and any GPU choice made at admission. | The assignment is known only after the run; the report records it. |
| A GPU that runs ahead takes over unread splits, which also brings back some balancing inside a machine. | A worker that falls behind leaves its resident chunks to others, which then copy them. |

**C. A new shard kind, `Shards::resident()`.** `prepare` resolves which worker reads which split
from the cache and the bindings, so every split is read where it lives.

| Pros | Cons |
|---|---|
| The simplest form for hosts, and every read is local while the cache holds. | The plan no longer says which worker reads which split; the bindings decide it. That bends "bindings say where, never what", even though no later node relies on it. |
| No run-time claims. | The fingerprint cannot cover the assignment, so the resolved plan and the report must carry it. |

A fourth case touches [question 13](#13-hints-and-certified-properties). If a table is pinned by
key, with the same hash scheme and partition count as an exchange, a co-partitioned join could skip
that exchange ([sirius-db/sirius#1018](https://github.com/sirius-db/sirius/issues/1018)). Then
residency is no longer only about speed: the scan declares `hashed` on its keys, and each partition
must be read by the worker that owns it, which makes the chunk-to-worker assignment a plan fact.

[sirius-db/sirius#2545](https://github.com/sirius-db/sirius/issues/2545) lists pinned tables with
the DuckDB storage scan that it plans to turn into a connector supplied by the extension. It does
not say who then reports residency, the context or the registered scan kind.

**No leaning yet.**

## 35. Choosing GPUs at admission

Today admission narrows each query to a subset of the process's GPUs (`gpus_per_query`,
`admission_bytes_per_gpu`). The subset is always the first *k* GPUs of the engine's sorted list, so
concurrent queries share the same leading GPUs and idle ones can stay idle. Which subset to pick is
upstream work
([sirius-db/sirius#1749](https://github.com/sirius-db/sirius/issues/1749)). In the plan, the set
size is fixed when the plan is built, `compute[2]` for example, and `prepare` maps workers to
endpoints through the bindings. On one machine, who picks *which* GPUs? There are two approaches.

**A. Fixed at prepare.** The bindings name the GPUs. `Bindings::local()` deals each set round robin
from the first endpoint, the same way every time. Admission only waits until those GPUs have room.

| Pros | Cons |
|---|---|
| `prepare` knows everything, and per-device checks and preparation happen once. | Concurrent small queries on `compute[1]` all land on GPU 0, as they do today, unless the host rotates its bindings itself. |
| The same model as several processes, where the bindings are agreed before the run. | The host must pick GPUs without knowing the engine's load and free memory. Idle GPUs can stay idle. |

**B. `Bindings::local()` resolved at admission.** Explicit `Bindings::builder()` devices stay fixed.
`Bindings::local()` means "any *k* GPUs of this context", and admission picks them when it admits
the execution, by free memory and load.

| Pros | Cons |
|---|---|
| It spreads concurrent queries over idle GPUs, which today's engine does not do. | `prepare` cannot finish binding: its device checks must cover every GPU the execution might get, or run again at admission. |
| The plan text and fingerprint stay the same, because bindings never change what a worker ends up with. The report records the GPUs chosen. | Work that needs the device, such as prefetching scan data, waits until admission. |
| Hosts on one machine need no load information. | It only works inside one process; several processes must agree on their bindings first. Two kinds of bindings then behave differently. |

Both need engine-level admission ([question 15](#15-one-context-per-process)). Approach B also
interacts with scans over pinned data ([question 34](#34-scans-over-gpu-resident-data)): GPUs picked
late fit dynamic claims, but not a plan built against the cache.

**No leaning yet.**

## 36. A distribution helper for one machine

Today the engine distributes every plan over the GPUs of one process on its own: the `wrap_*`
passes in `insert_gpu_pipeline_operators` insert the partitions, merges and broadcasts
([Plan-to-plan rewrites](../part-5/05-plan-to-plan-rewrites.md#todays-multi-gpu-wrapping-moves-to-the-front-end)).
In the explicit design the front-end writes every exchange, so sirius-duckdb must write those of a
plan on `compute[N]` ([Embedded in a host](../part-4/01-duckdb-extension.md#inside-one-machine)).
Front-ends that run on many machines, such as StarRocks, Axiom and Spark through Bolt, already plan
distribution, so this question is only about single-node multi-GPU. Who turns a single-worker plan
into one that uses every GPU of one machine? There are three approaches.

**A. No helper.** sirius-duckdb's translation places the exchanges itself. It is a front-end,
and it already translates DuckDB's plan node by node.

| Pros | Cons |
|---|---|
| Sirius owns no planner. | sirius-duckdb carries today's `wrap_*` policy, such as partition counts and broadcast or hash. |
| One owner per policy: the front-end that knows the query. | Any other single-node front-end must write its own. |

**B. A Sirius helper scoped to one machine.** It turns a single-worker plan into one on
`compute[N]` bound to one process. It works mechanically from the requirements each node declares
([question 27](#27-requirements-as-data-and-equivalence-properties)). Every policy choice, such as
the threshold between broadcast and hash or the partition count, is supplied by the caller or
emitted as a `Decide`.

| Pros | Cons |
|---|---|
| It reproduces today's single-node multi-GPU behavior for any single-node front-end. | It is a planner in Sirius, even if a small one. |
| Its output is an ordinary plan, visible in `explain()` and checked by validation. | There will be pressure to grow it to many machines. |
| | It must keep up with every node kind. |

**C. Keep the engine's implicit wrapping, for single-node multi-GPU only.**

| Pros | Cons |
|---|---|
| No work. | It breaks [rule 1](../part-2/01-everything-in-the-plan.md#the-ownership-rule): the exchanges the engine infers are not nodes. |
| | Single-node multi-GPU plans are not visible in `explain()` and not checked by validation. |
| | There are two execution models to maintain. |

**No leaning yet.** B is worth exploring for single-node multi-GPU only, never as a planner for
many machines.

## 37. Remote-source settings and credentials

A scan of an object store needs more than a URI: an endpoint, a region and credentials. Which
object owns them, and what may a plan or its printout reveal?
[sirius-db/sirius#2461](https://github.com/sirius-db/sirius/issues/2461) asks this. The earlier
bridge, `<sirius/ffi.hpp>`, keeps an immutable snapshot of object-store settings (endpoint, region,
credentials and an optional URI prefix) on its context, replaces it as a whole when credentials
rotate, and captures it when binding begins
([#2348](https://github.com/sirius-db/sirius/pull/2348)). A C builder for one such configuration
is the draft [#2609](https://github.com/sirius-db/sirius/pull/2609).

**Leaning:** the plan carries only the location, and the caller passes the settings to `prepare`.

- **The settings registry is an immutable value.** It is a list of object-store configurations,
  each with a URI prefix, and is unrelated to the `Registry` of plan construction. Adding or
  dropping an entry returns a new registry and leaves the old one unchanged.
- **`prepare` takes the registry.** For each scan it uses the entry with the longest matching
  prefix, and the prepared plan keeps that choice. A location with no matching entry fails
  `prepare`. A host rotates credentials by passing a new registry to the next `prepare`; a plan
  that is already prepared never sees the change.
- **The `Context` holds no settings.** A process has one engine and several hosts can share it,
  so a registry stored there would share their credentials. The engine may still share
  connections, per endpoint.
- **Nothing secret is a plan fact.** Settings and credentials never enter the plan or its
  fingerprint, so two processes with different credentials for one location still agree on the
  plan. A URI is a location, not a credential: put no secret in it.
- **Printouts redact.** `explain()` and error messages print a URI without its user-info, query
  and fragment, in case a secret slipped in.

Still open: how the registry reaches `prepare` (an argument, `PrepareOptions` or `Bindings`), the
error code for a location with no entry, and whether a later planning session holds a default
registry.
