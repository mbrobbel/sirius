# Related designs

Other systems put distribution into a plan before Sirius did. This appendix compares the Sirius
Plan IR with four groups of designs:

- Apache DataFusion, where every physical operator declares properties and requirements and the
  optimizer inserts the data movement that satisfies them.
- Ballista, datafusion-distributed and Comet, which take DataFusion plans across processes.
- Spark, Trino and Snowflake, which solved the same problems at scale with a coordinator in the
  middle.
- The Velox family (Velox, Axiom, Bolt and Apache Gluten), which splits planning and execution
  into separate projects.

It credits the ideas Sirius borrows and says where Sirius chooses differently. Sirius is an
explicit, checkable layer *under* a planner and does not replace one.

The descriptions of other systems follow their public documentation and papers, linked inline.
Several DataFusion APIs named here change between releases; the linked documentation is the
authority.

## Apache DataFusion

[Apache DataFusion](https://datafusion.apache.org/) is an extensible query engine written in Rust on
Apache Arrow. It has a SQL frontend, a logical plan, an optimizer and a physical plan of
`ExecutionPlan` operators that run in one process. Many systems embed and extend it.

### ExecutionPlan and its properties

Every physical operator implements the
[`ExecutionPlan`](https://docs.rs/datafusion/latest/datafusion/physical_plan/trait.ExecutionPlan.html)
trait. An operator reports its children, can be rebuilt with new children, and starts work with
`execute(partition, context)`, which returns a stream of Arrow record batches for one *partition*.
A DataFusion partition is a unit of parallelism inside one process: an operator with four output
partitions produces four independent streams.

An operator describes its output in
[`PlanProperties`](https://docs.rs/datafusion/latest/datafusion/physical_plan/struct.PlanProperties.html):
its equivalence properties (orderings, equal expressions and constants), its output
[`Partitioning`](https://docs.rs/datafusion/latest/datafusion/physical_plan/enum.Partitioning.html),
its emission type and whether its output is bounded. `Partitioning` is one of `RoundRobinBatch(n)`,
`Hash(exprs, n)`, `UnknownPartitioning(n)` and, in recent releases, `Range` for ranges a data
source declares.

### Requirements and the passes that enforce them

What an operator *needs* from its inputs is a separate set of trait methods:

- a required input distribution per child, a
  [`Distribution`](https://docs.rs/datafusion/latest/datafusion/physical_plan/enum.Distribution.html):
  unspecified, a single partition, or partitioned so that equal key values land in the same
  partition (`HashPartitioned`, `KeyPartitioned` in recent releases);
- a required input ordering per child;
- whether the operator maintains its input order, and whether it benefits from more input
  partitions.

The physical optimizer reads these as data. `EnforceDistribution` inserts repartitioning
(`RepartitionExec`) or a merge to one partition (`CoalescePartitionsExec`) wherever a child's
partitioning does not satisfy its parent's requirement. `EnforceSorting` inserts sorts and
order-preserving merges. Recent releases fold both into one rule,
[`EnsureRequirements`](https://docs.rs/datafusion-physical-optimizer/latest/datafusion_physical_optimizer/),
a name and a design that go back to Spark. A final
[`SanityCheckPlan`](https://docs.rs/datafusion/latest/datafusion/physical_optimizer/sanity_checker/struct.SanityCheckPlan.html)
pass rejects any plan whose order or distribution requirements are still unmet, and any plan that
puts a pipeline-breaking operator on an infinite input.

Whether a partitioning satisfies a requirement depends on
[`EquivalenceProperties`](https://docs.rs/datafusion/latest/datafusion/physical_expr/struct.EquivalenceProperties.html).
If `a` and `b` always hold the same value and the data is hash-partitioned on `b`, a requirement
on `a` is already met. The same tracking skips a sort when an equivalent ordering already holds.

### Aggregation in phases

[`AggregateMode`](https://docs.rs/datafusion/latest/datafusion/physical_plan/aggregates/enum.AggregateMode.html)
names the phase of an aggregate operator:

| `AggregateMode` | What it does |
|---|---|
| `Partial` | First phase, over any input partitioning; emits intermediate state. |
| `Final` | Merges partial states in exactly one partition. |
| `FinalPartitioned` | Merges partial states on input that is partitioned by the group keys. |
| `Single` | The whole aggregate in one operator, over one partition. |
| `SinglePartitioned` | The whole aggregate in one operator, over input partitioned by the group keys. |
| `PartialReduce` | Merges partial states into new partial states, for multi-stage reduction. |

`PartialReduce` lets distributed systems reduce shuffle traffic, as `Aggregate combine` does in
Sirius.

The aggregate function defines the intermediate state. An
[`Accumulator`](https://docs.rs/datafusion/latest/datafusion/logical_expr/trait.Accumulator.html)
returns its state as a list of scalar values from `state()`, merges other accumulators' states in
`merge_batch()`, and produces the result in `evaluate()`. An
[`AggregateUDFImpl`](https://docs.rs/datafusion/latest/datafusion/logical_expr/trait.AggregateUDFImpl.html)
declares the fields of that state with `state_fields()`: one field for `SUM` or `MIN`, more for
`AVG`.

A two-phase grouped sum as DataFusion's `EXPLAIN` prints it (abridged), then the same shape in the
Plan IR:

```text
AggregateExec: mode=FinalPartitioned, gby=[l_returnflag@0 as l_returnflag], aggr=[sum(l_quantity)]
  RepartitionExec: partitioning=Hash([l_returnflag@0], 4), input_partitions=4
    AggregateExec: mode=Partial, gby=[l_returnflag@0 as l_returnflag], aggr=[sum(l_quantity)]
      DataSourceExec: file_groups={4 groups: [...]}, projection=[l_returnflag, l_quantity]
```

```text
Aggregate final keys=[l_returnflag]
└─ Exchange x1 hash(l_returnflag) -> compute[4]
   └─ Aggregate partial keys=[l_returnflag] measures=[sum_qty = sum(l_quantity)]
      │  -> (l_returnflag string, sum_qty sum_state<decimal(38,2)>)
      └─ Scan lineitem [l_returnflag, l_quantity]  on compute[4]  shards=files_round_robin
```

The trees match node for node and differ in what each line commits to:

- `Hash(..., 4)` names four partitions of one process and leaves the hash function to the engine.
  `Exchange x1` names a target set of four workers that may live in different processes, and its
  partitioning signature includes `HashScheme::v1`.
- The DataFusion partial aggregate's state is whatever its accumulators return. The Sirius partial
  aggregate prints its state type in the schema.

### Dynamic filters

DataFusion passes information between operators at run time through
[`DynamicFilterPhysicalExpr`](https://docs.rs/datafusion/latest/datafusion/physical_expr/expressions/struct.DynamicFilterPhysicalExpr.html),
described in the post
[Dynamic Filters: Passing Information Between Operators During Execution](https://datafusion.apache.org/blog/2025/09/10/dynamic-filters/).
It is an ordinary physical expression whose contents any holder of a reference can replace. Two
producers use it:

- a TopK operator (a sort with a limit) tightens the filter as its heap improves, so the scan can
  skip rows, row groups and files that cannot enter the top K;
- a hash join, once its build side is complete, publishes what it learned about the build keys,
  such as their minimum and maximum, so the probe-side scan can drop rows that cannot match. The
  post describes this for inner hash joins.

Each update advances a generation counter. A consumer can take a snapshot of the current
expression, wait for the next update, or wait until the producer marks the filter complete.
Filters reach the scan through filter pushdown: an operator collects filters for its children on
the way down and decides on the way up which ones it absorbed.

The filter is shared state in one process: a lock-protected reference that the producer swaps and
the consumers read. The post notes that a distributed engine can send a snapshot to another
process, but the filter does not record which rows it covers.

### Extension points

Most of DataFusion's extension points have a counterpart in Sirius's
[Extension points](../reference/extension-points.md):

| DataFusion | What it adds | Sirius counterpart |
|---|---|---|
| [`TableProvider`](https://datafusion.apache.org/library-user-guide/custom-table-providers.html) | A table: schema, scan, filter and limit pushdown | `TableSource`, and registered scan kinds ([question 14](../reference/open-questions.md#14-host-registered-scan-kinds)) |
| [Scalar, aggregate, window and table functions](https://datafusion.apache.org/library-user-guide/functions/adding-udfs.html) | Functions with their types; aggregates with state fields and accumulators | Functions in a `Registry`, each with a determinism class; aggregate state types with named combine and final functions |
| [`UserDefinedLogicalNode`](https://docs.rs/datafusion/latest/datafusion/logical_expr/trait.UserDefinedLogicalNode.html) with an `ExtensionPlanner` | A new logical operator and its physical planning | No logical layer; a registered node declares the node contract |
| A custom `ExecutionPlan` | A new physical operator with its properties and requirements | A registered node kind, later |
| [`PhysicalOptimizerRule`](https://docs.rs/datafusion-physical-optimizer/latest/datafusion_physical_optimizer/optimizer/trait.PhysicalOptimizerRule.html) | A plan-to-plan rewrite: `optimize(plan, config)`, a name, and whether the schema must not change | A rewrite in `sirius::rewrite`: a function from plan to plan |

The main structural difference: a DataFusion operator states its properties and requirements in
trait methods, and a Sirius node states them as data in its contract. A trait method can compute
anything. A declaration can be checked by the validator, read by a front-end's planner and covered
by a fingerprint.

## DataFusion across processes

Three projects take DataFusion plans beyond one process. They differ in who plans and who
schedules. All three build the plan once and ship it to the processes that run it.

### Ballista

[Ballista](https://datafusion.apache.org/ballista/contributors-guide/architecture.html) is a
distributed engine with a scheduler and executors. The scheduler plans the query with DataFusion and
breaks the physical plan into stages wherever the partitioning changes. Each `RepartitionExec`
becomes a pair: its input is wrapped in a `ShuffleWriterExec`, and an `UnresolvedShuffleExec` holds
its place in the consuming stage. Executors poll the scheduler for tasks, and each task runs one
stage's plan over its share of the input.

The [shuffle](https://datafusion.apache.org/ballista/contributors-guide/shuffle.html) is blocking. A
stage runs to completion and writes its output to the executor's local storage, one file per output
partition per task, before any downstream stage starts. When every input stage is complete, the
scheduler replaces each `UnresolvedShuffleExec` with a `ShuffleReaderExec` that knows the concrete
locations, and the reader fetches the files locally or from the producing executor.

Materialized shuffle output makes recovery cheap. When a fetch fails or an executor is lost, the
scheduler rolls back the consuming stage and re-runs only the map tasks whose output was lost. A
retry budget caps repeated failures. Ballista can also build stages incrementally, re-optimizing
the rest of the plan after each shuffle stage from observed row counts and sizes.

The closest Sirius shape is an exchange with `Durability::spooled()`, which retains the sender's
output so that a failed stage instance can re-run under a new attempt. The plan chooses spooling
per exchange; pipelined is the default. See
[Durability](../part-5/07-failure-cancellation-and-recovery.md#durability-pipelined-or-spooled) and
[Retrying a stage instance](../part-5/07-failure-cancellation-and-recovery.md#retrying-a-stage-instance).

### datafusion-distributed

[datafusion-distributed](https://github.com/datafusion-contrib/datafusion-distributed) is a
library, not an engine, for building distributed engines on DataFusion. A distributed plan is an
ordinary DataFusion physical plan with a few extra nodes that stream Arrow data between machines.

A distributed planner runs after DataFusion's own physical planning. It walks the single-process
plan from the leaves up and places *network boundaries* in it: `NetworkShuffleExec` to redistribute
partitions across workers, `NetworkCoalesceExec` to collapse many tasks into fewer, and a broadcast
boundary for the build side of a hash join. It then divides the plan into stages at those
boundaries and picks a task count for each stage. Any node can act as the coordinator, which builds
the plan and sends each task's stage to one of the available workers; a user-supplied handler can
choose which.

Sirius ships no such planner; that work belongs to its front-ends. The front-end writes the
exchanges, with its own planner or by hand, so they are nodes from the start, and the validator
checks each one before anything runs.

### Comet

[Comet](https://datafusion.apache.org/comet/contributor-guide/plugin_overview.html) accelerates
Apache Spark by running parts of Spark's physical plan on DataFusion. While Spark plans a query
stage, Comet's rules replace supported scans and operators, bottom up, with native equivalents. An
operator, expression or type Comet does not support leaves that part of the plan to Spark. Spark
keeps everything around the operators: it plans, schedules stages and tasks, runs the shuffle
service, and ships the native plan to the executors with each task.

The shuffle is the relevant part for Sirius. Native and Spark operators must agree on where each
row goes, so Comet's
[native shuffle](https://datafusion.apache.org/comet/contributor-guide/native_shuffle.html)
implements Spark-compatible hash partitioning: Murmur3 with seed 42, then the hash modulo the
partition count. Comet's round robin does not place rows where Spark's would, so it is off by
default. For a range partitioning, Spark samples the data and computes the boundaries on the
driver, and Comet uses them.

Sirius calls this contract a hash scheme. In Comet it is an engineering promise between two code
bases. In Sirius it is a named, versioned part of the plan: `HashScheme::v1` fixes the function,
the seed and key canonicalization. A scheme that matches Spark's Murmur3 hash, or Iceberg bucket
hashing, would be another registered name
([question 25](../reference/open-questions.md#25-open-registries-and-versioning)).

## Spark, Trino and Snowflake

**Spark.** Spark's physical operators declare required child distributions and orderings, and a
rule named `EnsureRequirements` inserts shuffles and sorts to meet them. Spark's
[adaptive query execution](https://spark.apache.org/docs/latest/sql-performance-tuning.html)
re-optimizes the plan during execution from runtime statistics. It coalesces small post-shuffle
partitions, turns a sort-merge join into a broadcast or shuffled hash join when a side turns out
small, and splits skewed join partitions. The driver, which sees all statistics, does the
re-planning. In Sirius a `Decide` node picks among alternatives that are all in the plan, so the
stage graph never changes
([Adaptive plans](../part-2/06-adaptive-plans.md#the-adaptive-loop)), and moderate skew gets an
explicit partition-to-worker assignment
([Skew](../part-5/08-running-at-scale.md#skew-more-partitions-than-workers)). A
*storage partition join* avoids the shuffle when the tables' storage layout already partitions
both sides on the join keys; in Sirius, partitioning declared by a source is
[question 13](../reference/open-questions.md#13-hints-and-certified-properties).

**Trino.** Trino runs a query as a tree of
[stages](https://trino.io/docs/current/overview/concepts.html), each carried out by tasks on
workers, connected by exchanges. The coordinator asks the connectors for the splits of each table
and hands them to tasks as the query runs. Its
[dynamic filters](https://trino.io/docs/current/admin/dynamic-filtering.html) are applied locally
for broadcast joins; for partitioned joins the coordinator collects them and sends them to the
workers. Execution is pipelined by default. With
[fault-tolerant execution](https://trino.io/docs/current/admin/fault-tolerant-execution.html)
enabled, the retry policy `QUERY` retries a whole query and `TASK` retries individual tasks, which
requires an exchange manager that spools intermediate data. Sirius is also pipelined by default
and spooled by choice, but chooses per exchange where Trino chooses for a whole query
([Durability](../part-5/07-failure-cancellation-and-recovery.md#durability-pipelined-or-spooled)).

**Snowflake.** The paper
[The Snowflake Elastic Data Warehouse](https://dl.acm.org/doi/10.1145/2882903.2903741) (SIGMOD 2016)
describes a columnar, vectorized, push-based engine coordinated by a central services layer. The
optimizer assigns table files to worker nodes by consistent hashing over file names, so repeated
queries hit the same node's cache. Skew is handled at the scan: a worker that finishes its files
asks its peers for more, and a peer with files left transfers ownership of one for the rest of the
query, which the paper calls *file stealing*. Worker failures are handled by retrying queries. The
Sirius counterpart of file stealing is `Shards::dynamic()`, written in the plan as
`shards=dynamic`: the workers of a set claim the splits of a scan at run time, exactly once each
([Dynamic splits](../part-5/08-running-at-scale.md#dynamic-splits)).

## The Velox family

[Velox](https://github.com/facebookincubator/velox) is Meta's C++ execution library: typed vectors,
a tree of plan nodes, tasks, drivers and operators, and connectors. It leaves SQL planning,
optimization and scheduling to the systems built on it, so it always runs under someone else's
planner. Several projects around it divide the work in ways that match the integration levels of
Part IV.

- **Velox's cuDF backend.** Velox's
  [GPU acceleration](https://facebookincubator.github.io/velox/develop/gpu-acceleration.html) page
  describes an experimental backend that plugs into Velox's `DriverAdapter` extension point and
  rewrites a plan to run on the GPU, "generally replacing operators one-to-one" with
  libcudf-backed operators. Operators it does not support fall back to the CPU, and a separate,
  also experimental exchange moves device buffers between workers. It accelerates operators inside
  Velox tasks and leaves the plan and the data movement to Velox. Sirius works one level up: it
  takes a whole plan in which every exchange, filter edge and decision is a node, checks it, and
  runs it across GPUs and hosts.
- **Axiom.** [Axiom](https://github.com/facebookincubator/axiom) is Meta's set of front-end
  components for Velox, with a cost-based optimizer that cuts the plan into fragments with explicit
  exchanges. It hands its decisions over as a public object, which is what a layer under a planner
  needs; see [Plans from other systems](../part-4/05-plans-from-other-systems.md#a-multi-fragment-plan-from-axiom). Its design for
  cross-fragment dynamic filters, not implemented yet, is close to Sirius value edges, except that
  its scans block until the filter arrives
  ([question 6](../reference/open-questions.md#6-filter-waits)).
- **Bolt.** [Bolt](https://github.com/bytedance/bolt) is ByteDance's execution library, initially
  derived from Velox, also without an optimizer or scheduler. Its main use is inside Spark tasks;
  see [Plans from other systems](../part-4/05-plans-from-other-systems.md#one-task-at-a-time-bolt-under-spark).
- **Apache Gluten.** [Gluten](https://github.com/apache/gluten) is "a middle layer responsible for
  offloading JVM-based SQL engines' execution to native engines". For Spark it replaces supported
  operators of the physical plan with native ones, much as Comet does, and runs them on a backend
  such as Velox or Bolt, while Spark keeps the stages, the shuffle and the scheduler. The native
  side must still route rows exactly as Spark would, the contract [Comet](#comet) keeps with its
  native shuffle.

The family shows that planning and execution can live in different projects and still agree, as
long as the plan between them is complete; Axiom's multi-fragment plan is close to what the Plan IR
asks for. The agreements that cross a process boundary (partition functions, aggregate state
layouts, function semantics) are engineering promises between code bases, as in Comet. Sirius
makes them named, versioned parts of the plan: hash schemes
([question 18](../reference/open-questions.md#18-hash-scheme-stability)) and registered dialect
functions and state types ([question 31](../reference/open-questions.md#31-dialect-semantics)).

## Side by side

| Concept | DataFusion | Sirius |
|---|---|---|
| Unit of parallelism | A partition: one stream inside one process | A worker: one logical slot on one GPU, in any process |
| Output facts | `PlanProperties`: partitioning, equivalence properties, emission, boundedness | `Distribution` on a worker set, plus `sorted(keys)`, derived for every node |
| Input requirements | Trait methods per child: distribution, ordering | Declared as data in the [node contract](../reference/extension-points.md#requirements-are-data) |
| Who inserts movement | The optimizer (`EnforceDistribution`, `EnforceSorting`, now `EnsureRequirements`) | The front-end that writes the plan; Sirius inserts nothing on its own and validates every exchange |
| Final check | `SanityCheckPlan` | `Plan::create` (V1-V20, V24, V26-V28), `Context::check`, `Context::prepare` ([Validation](../part-2/07-validation.md)) |
| Hash partitioning | `Hash(exprs, n)`; the function is the engine's | `hashed(keys, sig)`; the signature names the scheme, key types, partition count and target set |
| Equal columns | Equivalence classes and constants | Inner join outputs count as hashed on either key list; general classes are [question 27](../reference/open-questions.md#27-requirements-as-data-and-equivalence-properties) |
| Aggregate phases | `Partial`, `Final`, `FinalPartitioned`, `Single`, `SinglePartitioned`, `PartialReduce` | `single`, `partial`, `combine`, `final`; the input distribution decides legality (V6) |
| Intermediate state | Accumulator `state()` values, described by `state_fields()` | A state type in the schema, with a fixed layout and named combine and final functions |
| Runtime filters | `DynamicFilterPhysicalExpr`: shared, updatable, in one process | `RuntimeFilterBuild`, `ValueExchange`, `RuntimeFilterApply`: value edges with a scope |
| Refinement | A filter updates many times; a generation counter tracks it | One value per producing worker; refinable values are additive later |
| Adaptivity | Re-optimization between stages in Ballista, as in Spark | `Decide` among labeled alternatives in the plan: exchange modes, partition counts and `Choose` arms |
| Across processes | Added by a distributed layer; plans built once and shipped | Exchanges and value edges are in the plan from the start; peers check edge signatures at start |
| Extension | Traits for tables, functions, nodes and optimizer rules | Registered members with identities, in an immutable `Registry` that construction takes; a node contract stated as data |

## What Sirius borrows

Four ideas come from this lineage.

- **Requirements as data, without the insertion pass.** DataFusion and Spark let each operator say
  what it needs and let one optimizer pass meet every need. Sirius keeps the first half. Every node
  declares its required input distributions and orderings as values
  ([Requirements are data](../reference/extension-points.md#requirements-are-data)). The validator
  fails closed with `invalid_plan` and an error that says what to insert, and a front-end planner
  reads the same declarations to place its exchanges. How far requirements as data and equivalence
  properties go is
  [question 27](../reference/open-questions.md#27-requirements-as-data-and-equivalence-properties).
  A small helper that spreads a plan over the GPUs of one machine, never over many, is
  [question 36](../reference/open-questions.md#36-a-distribution-helper-for-one-machine).
- **Equivalence properties.** The rule that an inner join's output is hashed on either key list is
  a small equivalence class: after `l_orderkey == o_orderkey`, the two columns are interchangeable
  for any requirement. DataFusion tracks such classes, and constants, in general; question 27 asks
  whether Sirius should too. Equivalence must respect join types. After a left join an unmatched
  line item has a null `o_orderkey` on the worker its `l_orderkey` hashed to, so the two columns
  are not equivalent there
  ([The Distribution property](../part-2/02-workers-and-distribution.md#the-distribution-property)).
- **State as a schema plus a named merge.** DataFusion describes an aggregate's intermediate state
  with `state_fields` and merges it in `merge_batch`; each accumulator defines the state, which can
  vary with the implementation. A Sirius state type is an Arrow schema plus combine and final
  functions identified by namespace, name, signature and version
  ([Closed and open lists](../reference/extension-points.md#closed-and-open-lists)). A partial
  aggregate prints its state types, such as `sum_state<decimal(38,0)>` for an integer sum, and V5
  checks that a final aggregate reads exactly the keys and states of one partial signature. A
  final aggregate in another process, or in another engine build, needs only the schema
  ([State types](../part-2/03-exchanges.md#state-types-travel-in-the-schema)).
- **Refinable values.** A TopK dynamic filter improves while the query runs, and a consumer may
  use any snapshot: each is correct, only less selective. Sirius filters already allow this on the
  consumer side: `wait=never` takes a snapshot at each checkpoint. A value kind that publishes
  more than once, each time tighter, such as a top-N bound, is listed as
  [additive later](../reference/extension-points.md#additive-later). It fits because filters are
  optional and their merge rules are associative and commutative: a monotone sequence of shares
  merges to the same final value whatever arrives first. Today's engine already publishes such a
  bound from its top-N operator, inside one process and under the conditions that
  [Runtime filters today](../part-2/05-runtime-filters.md#runtime-filters-today) lists.

## Where Sirius differs

DataFusion's properties are promises inside one process, between operators compiled together.
Sirius's properties are promises between workers that may live in different processes, built by
different code, sometimes by different hosts, so every promise must be checkable on both sides.

- **Checkable cross-process partitioning signatures.** In one process, `Hash(exprs, 4)` is enough:
  one hash function, one partition count, one address space. Across processes it is not, as
  Comet's match of Spark's Murmur3 seed shows. A Sirius `hashed` distribution carries a
  [partitioning signature](../part-2/03-exchanges.md#the-partitioning-signature-and-co-partitioning),
  and two edges are co-partitioned only when their signatures are equal, which every worker can
  check. The one exception is a `mark` join, whose build exchange may route nulls `to_all` against
  a `hashed` probe.
- **Scoped value edges.** A DataFusion dynamic filter is shared memory, so every consumer sees the
  producer's view, whatever rows that view covered. A Sirius runtime filter carries a
  [scope](../part-2/05-runtime-filters.md#scope-which-rows-does-a-filter-know-about), and a
  `partial` filter cannot be applied (V16) until a `ValueExchange` has merged every share, so a
  filter built from a quarter of the keys cannot drop rows that match the other three quarters.
- **Coordinator-free decisions.** Spark's adaptive execution and Ballista's stage-by-stage
  planning decide on a driver or scheduler that sees all statistics, and Trino collects
  partitioned dynamic filters at its coordinator. A Sirius `Decide` reads a global value, and
  every worker computes the same label from it with a pure policy
  ([Adaptive plans](../part-2/06-adaptive-plans.md#why-every-worker-picks-the-same-label)); an
  `AssignPartitions` policy works the same way. A plan that wants one place to decide names a
  leader explicitly.
- **Edge signatures.** DataFusion-based systems agree on a plan because one process built it and
  shipped it. Sirius allows that but does not rely on it. In a closed plan every process builds
  the same plan and compares fingerprints. In an open plan, such as one StarRocks fragment
  instance, each side compares an
  [edge signature](../part-2/04-stages-and-bindings.md#every-process-prepares-the-same-plan) at
  start, which covers the edge's parameters, the IR version, the wire version and the identity of
  every registered item on the edge. A mismatch fails before the first row moves. How whole plans
  might travel between processes is
  [question 1](../reference/open-questions.md#1-how-plans-travel).
- **No re-planning.** Systems that re-optimize between stages can change the stage graph halfway
  through a query. In Sirius every alternative is in the plan (an exchange mode, a partition count
  or a whole subtree under a
  [`Choose`](../part-2/06-adaptive-plans.md#choosing-between-subtrees)), so the stage graph is
  fixed, and `report.resolved_plan()` shows which alternative ran. `prepare` never rewrites the
  plan; it only resolves worker-local `auto` properties, such as a join's
  [build side](../reference/node-catalog.md#join). That gives up some adaptivity, such as
  inventing a join order mid-query, for a plan that is fully known before it starts. Whether a
  host should be able to chain plans and re-plan between them is
  [question 33](../reference/open-questions.md#33-re-planning-beyond-the-plans-alternatives).

## Positioning: the layer under a planner

DataFusion, Spark and Trino are planners and engines at once. They parse SQL, choose join orders
from costs, decide where to repartition, and then run the result. Sirius does not plan. It takes a
plan whose decisions are already made, or are made by a pure rule inside the plan, states them in
one printable IR, checks that they are consistent before anything runs, and runs them on GPUs.

Each host hands over its decisions in its own shape, from a whole query to one task at a time
([Who owns what](../part-4/04-one-ir-three-integrations.md#who-owns-what)). A planner of the
DataFusion kind could sit on top in the same way: its requirement passes would decide where data
moves, and the Plan IR would record those decisions in a form that every worker can check. The
optimizer and its choices stay the host's.
