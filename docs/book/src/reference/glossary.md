# Glossary

The book uses each term below in one sense. Where a word means something else inside the engine,
the entry says so. The [terminology rules](#terminology-rules) list the words the book avoids and
what it writes instead.

## Plan and API terms

| Term | Definition | See |
|---|---|---|
| **Adaptive exchange** | An `Exchange` built with `Exchange::decided_by`: one alternative per label of a Decision, all with the same target set. The `ExecutionReport` records the chosen label. | [Adaptive plans](../part-2/06-adaptive-plans.md) |
| **Admission key** | (priority, `ExecutionId`): the order in which every process admits executions under gang admission. Highest priority first, then by execution id. | [Running at scale](../part-5/08-running-at-scale.md#admission-across-processes) |
| **Aggregate phase** | *single* (values to values), *partial* (values to states), *combine* (states to states) or *final* (states to values). | [Node catalog](node-catalog.md) |
| **Arm** | One labelled subtree of a `Choose`. Every arm is prepared; only the arm its Decision chose runs. | [Adaptive plans](../part-2/06-adaptive-plans.md#choosing-between-subtrees) |
| **Assignment** | A required, global runtime value that maps each partition of an over-partitioned hash exchange to a worker. `AssignPartitions` computes it from a global histogram of partition sizes; `Partitions::assigned_by` uses it. Value ids use the letter `a`. | [Running at scale](../part-5/08-running-at-scale.md#skew-more-partitions-than-workers) |
| **Attempt** | One run of a stage instance. The first is attempt 0; a retry runs the same plan as attempt n+1. It is part of every *message key*, and receivers ignore messages from attempts they do not consume. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#retrying-a-stage-instance) |
| **Auto (property)** | A node property set to `auto`, such as `BuildSide::auto_` (Rust `BuildSide::Auto`). Allowed only for worker-local properties that never change what another worker receives: the build side of a `co_partitioned` or `single` join, and an algorithm variant if a node offers one (none does yet). Prepare resolves it deterministically, and the explain prints the result (`build=auto->right`). | [Preparing a plan](../part-1/06-prepare-and-execute.md#preparing-a-plan), [Node catalog](node-catalog.md#join) |
| **Binding** | The prepare-time map of logical workers to endpoints and of named inputs to input sources (see *completed input*). It says *where*, never *what*, so it never changes the plan. | [Stages and bindings](../part-2/04-stages-and-bindings.md) |
| **Binding epoch** | A counter on the bindings: 0 when an execution starts, plus one for every retry the execution applies. The bindings digest covers it. Not the reserved *epoch* of a message key. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#binding-epochs) |
| **Boundaries** | A required runtime value: the \|T\|-1 splitters of a range exchange to T. Intervals are half-open in sort order, so a key equal to a splitter goes to the higher partition. `ComputeBoundaries` computes it from a global key sample, weighting each worker's sample by the rows that worker saw. | [Sort, top-N and limit](../part-3/06-sort-top-n-and-limit.md) |
| **Bounded wait** | `wait=up_to(t)` on a `RuntimeFilterApply`: the apply holds its input for at most t while it waits for an optional filter, then continues with the filter state it has. It never fails the execution or changes results, and the filter stays optional. | [Runtime filters](../part-2/05-runtime-filters.md#waiting-or-not) |
| **Choose** | A node that passes on the rows of one of several arms, picked by the label of a Decision. The arms have the same schema, the stage graph is the union of their stages, and downstream nodes see the meet of the arms (V26). | [Adaptive plans](../part-2/06-adaptive-plans.md#choosing-between-subtrees) |
| **Close** | The stop signal from a receiver to its senders. A satisfied `Limit` over unsorted input closes its input; across an exchange, close goes upstream to every sender. A closed sender still sends end of stream. See *outcome*. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#close-is-not-end-of-stream) |
| **Closed (outcome)** | See *outcome*. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#how-an-execution-ends) |
| **Closed plan** | A plan that contains both ends of every exchange and value edge. See *open plan*. | [Stages and bindings](../part-2/04-stages-and-bindings.md) |
| **Co-partitioned** | Two edges whose partitioning signatures are equal (see *partitioning signature* for the one exception) and whose keys match positionally. A join over co-partitioned inputs needs no further exchange. | [Exchanges](../part-2/03-exchanges.md) |
| **Completed input** | An `InputSource` made with `InputSource::completed`: every batch is supplied before the execution starts, and the producer side is sealed. Live channels come later. | [API at a glance](api-at-a-glance.md#completed-inputs) |
| **Completion state** | How a value producer ends on one worker, exactly once: `published`, `skipped`, `failed` or `cancelled`. | [Runtime filters](../part-2/05-runtime-filters.md) |
| **Context** | The public `Context` type only: a handle to the process's one engine. Plans are checked and prepared on it. See *runtime (engine)*. | [The context](../part-1/03-context-and-session.md) |
| **Data edge** | An edge that carries batches with a schema, then an end of stream from each sender. | [Node catalog](node-catalog.md) |
| **Data file** | One file of a table source, written `DataFile`: a URI and, optionally, its size in bytes. A *sized file* declares its size, a plan fact the fingerprint covers; `Shards::byte_ranges` needs every file sized. Unrelated to a *sized worker set*. | [Workers and distribution](../part-2/02-workers-and-distribution.md#sized-files) |
| **Decision** | A required runtime value that names one label out of a fixed list. A `Decide` node produces it from global statistics with a pure policy (`threshold` or `partition_count`). | [Adaptive plans](../part-2/06-adaptive-plans.md) |
| **Determinism class** | `deterministic`, `per_execution` (bound to a literal when the plan is built, such as `now()`) or `volatile` (such as `random()`). Every function and node has one; a `volatile` node over a `replicated` input makes it `arbitrary`. | [Extension points](extension-points.md#determinism-classes) |
| **Dialect** | A host's own semantics for functions, types, overflow and aggregate state layouts, such as Spark's or Presto's. A plan speaks a dialect through translator casts and through functions and state types registered in the host's namespace (`spark.hash`). | [Plans from other systems](../part-4/05-plans-from-other-systems.md#dialect-semantics) |
| **Distribution** | The derived property that says how a node's rows are spread over its worker set: `single`, `arbitrary`, `hashed`, `replicated`, `ranged` or `decided`. | [Workers and distribution](../part-2/02-workers-and-distribution.md) |
| **Durability** | A property of every exchange. `pipelined` (the default): rows flow as they are produced. `spooled`: the sender's output is retained until the execution ends, and a receiver consumes an attempt's output only after that attempt completed. Printed `durability=spooled`; part of the edge signature. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#durability-pipelined-or-spooled) |
| **Dynamic shards** | The shard kind `Shards::dynamic()`, printed `shards=dynamic`: the workers of a sized set claim a scan's splits at run time, exactly once each. The output is `arbitrary(W)`; the report and the resolved plan record the claims. | [Running at scale](../part-5/08-running-at-scale.md#dynamic-splits) |
| **Edge signature** | The record both sides of an exchange or value edge compare at start in an open plan: id, execution id, mode and parameters, schema, hash scheme, partition count (and assignment), ordered destinations, the sender set (name and size) and durability, plus the IR version, the wire version and the identities of everything on the edge. It never includes the membership of the side that only sends. | [Stages and bindings](../part-2/04-stages-and-bindings.md) |
| **Endpoint** | A (process, device) pair that workers are bound to. Opaque, comparable, and shareable between processes as bytes. | [Stages and bindings](../part-2/04-stages-and-bindings.md) |
| **EOS** | End of stream. Every sender sends it to every receiver of an exchange, even after zero rows and even after the receiver closed. A receiver is complete after one EOS from each sender. | [Exchanges](../part-2/03-exchanges.md) |
| **Epoch** | A reserved field in every message key, end of stream and value completion, always 0 today, so iteration and streaming can come later. Not the *binding epoch*. | [Extension points](extension-points.md#reserved-now-epochs) |
| **Exchange** | A node that moves rows between workers. Its **mode** is gather, hash, broadcast, range, round_robin, forward or decided_by. | [Exchanges](../part-2/03-exchanges.md) |
| **Exchange summary** | The per-exchange report record from `report.exchange_summaries()`: rows, bytes, quantiles of the bytes per receiver and per sender-receiver pair, and the heaviest receivers. | [Running at scale](../part-5/08-running-at-scale.md#observability-at-scale) |
| **Execution** | One run of a `PreparedPlan`, identified by an `ExecutionId`, which is single use per hosted worker. It ends with an `ExecutionReport` from `wait()`, or with an error. | [Prepare and execute](../part-1/06-prepare-and-execute.md) |
| **Expression** | A typed scalar bound to the relation its columns came from. Built from `relation.column(name)`, literals and combinators. | [Types, schemas and expressions](../part-1/04-schemas-and-expressions.md) |
| **Extension point** | A list the Plan IR can grow, such as functions, data types, value kinds or hash schemes. Each list is closed (new members only in a new IR version) or open for registration. | [Extension points](extension-points.md#closed-and-open-lists) |
| **Extension type** | A registered data type: a name plus a storage type. Hashing, comparison and the wire use the storage type. | [Extension points](extension-points.md#closed-and-open-lists) |
| **Fail-stop** | The default failure behavior: any failure fails the whole execution. Failure travels along edges in both directions; the origin ends `failed` and every instance it stops ends `cancelled`. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#fail-stop-along-the-edges) |
| **Fingerprint** | A hash of the plan's canonical form, covering the IR version and the identity of every registered item the plan uses. Processes running the same closed plan compare fingerprints at `start`. | [Stages and bindings](../part-2/04-stages-and-bindings.md) |
| **First-contact deadline** | A prepare option, `PrepareOptions::with_first_contact_deadline`: after `start`, every peer of an edge must answer the handshake within it, or the edge fails with `transport`. Always finite; 60 seconds by default. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#the-first-contact-deadline) |
| **Gang admission** | Admitting an execution on every process that hosts one of its workers, or on none, in *admission key* order, so admission cannot deadlock across processes. | [Running at scale](../part-5/08-running-at-scale.md#admission-across-processes) |
| **Grouping id** | The `int32` key column that an `Aggregate` with grouping sets adds after its keys: the 0-based position of the row's set in the plan's list. Named `grouping_id` unless renamed. | [Aggregates, joins and ordering](../part-1/07-aggregates-joins-and-ordering.md#grouping-sets) |
| **Grouping sets** | A parameter of a single or partial `Aggregate`: an explicit list of subsets of its keys. Every input row is aggregated once per set, and the keys outside a row's set are null. The builder helpers `rollup` and `cube` expand to the list. | [Aggregates, joins and ordering](../part-1/07-aggregates-joins-and-ordering.md#grouping-sets) |
| **Host** | An application that embeds Sirius and builds plans: sirius-duckdb, StarRocks compute nodes, Distributed Sirius, and, in Plans from other systems, an Axiom runner or a Spark executor running Bolt. | [One IR, three integrations](../part-4/04-one-ir-three-integrations.md) |
| **Host-carried edge** | An edge between two plans that the host moves, from an `Output` in one plan to an `Input` in another, such as a Spark shuffle between two Spark tasks. Sirius sees no edge, so there is no handshake and no edge signature; the host's tasks are the parties that must agree. | [Inside Spark tasks: Bolt](../part-2/04-stages-and-bindings.md#host-carried-edges) |
| **Identity** | (namespace, name, signature, version): what names a function, state type, Decide policy, hash scheme, scan kind, value kind or registered node. The fingerprint and the edge signature cover identities. | [Extension points](extension-points.md#versioning) |
| **Instance plan** | A plan in instance form: one worker's view, written `instance frag1#2`. Produced by `rewrite::instance` from a split plan, or authored directly with `Plan::create_instance`; both print the same way. | [Stages and bindings](../part-2/04-stages-and-bindings.md) |
| **Integration level** | How much of a query Sirius sees in a host: the whole query as one closed plan, open instance plans under a foreign scheduler, one single-worker plan per host task, operator-level offload, or the data level of Arrow results and registered scans. | [One IR, three integrations](../part-4/04-one-ir-three-integrations.md#beyond-three-hosts) |
| **IR version** | One version number for the node vocabulary and its semantics, covered by the fingerprint and every edge signature. | [Extension points](extension-points.md#versioning) |
| **Label** | The name of one alternative of a Decision, such as `broadcast` or `partitioned`. | [Adaptive plans](../part-2/06-adaptive-plans.md) |
| **Logical bytes** | The unit of `Metric::bytes`, computed from Arrow type widths: width times rows for fixed-width columns, plus the payload lengths of strings and lists. Never encoded, compressed or device bytes, so every engine computes the same number for the same rows. | [Adaptivity in the engine](../part-5/04-adaptivity-in-the-engine.md#every-worker-computes-the-same-label) |
| **Logical edge** | What the plan fixes about an exchange or value edge: mode, keys, hash scheme, partition count, null routing, target and sender sets, and for a value edge its kind, merge rule and wait policy. It decides what each receiving worker ends up with. See *physical shape*. | [Everything in the plan](../part-2/01-everything-in-the-plan.md#the-logical-edge-and-its-physical-shape) |
| **Lowering** | The engine's translation of a stage into engine pipelines. Unlike a rewrite, it does not produce a plan. | [From plan to pipelines](../part-5/01-from-plan-to-pipelines.md) |
| **Meet** | The weakest guarantee shared by the alternatives of a decided distribution. Downstream checks use it. | [Adaptive plans](../part-2/06-adaptive-plans.md) |
| **Message key** | `(execution id, attempt, epoch, edge id, sender, receiver, sequence)`. The execution id, edge id, sender and receiver name a stream; the sequence names a message within it. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#every-message-has-a-key) |
| **Node** | An element of the Sirius Plan IR, written in PascalCase (`RuntimeFilterBuild`). | [Node catalog](node-catalog.md) |
| **Node contract** | The ten facts every node declares, built in or registered: schema rule, required input distributions and orderings, output distribution and ordering, key preservation, value edges, input contract, determinism class and side-effect class. Missing declarations fail closed. | [Extension points](extension-points.md#the-node-contract) |
| **Node view** | The read-only accessors `Plan::terminals()`, `Relation::kind()` and `Relation::children()`, which show a plan's nodes and data edges. Node parameters cannot be read back yet. | [API at a glance](api-at-a-glance.md#node-view) |
| **Null-safe key pair** | A join key pair that treats two nulls as equal, built with `JoinKey::null_safe` and printed `<=>`. A plain pair, printed `==`, matches no null. | [Aggregates, joins and ordering](../part-1/07-aggregates-joins-and-ordering.md#null-safe-keys) |
| **Open plan** | A plan where the far side of at least one exchange or value edge is not in the plan. A host that owns scheduling, such as StarRocks, authors one open instance plan per fragment instance. | [Stages and bindings](../part-2/04-stages-and-bindings.md) |
| **Optional value** | A runtime filter. A filter that is missing, skipped or late lets every row through, so results never depend on it. | [Runtime filters](../part-2/05-runtime-filters.md) |
| **Outcome** | How a stage instance or an execution ends, exactly once: `succeeded`, `closed`, `failed` or `cancelled`. A stage instance ends `closed` when every receiver of its output closed early, such as after a satisfied `Limit`, or when it belongs only to arms of a `Choose` that were not chosen. An execution ends `closed` when the host closed every output. `closed` is not a failure: `wait()` returns the report. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#how-an-execution-ends) |
| **Over-partitioning** | A hash exchange with more partitions than target workers, P > \|T\|, so each worker holds several partitions and an *assignment* can balance them. | [Running at scale](../part-5/08-running-at-scale.md#skew-more-partitions-than-workers) |
| **Ownership rule** | The plan owns every fact two workers must agree on, and every distribution or ordering property later nodes rely on. The engine owns how each edge is carried out, and any choice the plan leaves open, such as an `auto` property. The test: what each receiving worker ends up with is in the plan; the route the bytes take is not. | [Everything in the plan](../part-2/01-everything-in-the-plan.md) |
| **Partition** | Only "partition p of exchange xK": one hash or range destination of an exchange. Splitting rows inside one worker is *local sub-partitioning*. | [Exchanges](../part-2/03-exchanges.md) |
| **Partitioning signature** | The tuple (HashScheme including NullRouting, key types, partition count or decision, assignment if any, worker set) that defines a hashed distribution. Co-partitioning needs equal signatures, except that a `mark` join's build may differ in NullRouting alone (`to_all` against a `hashed` probe, V10). A null-safe key pair adds nothing to it. | [Node catalog](node-catalog.md) |
| **Peers** | Rows of one window group with equal order keys. `rank` and `dense_rank` give peers one value; which peer comes first for `row_number` and a running measure is the engine's choice. | [Node catalog](node-catalog.md#window) |
| **Per-key top-N** | A `TopN` with keys: the top n rows of each key value within each worker. With n = 1 it expresses `DISTINCT ON` and a grouped `first()`. | [Aggregates, joins and ordering](../part-1/07-aggregates-joins-and-ordering.md#top-n-per-key) |
| **Physical shape** | How the engine carries out a logical edge, such as its transport, flow control and credits, a relay broadcast or a two-level shuffle. It never changes what a receiver ends up with, so it never appears in the plan. | [Everything in the plan](../part-2/01-everything-in-the-plan.md#the-logical-edge-and-its-physical-shape), [Exchanges in the engine](../part-5/02-exchanges-in-the-engine.md#the-logical-edge-and-its-physical-shape) |
| **Plan** | The Sirius Plan IR: the only plan representation, an immutable DAG with data and value edges. Unqualified "plan" always means this. | [Relations and plans](../part-1/05-relations-and-plans.md) |
| **Plan forms** | *joined* (Exchange nodes), *split* (sink and source pairs per stage), *instance* (one worker's view) and *resolved* (decisions, assignments and dynamic split claims substituted). All are the same IR. | [Plan notation](plan-notation.md) |
| **PreparedPlan** | A plan bound to its bindings and this process's resources by `Context::prepare`, with every `auto` property resolved and nothing rewritten. Complete or an error; single use, consumed by `start()` or `collect()`. | [Prepare and execute](../part-1/06-prepare-and-execute.md) |
| **Registry** | An immutable `Registry` in which plan construction resolves names: functions, aggregate state types, `Decide` and `AssignPartitions` policies, hash schemes, scan kinds, value kinds and registered nodes. It needs no `Context`. `Registry::builtin()` is the default, and `Registry::builder()` extends it. An unknown name fails construction with `invalid_plan`; `check` and `prepare` check whether the engine implements it (V21, V22). | [Extension points](extension-points.md#closed-and-open-lists) |
| **Relation** | The builder handle for a node and everything below it. Immutable and complete, with its schema and distribution. | [Relations and plans](../part-1/05-relations-and-plans.md) |
| **Repeat** | A node that emits each input row as many times as an integer column of the row says, and drops that column. `INTERSECT ALL` and `EXCEPT ALL` count rows with an aggregate and end in a `Repeat`. | [Aggregates, joins and ordering](../part-1/07-aggregates-joins-and-ordering.md#set-operations) |
| **Required value** | Statistics, Boundaries, a Decision or an Assignment. Its consumer waits for it, and a required value that fails or is skipped fails the execution. A filter is never required, even under a bounded wait. | [Runtime filters](../part-2/05-runtime-filters.md) |
| **Requirement** | A value a node declares per input, saying which distributions or orderings it accepts: any, not replicated, `single(W)`, hashed on a subset of keys, co-partitioned pair, replicated build, `sorted(keys)`, global value. An unmet requirement fails construction. | [Extension points](extension-points.md#requirements-are-data) |
| **Retryable** | A failed stage instance whose inputs are scans or spooled exchanges, whose outputs are spooled, whose values would re-publish the same shares, whose nodes are `none` or `idempotent`, and which closed no spooled input. The host decides whether to retry it. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#retrying-a-stage-instance) |
| **Rewrite** | A plan-to-plan function in `sirius::rewrite`. Its result is a plan in the same IR that you can print, diff and execute. Preparation never applies one; a host calls each rewrite it wants before preparing. | [Plan-to-plan rewrites](../part-5/05-plan-to-plan-rewrites.md) |
| **Runtime (engine)** | The internal runtime behind the engine, not a public type: one reference-counted object per process that owns every GPU-bound resource, retained by every public handle and every task in flight. Unrelated to runtime values and runtime filters. See *unavailable (engine)*. | [The context](../part-1/03-context-and-session.md#the-context-your-handle-to-the-engine) |
| **Runtime filter** | A Filter value built from a join's build side and applied on its probe side to drop rows that cannot match. The plan and API term. | [Runtime filters](../part-2/05-runtime-filters.md) |
| **Scope (of an expression)** | The relation an expression's columns belong to. An expression can be used only on nodes over that relation; `project` creates a new scope. | [Types, schemas and expressions](../part-1/04-schemas-and-expressions.md) |
| **Scope (of a value)** | `partial` (this worker's rows), `partition(xK)` (the rows of this worker's partitions of exchange xK) or `global` (all rows). | [Runtime filters](../part-2/05-runtime-filters.md) |
| **Sequence number** | The last field of a message key. It counts from 0 per stream and attempt, and end of stream carries one too, so a receiver drops duplicates and detects gaps. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#every-message-has-a-key) |
| **Session** | Not part of the API (see *context*). A `Session` may arrive later as a *planning environment*, with a catalog and settings, for catalog-backed or SQL preparation. | [Open question 15](open-questions.md#15-one-context-per-process) |
| **Shard** | The part of a source one worker reads, chosen by a `Shards` kind. Every shard is a fixed list of splits stored in the plan when the scan is built (by reference, over a split manifest), except under *dynamic shards*. | [Workers and distribution](../part-2/02-workers-and-distribution.md) |
| **Side-effect class** | `none`, `idempotent` or `effectful`. Every node has one. Retry re-runs only `none` and `idempotent` nodes, and an `effectful` node may not be shared. | [Extension points](extension-points.md#side-effect-classes) |
| **Single-node multi-GPU (SNMG)** | One process driving several GPUs of one machine. Not a mode: a plan on `compute[N]` bound with `Bindings::local()`, where every move between GPUs is still an `Exchange`. | [Embedded in a host: sirius-duckdb](../part-4/01-duckdb-extension.md#inside-one-machine) |
| **Single-worker plan** | A joined plan on `local[1]`, the implicit one-worker set. | [Relations and plans](../part-1/05-relations-and-plans.md) |
| **Sirius Plan IR** | See *plan*. | |
| **sirius-duckdb** | The DuckDB extension that embeds Sirius in DuckDB. | [Embedded in a host: sirius-duckdb](../part-4/01-duckdb-extension.md) |
| **Sized worker set** | A worker set with a size in the plan, required wherever the plan depends on that size (V24). See *worker set*. | [Workers and distribution](../part-2/02-workers-and-distribution.md) |
| **Source check** | The comparison of a scanned file with what the plan declares for it: column names and types, a declared size, no nulls in a `required` column. It always runs when a worker opens the file and, with `SourceCheck::at_prepare`, also at `prepare`. A mismatch is `source_mismatch`. | [Relations and plans](../part-1/05-relations-and-plans.md#the-source-check) |
| **Split** | The unit of scan work: a file, optionally with a byte range. Splits are assigned to workers when the plan is built, never at prepare, except under *dynamic shards*. Not the *split form* of a plan. | [Workers and distribution](../part-2/02-workers-and-distribution.md) |
| **Split manifest** | An immutable, content-addressed list of splits, made with `SplitManifest::create`. A scan over it stores its shard rule and the manifest's digest, and the fingerprint covers the digest. | [Running at scale](../part-5/08-running-at-scale.md#split-manifests) |
| **SPMD** | Every process runs the same builder code, gets the same plan and fingerprint, and prepares it with the same bindings. | [Sirius on its own: Distributed Sirius](../part-4/03-distributed-sirius.md) |
| **Stage** | A connected component of the plan after cutting at every exchange. Derived, never declared. | [Stages and bindings](../part-2/04-stages-and-bindings.md) |
| **Stage instance** | A stage on one worker. | [Stages and bindings](../part-2/04-stages-and-bindings.md) |
| **State type** | The `DataType` of a partial aggregate's measure column, such as `sum_state<decimal(38,2)>`, with a fixed layout. | [Node catalog](node-catalog.md) |
| **Statistics** | A required runtime value with row counts, logical byte counts, a key sample or a histogram of logical bytes per partition, produced by `Observe`. | [Adaptive plans](../part-2/06-adaptive-plans.md) |
| **Truncated** | A flag on a `Statistics` value published by an `Observe` whose stage closed early: the value covers only the rows seen before the close. A merged value is truncated if any share is, and the report records the flag. | [Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#close-is-not-end-of-stream) |
| **Unavailable (engine)** | The state of an engine after a fatal device error or GPU work whose completion could not be confirmed. It lasts for the whole process: every call on any handle returns `runtime_unavailable`, unlike `execution`, which fails one query. Nothing is recovered in place or replayed on the CPU. An unavailable engine whose last owner has let go is *abandoned*. [Errors](api-at-a-glance.md#errors) says what `Context::create` returns before and after that. | [The context](../part-1/03-context-and-session.md#how-long-a-context-lives), [Prepare and execute](../part-1/06-prepare-and-execute.md#errors-by-phase) |
| **Unsized worker set** | A worker set without a size, made with `WorkerSet::unsized`. Allowed only for the hosting set of an instance plan, and only when nothing in that plan depends on the set's size. | [Workers and distribution](../part-2/02-workers-and-distribution.md) |
| **Value edge** | An edge that carries one small runtime value per producing worker: a Filter, Statistics, Boundaries, a Decision or an Assignment. Drawn dotted. | [Runtime filters](../part-2/05-runtime-filters.md) |
| **Velox plan adapter** | `sirius::velox`, a C++ library on top of the builder API that turns Velox plan nodes, or Axiom's multi-fragment plan, into builder calls and returns an ordinary `Plan`. Not part of the core API and not a second plan representation. | [Plans from other systems](../part-4/05-plans-from-other-systems.md#the-velox-plan-adapter) |
| **Wait-for graph** | The data and value edges plus the ordered waits (a required value before rows, a join's build before its probe, every sender before a merge step) and the credit waits into streaming queues. V1 requires it to be acyclic. | [Running at scale](../part-5/08-running-at-scale.md#credits-and-deadlock-freedom) |
| **Window** | A node that keeps every input row and adds one column per function, computed within the row's window group. Its form is `whole`, `ranking` or `running`. | [Aggregates, joins and ordering](../part-1/07-aggregates-joins-and-ordering.md#window) |
| **Window group** | The rows of a `Window`'s input that share the same values of its window keys (SQL's `PARTITION BY`); all rows when it has no keys. Every row of a group must be on one worker (V27). | [Exchanges](../part-2/03-exchanges.md#windows-need-whole-groups) |
| **Wire version** | One version number for the message format between engines, carried in every edge signature. | [Extension points](extension-points.md#versioning) |
| **Worker** | A logical execution slot bound to exactly one GPU, written `compute#2`. | [Workers and distribution](../part-2/02-workers-and-distribution.md) |
| **Worker set** | A named set of workers, written `compute[4]` when sized and `frag1[?]` when unsized. `local[1]` is the default. | [Workers and distribution](../part-2/02-workers-and-distribution.md) |

## Engine terms

These terms name engine internals, which Part V and the "What the engine does" sections of Part III
describe.

| Term | Definition |
|---|---|
| **Barrier** | The kind of an edge between pipelines: `FULL` (the consumer needs the complete input), `PARTIAL`, or `PIPELINE` (inside one pipeline). |
| **Barrier queue** | A queue whose consumer may wait for something else first: the router of an exchange that needs a required value, a join's build, a join's probe while its build is incomplete, and each per-sender queue of a gather that reads its senders in a fixed order (`merge` or `by_worker`). It accepts everything and spills, so credits never bound it. See *streaming queue*. |
| **cuCascade** | The tiered memory library under the engine, with GPU, pinned host and disk tiers and memory reservations. |
| **Downgrade executor** | The engine service that moves data down the memory tiers under memory pressure. Inside a process, the engine spills instead of applying backpressure. |
| **Engine operator** | An engine construct, written `UPPER_CASE` in code font (`HASH_GROUP_BY`). One IR node can lower to several operators. |
| **Pipeline** | A chain of engine operators that runs batch by batch, from a source to a sink. |
| **Port** | A named, repository-backed input of an engine operator, such as `default` or `build`. |
| **Publication session** | The hash join's `dynamic_filter_publication_session`, which builds and publishes a runtime filter today. |
| **Repository** | A spillable queue of batches between pipelines, keyed by operator and port. |
| **Spool** | The retained output of a sender on a `spooled` exchange: a send repository that keeps its batches after they are sent, until the execution ends. |
| **Stream session** | `exec::stream_session`, the engine's id-to-operator router for the streaming inputs and outputs of one `exec::streaming_fragment`. Not the later planning `Session`. |
| **Streaming queue** | A queue drained by a consumer that waits for nothing else. Receiver credits may bound it. See *barrier queue*. |
| **Task** | A pipeline plus one input, scheduled onto a GPU. |

## Other systems

What the book means by each system that Part IV and the appendix name. The repositories linked
from those chapters are the authority.

| Term | Definition | See |
|---|---|---|
| **Apache Doris** | An analytical database whose frontend plans a query into fragments and schedules fragment instances onto backends, as StarRocks does. What the book says about StarRocks applies to it. | [Under someone else's scheduler](../part-4/02-starrocks.md) |
| **Apache Gluten** | A middle layer that offloads the execution of JVM-based SQL engines, Spark in this book, to native engines such as Velox and Bolt. It builds the native engine's plan nodes from Spark's physical plan, one task at a time. | [Plans from other systems](../part-4/05-plans-from-other-systems.md#one-task-at-a-time-bolt-under-spark) |
| **Axiom** | Meta's set of C++ components for building front ends to Velox: Presto SQL and PySpark front ends, a logical plan, a cost-based optimizer, connectors and runners. Its optimizer produces a multi-fragment plan of Velox plan nodes. | [Plans from other systems](../part-4/05-plans-from-other-systems.md#velox-plans) |
| **Bolt** | ByteDance's C++ execution library, initially derived from Velox. It has no optimizer or scheduler; its main use is as a native backend for Spark through Apache Gluten. | [Plans from other systems](../part-4/05-plans-from-other-systems.md#one-task-at-a-time-bolt-under-spark) |
| **Fragment (host)** | A host's unit of planning between exchanges: a StarRocks fragment, an Axiom `ExecutableFragment`, a Velox or Bolt plan fragment. It corresponds to a stage in the Plan IR. | [Plans from other systems](../part-4/05-plans-from-other-systems.md#a-multi-fragment-plan-from-axiom) |
| **Substrait** | A cross-language specification for relational plans: relations over typed expressions, with functions named by extension. A converter turns a Substrait plan into builder calls; it is not a second entry point. | [Plans from other systems](../part-4/05-plans-from-other-systems.md#substrait) |
| **Velox** | Meta's C++ execution library: vectors, plan nodes, tasks, drivers and operators. It leaves SQL planning, optimization and scheduling to the systems built on it. Its experimental cuDF backend replaces operators with GPU ones. | [Related designs](../appendix/related-designs.md#the-velox-family) |

## Terminology rules

- **Logical and physical plan.** The IR is never a "logical plan" or a "physical plan". The book
  writes *single-worker plan*, *joined plan* or *engine pipelines*. A host's own plan is "the
  host's optimized plan", and another system's logical or physical plan keeps its name, such as
  Axiom's logical plan or Spark's physical plan. *Logical edge* and *physical shape* describe one
  edge, never a plan. Upstream's issue titles and the C headers of its plan builder call the IR a
  *physical plan*.
- **Fragment.** Only a host's own fragment in Part IV (StarRocks, Axiom, Velox and Bolt), and the
  engine's `exec::streaming_fragment` in Part V. The IR has stages.
- **Shuffle.** Informal only, or Spark's own shuffle under Bolt. The node is a *hash
  exchange*.
- **Broadcast.** An exchange mode only. For values, the book writes *merge to* or *deliver*.
- **Partition.** Besides exchange destinations, only Spark's own partitions under Bolt.
  The engine operator is always `PARTITION` in code font. A shard or split is never a partition.
  SQL's `PARTITION BY` names a `Window`'s *window keys*, and the rows that share them are a *window
  group*, never a partition.
- **Runtime filter.** *Dynamic filter* appears only next to engine names or as another system's own
  term, such as Axiom's dynamic filter design.
- **Side channel.** Only for today's hidden implementation. In the plan, it is a *dotted value
  edge*.
- **GPU and worker.** The book writes *worker* where it means a worker, although each worker is
  bound to one GPU.
- **Rank.** Never a process or a worker; the book writes *worker*. `rank` in code font is the
  window function.
- **"The engine decides".** Never used for anything the ownership rule assigns to the plan.
- **Context and session.** Unqualified, *context* means only the public `Context` type and
  *session* only the later planning `Session`. The engine's internal sessions are always the
  *stream session* and the *publication session*.
- **Engine and runtime.** A `Context` is never called the engine. The book says *the engine*;
  *the runtime* is its internal name, introduced in [The context](../part-1/03-context-and-session.md#the-context-your-handle-to-the-engine).
- **Node, relation, operator.** Never used for one another. Another system's plan nodes and
  operators keep their own names in code font (`HashJoinNode`).
- **Task.** In Part V, the engine's task. In Part IV, a host's unit of work, such as a Spark task or
  a Velox or Bolt `Task`, named with its system.
