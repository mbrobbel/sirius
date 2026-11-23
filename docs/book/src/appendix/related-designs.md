# Related designs

When two inputs to a join are partitioned differently, someone has to move rows. DataFusion's
physical optimizer can insert a repartition. Spark can revise the plan once the shuffle sizes
are known. The Sirius builder proposed here lets a host make those choices, then check the plan
it produced. The sections below follow that distinction through aggregates, filters and recovery.

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
  partition (`KeyPartitioned` in DataFusion 55, previously `HashPartitioned`);
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

Ballista and datafusion-distributed distribute DataFusion plans. Comet embeds DataFusion
inside Spark tasks and leaves the stage scheduler to Spark.

### Ballista

Ballista breaks a query into stages at shuffle boundaries. The scheduler assigns tasks to
executors. A writer materializes its output, and a downstream stage starts once its inputs
are complete and the scheduler knows where to find their partitions. This frees the producer's
execution slots before the consumer needs them. [Ballista architecture](https://datafusion.apache.org/ballista/contributors-guide/architecture.html)

The blocking shuffle gives the scheduler a recovery point. When a fetch fails, the scheduler
can roll back the affected consumer and regenerate missing partitions. Ballista also has an
adaptive planning path that uses statistics from completed stages. Failure handling on that
path has separate limitations. [Shuffle design](https://datafusion.apache.org/ballista/contributors-guide/shuffle.html) · [Adaptive scheduler](https://datafusion.apache.org/ballista/contributors-guide/architecture.html)

The proposed Sirius counterpart is a spooled exchange. A surviving spool can feed another
attempt. A lost spool raises a harder question if a downstream stage has already used its
contents. [Retrying a stage instance](../part-5/07-failure-cancellation-and-recovery.md#retrying-a-stage-instance)

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

In the proposed API the frontend chooses the exchanges. The Sirius validator checks the properties each consumer requires.

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
partition count. Round-robin shuffles raise a separate replay question. A different input order on retry can send rows to different partitions. For a range partitioning, Spark samples the data and computes the boundaries on the
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
for broadcast joins; for partitioned joins the coordinator collects and distributes the filters. It can also use filters during split enumeration. Execution is pipelined by default. With
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
  needs; see [Plans from other systems](../part-4/05-plans-from-other-systems.md#a-multi-fragment-plan-from-axiom).
- **Bolt.** [Bolt](https://github.com/bytedance/bolt) is ByteDance's execution library, initially
  derived from Velox, also without an optimizer or scheduler. Its main use is inside Spark tasks;
  see [Plans from other systems](../part-4/05-plans-from-other-systems.md#one-task-at-a-time-bolt-under-spark).
- **Apache Gluten.** [Gluten](https://github.com/apache/gluten) is "a middle layer responsible for
  offloading JVM-based SQL engines' execution to native engines". For Spark it replaces supported
  operators of the physical plan with native ones, much as Comet does, and runs them on a backend
  such as Velox or Bolt, while Spark keeps the stages, the shuffle and the scheduler. The native
  side must still route rows exactly as Spark would, the contract [Comet](#comet) keeps with its
  native shuffle.



## What Sirius borrows

- **Input requirements.** Suppose an inner equijoin matches `l_orderkey` to `o_orderkey`.
  Either key can describe the partitioning of matched rows. After a left outer join, an
  unmatched line item still belongs to its original partition even though its `o_orderkey` is
  null. A planner reusing a distribution must account for the join type.
  [Equivalence properties](../reference/open-questions.md#27-requirements-as-data-and-equivalence-properties)
- **Aggregate states.** DataFusion's accumulator declares state fields and knows how to merge
  them. A distributed average, for example, must preserve enough information to weight each
  partial result. Sirius proposes naming the state type and its merge and final functions so
  a receiving worker can check their identity. [State types](../reference/node-catalog.md#aggregate-state-types)
- **Refinable filters.** A top-N bound can become tighter as better rows arrive. A scan can
  use an older bound safely if it only prunes rows that cannot enter the result. Supporting
  several publications of one value would need a rule for which updates a consumer can use.
  [Refinable values](../reference/extension-points.md#additive-later)

## Where Sirius differs

In a cross-worker hash join, “four partitions” is insufficient. Both senders must agree on how
to encode keys, hash them and assign partitions to workers. Naming a scheme gives the peers
something to compare. Independently built implementations also need a specification precise
enough to compute the same result. [Hash scheme stability](../reference/open-questions.md#18-hash-scheme-stability)

The equivalent issue for a runtime filter is coverage. A filter built from one quarter of the
orders cannot reject line items on the other three workers until the necessary shares have
been combined. Naming the producer, key and scope lets a consumer check what it can infer.
[Runtime filter scope](../part-2/05-runtime-filters.md#scope-which-rows-does-a-filter-know-about)

An open fragment only describes its own side of a boundary. Comparing signatures can detect
a schema or routing mismatch on that edge. Questions about the whole query, such as whether
a filter removes rows an outer join should preserve, require more information.
[Open-plan composition](../reference/open-questions.md#9-open-plan-composition)

## Positioning: the layer under a planner

The proposed `prepare` takes a plan whose distribution choices are already recorded. A
frontend can use cost estimates to choose an exchange, or supply alternatives and a rule for
choosing among them during execution. This places a clear limit on adaptivity. A worker can
choose a listed broadcast join after observing a small build, but choosing a new join order
would require a new plan. [Re-planning beyond a plan's alternatives](../reference/open-questions.md#33-re-planning-beyond-the-plans-alternatives)
