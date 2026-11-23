# Node catalog

The catalog describes the proposed Plan IR. Each entry gives the operator's inputs, output
properties and builder call. For how the current engine executes a stage, see
[From plan to pipelines](../part-5/01-from-plan-to-pipelines.md).

## Node index

The **Inputs** column gives each input's contract. **S** (streaming) means the node works batch by
batch; **B** (blocking) means it needs the complete input on its worker before it produces output.

| Node | Group | Inputs | Created by |
|---|---|---|---|
| [`Scan`](#scan) | leaf | none | `Relation::scan` |
| [`Input`](#input) | leaf | none | `Relation::input` |
| [`Values`](#values) | leaf | none | `Relation::values` |
| [`ExchangeSource`](#exchangesource) | leaf | none | `rewrite::split`, `Relation::exchange_source` |
| [`Output`](#output) | terminal | S | `Relation::output`, `Plan::from_root` |
| [`ExchangeSink`](#exchangesink) | terminal | S | `rewrite::split`, `Relation::exchange_sink` |
| [`ValueSink` / `ValueSource`](#valuesink-and-valuesource) | value terminal / value leaf | value | `rewrite::split`, `RuntimeFilter::value_sink`, `RuntimeFilter::value_source` |
| [`Filter`](#filter) | relational | S | `Relation::filter` |
| [`Project`](#project) | relational | S | `Relation::project` |
| [`Aggregate`](#aggregate) | relational | S (single, partial), B per group (combine, final) | `aggregate`, `aggregate_sets`, `aggregate_partial`, `aggregate_partial_sets`, `aggregate_combine`, `aggregate_final` |
| [`Join`](#join) | relational | probe S, build B (+ value for a co-partitioned `mark` with a plain key pair) | `Relation::join` |
| [`Sort`](#sort) | relational | B | `Relation::sort` |
| [`TopN`](#topn) | relational | B | `Relation::top_n`, `Relation::top_n_per_key` |
| [`Limit`](#limit) | relational | S, or B when the input is sorted | `Relation::limit` |
| [`Window`](#window) | relational | B | `window_whole`, `window_ranking`, `window_running` |
| [`Union`](#union) | relational | k x S | `Relation::union_all` |
| [`Repeat`](#repeat) | relational | S | `Relation::repeat` |
| [`Choose`](#choose) | relational | k x S + value | `Relation::choose` |
| [`Exchange`](#exchange) | distribution | S | `Relation::exchange` |
| [`RuntimeFilterBuild`](#runtimefilterbuild) | runtime value | S | `Relation::build_runtime_filter` |
| [`RuntimeFilterApply`](#runtimefilterapply) | runtime value | S + value | `Relation::apply_runtime_filter` |
| [`ValueExchange`](#valueexchange) | runtime value | values | `RuntimeFilter::merge_to`, `Statistics::merge_to`, `Decision::merge_to`, `Assignment::merge_to` |
| [`Observe`](#observe) | runtime value | S | `Relation::observe` |
| [`ComputeBoundaries`](#computeboundaries) | runtime value | value | `Boundaries::from_sample` |
| [`Decide`](#decide) | runtime value | value | `Decision::threshold`, `Decision::partition_count` |
| [`AssignPartitions`](#assignpartitions) | runtime value | value | `Assignment::balanced` |

## Edges, facts and ids

### Edges

A plan is a directed acyclic graph with two kinds of edges.

- A **data edge** carries batches with a schema, followed by an end of stream (EOS) from each
  sender.
- A **value edge** carries one small runtime value per producing worker. A value has one of five
  kinds: `Filter`, `Statistics`, `Boundaries`, `Decision` or `Assignment`.

In text trees a data edge is the tree structure; a value edge is written `~> f1` on the producer
and `<~ f1` on the consumer. In Mermaid stage graphs, data edges are solid and value edges dotted.

### Node facts

Every node carries:

- a `NodeId`;
- an output schema;
- a `Distribution` on a `WorkerSet` (see [Distributions](#distributions));
- an optional ordering, `sorted(keys)`, which holds *within each worker*;
- for each input, a contract: streaming (S), blocking per group, or blocking (B);
- a determinism class (`deterministic`, `per_execution` or `volatile`) and a side-effect class
  (`none`, `idempotent` or `effectful`).

Data-producing nodes declare these properties. Value producers instead declare their value kind,
scope and completion rules. See the
[node contract](extension-points.md#the-node-contract).

A relation exposes the schema, distribution and set through `schema()`, `distribution()` and
`workers()`, the subtree's classes through `determinism()` and `side_effects()`, and the node's
kind and data inputs through `kind()` and `children()` ([Node view](api-at-a-glance.md#node-view)).

### Ids

`Plan::create` assigns ids in post-order: children before parents, left before right. Identical canonical graphs yield identical ids within the same IR version.

| Thing | Ids | Rule |
|---|---|---|
| Exchanges | `x1`, `x2`, ... | Post-order. |
| Filters | `f1`, `f2`, ... | Numbered per kind among value producers (`RuntimeFilterBuild`, `ValueExchange`, `ValueSource`). |
| Statistics | `s1`, `s2`, ... | Numbered per kind (`Observe`, `ValueExchange`). |
| Boundaries | `b1`, ... | Numbered per kind (`ComputeBoundaries`). |
| Decisions | `d1`, ... | Numbered per kind (`Decide`, `ValueExchange`). |
| Assignments | `a1`, ... | Numbered per kind (`AssignPartitions`, `ValueExchange`). |
| Merged values | next number | A merged value takes the number right after its source: merging `f1` gives `f2`. |
| Stages | `S1`, `S2`, ... | `S<k>` is the stage whose root is `ExchangeSink x<k>`. The `Output` stage is numbered after the last exchange. A stage with several sinks takes its lowest number. |

In joined plans the builder assigns every id. In open plans the host chooses ids with
`ExchangeId::create(n)` and `ValueId::create(n)`
([Stages and bindings](../part-2/04-stages-and-bindings.md)).

## Distributions

A `Distribution` says how a node's rows are spread over the workers of its set W. The validator
derives it for every node and checks it wherever a node depends on where its rows are.

| Distribution | Meaning |
|---|---|
| `single(W)` | All rows on the one worker of W. Requires \|W\| = 1. |
| `arbitrary(W)` | Rows are spread over W with no known rule. This is the bottom: it guarantees nothing. |
| `hashed(keys, sig)` | Each row is on the worker its keys hash to, under the partitioning signature `sig`. |
| `replicated(W)` | Every worker of W holds all rows. |
| `ranged(keys, b)` | Worker i holds the rows whose keys fall in interval i of the boundaries `b` (half-open; see [Exchange](#exchange)). |
| `decided(d, [..])` | One of several distributions, chosen at run time by the Decision `d`. |

**Meet.** Downstream checks treat `decided(d, [..])` as the **meet** of its alternatives: the
weakest guarantee they all share. A choice between `replicated` and `hashed` loses the common placement guarantee. It may
still produce several copies of each row. Treating it as ordinary partitioned input can multiply
the result of a subsequent gather or aggregate. The rule for consuming such a choice is unresolved.
[open question 27](open-questions.md#27-requirements-as-data-and-equivalence-properties).

**Size-1 sets.** On a set of size 1, `arbitrary(W)` and `single(W)` coincide, and checks treat any
non-replicated distribution on it as `single(W)`. So single-phase aggregates and `mode=single`
joins on `local[1]` need no exchange.

**Sized and unsized sets.** A set is usually **sized**, `compute[4]`. The hosting set of an
instance plan may be **unsized**, `frag1[?]`: its worker knows it runs one instance of the stage,
not how many siblings it has. Distributions on an unsized set read as usual
(`arbitrary(frag1[?])`), but the set is never treated as size 1, and anything that depends on the
size needs a sized set (V24).

**Partitioning signature.** `sig` is the tuple (HashScheme including NullRouting, key types,
partition count or decision, assignment if any, W). The key types are the keys' `DataType`s
unchanged, so an exchange on an int32 key and one on an int64 key never have equal signatures. Two
edges are **co-partitioned** when their signatures are equal and their keys match positionally.
One exception: the build exchange of a `mark` join may differ from the probe exchange in
`NullRouting` alone (V10), because non-null keys route the same way under both
([Mark joins need the whole build](../part-2/03-exchanges.md#mark-joins-need-the-whole-build)).
A null-safe key pair changes nothing in the signature: under `NullRouting::hashed` a null key
hashes as the fixed null marker, so equal keys meet on one worker, null or not.

**Hash rule.** Keys are first canonicalized (signed integers become int64, for example).
[Hash canonicalization in v1](extension-points.md#hash-canonicalization-in-v1) lists the form of
every key type; any other type is `unsupported` (V21). Canonicalization is part of `HashScheme`. A
row goes to partition `h_v1(canon(keys)) mod P`, and partition `p` goes to worker `p mod |W|`, or
to the worker an assignment names.

## Leaves and terminals

### Scan

Reads a table source on every worker of a set.

- **Parameters:** source (`TableSource`: a list of files, each a URI with an optional declared
  size; a [split manifest](#split-manifests); or a registered scan kind), columns, shards, an
  optional pushed predicate, optional fused filter applies.
- **Semantics:** each worker reads exactly its shard. Shards cover every split exactly once, or all
  splits on every worker when `replicated`. `Relation::scan` resolves the shard kind when the scan
  is built: the plan stores the list of splits each worker reads (or the rule and a manifest
  digest), and the fingerprint covers them and every declared file size. Nothing reassigns a split
  at run time, except under `shards=dynamic`, which the plan states. A pushed predicate is a
  pruning hint; it is exact only when the `Filter` above it is kept.
- **Files:** a URI is literal: no glob is expanded and no directory is listed. `prepare` resolves a
  relative path against the working directory of the preparing process; the plan and the
  fingerprint hold it as written. Building a scan opens no file.
- **Source check:** when a worker opens a Parquet file, and before it reads a row, the engine
  checks that every scanned column is in the file under its declared name and type, and that a
  sized file has its declared size. A `required` column that holds a null fails when it is read.
  Types are compared as logical types, whatever the file's encoding; no type is converted, and
  unread columns are not checked. A mismatch fails the execution with `source_mismatch`, located
  at the scan. `SourceCheck::at_prepare` also runs the metadata part of the check at `prepare`
  ([Checking sources early](../part-1/06-prepare-and-execute.md#checking-sources-early)). What a
  registered scan kind checks is
  [open question 14](open-questions.md#14-host-registered-scan-kinds).
- **Dynamic shards:** under `Shards::dynamic()` the output is always `arbitrary(W)`. The claims are
  recorded per attempt in `report.split_claims()`, and the resolved plan prints the scan as
  `shards=explicit_splits (resolved from dynamic)`
  ([Dynamic splits](../part-5/08-running-at-scale.md#dynamic-splits)).
- **Distribution:** `arbitrary(W)`, or `replicated(W)` with `Shards::replicated()`; `single(W)` on
  a size-1 set.
- **Side effects:** `none` for Parquet; a registered scan kind declares its own class.
- **Prints as:** `Scan lineitem [l_orderkey, l_shipdate]  on compute[4]  shards=files_round_robin`;
  the `pushed=`, `fused=` and `kind=` continuation lines and the manifest form are in
  [Plan notation](plan-notation.md#node-lines-by-kind), the per-worker lists under
  [Instance form](plan-notation.md#instance-form).

The shard kinds:

| Kind | Rule | Sized set (V24) |
|---|---|---|
| `files_round_robin` | File i to worker i mod n. A builder rule over the source's file list. | required |
| `byte_ranges(target_bytes)` | Files laid end to end in list order and cut at every multiple of `target_bytes`; range j goes to worker j mod n, as one split per file it overlaps, and a row group belongs to the split holding its start. A builder rule over the source's file list and declared sizes: every file must be a [sized file](../part-2/02-workers-and-distribution.md#sized-files), or `Relation::scan` fails with `invalid_plan`. | required |
| `replicated` | All splits on every worker. | not required |
| `explicit_splits(per_worker)` | Each worker reads the list given for it. | required |
| `splits(list)` | The hosting worker reads exactly these splits; covering the table across instances is the host's job. Legal only in an instance plan. | not required |
| `dynamic` | The workers of the set claim the splits at run time, each exactly once. | required |

#### Split manifests

A `SplitManifest` (`SplitManifest::create`) is an immutable, content-addressed list of splits that
a `TableSource` built with `TableSource::from_manifest` carries instead of a file list. A scan over
it stores its shard rule and the manifest's digest, which the fingerprint covers, so every process
with the same manifest derives the same per-worker lists. Over a manifest,
`files_round_robin` deals entry i to worker i mod n, and `byte_ranges` is `invalid_plan`
([Split manifests](../part-5/08-running-at-scale.md#split-manifests)).

### Input

A named stream of batches the application supplies. An input is completed: every batch is supplied
before the execution starts. Live channels, which the application writes while the plan runs, come
later ([Feeding live inputs](../part-1/06-prepare-and-execute.md#feeding-live-inputs)).

- **Parameters:** name, schema, set W.
- **Semantics:** one bound `InputSource` per worker. The input ends after its last batch.
- **Distribution:** `arbitrary(W)`, or `single(W)` on a size-1 set.
- **Prints as:** `Input "recent_orders" on head[1]`

### Values

Inline constant rows.

- **Parameters:** schema, batches, set W.
- **Distribution:** `single(W)`, or `replicated(W)`.

### ExchangeSource

The receiving half of an exchange, in split form or in an open plan.

- **Parameters:** exchange id, exchange description, sender set.
- **Semantics:** ends after an EOS from every sender. `rewrite::split` produces it; open plans
  author it with `Relation::exchange_source`. The sender set is sized, as is the receiving set of
  a partitioned or gathered exchange (V24).
- **Distribution:** follows from the exchange mode (see [Exchange](#exchange)).
- **Prints as:** `ExchangeSource x1 <- S1 senders=[compute#0..3]`; the open-stage form is under
  [Split form](plan-notation.md#split-form), the per-worker forms under
  [Instance form](plan-notation.md#instance-form).

### Output

A named result stream.

- **Parameters:** name.
- **Semantics:** one `OutputReader` per worker of its set, delivered to the process hosting that
  worker: one reader on `head[1]`, four on `compute[4]`. To route results, put an `Exchange` before
  the `Output`
  ([Routing moves into the plan](../part-2/01-everything-in-the-plan.md#routing-moves-into-the-plan));
  preparation only binds a reader to each worker the `Output` names. Dropping a reader before its
  end closes that output; if every output is closed, the execution ends `closed`. Once its first
  batch reaches the host, an `Output` is `effectful` and its stage is never retried.
- **Distribution:** requires its input on its own set.
- **Prints as:** `Output "result" on head[1]`

### ExchangeSink

The sending half of an exchange and the root of a stage.

- **Parameters:** exchange id, exchange description.
- **Semantics:** terminal. Matched to its `ExchangeSource` by exchange id. Its target set is sized
  (V24); its own set may be the unsized hosting set of an instance plan.
- **Prints as:** `ExchangeSink x1 hash(l_orderkey) -> S3 [compute#0..3]`; the open-stage form is
  under [Split form](plan-notation.md#split-form), the per-worker forms under
  [Instance form](plan-notation.md#instance-form) and
  [Authored instance plans](plan-notation.md#authored-instance-plans).

### ValueSink and ValueSource

The split-form halves of a [`ValueExchange`](#valueexchange).

- **Parameters:** value id, mode `merge`, senders or receivers.
- **Semantics:** produced by `rewrite::split`, or authored in open plans with
  `RuntimeFilter::value_sink` and `RuntimeFilter::value_source`. A `ValueSource` merges the shares
  it receives under the merge rule of its kind. The sender set of a `ValueSource` and the
  receiving set of a `ValueSink` are sized (V24).
- **Prints as:** `ValueSink f2 merge -> S1` and `ValueSource f3 = merge f2 <- S3 senders=[compute#0..3]`;
  the open-stage form is under [Split form](plan-notation.md#split-form), the instance form under
  [Authored instance plans](plan-notation.md#authored-instance-plans).

## Relational nodes

### Filter

- **Input:** S. **Parameters:** a boolean predicate in the input's scope.
- **Semantics:** keeps the rows whose predicate is true and drops those where it is false or null
  ([Nulls and NaN](../part-1/04-schemas-and-expressions.md#nulls-and-nan)). The output schema is
  the input schema.
- **Distribution:** preserves ordering and placement. A volatile predicate on replicas can accept different rows on different workers. Downstream multiplicity is unresolved ([open question 27](open-questions.md#27-requirements-as-data-and-equivalence-properties)).

### Project

- **Input:** S. **Parameters:** named expressions; output names are unique. Creates a new scope.
- **Distribution:** `hashed` and `ranged` survive only if every key passes through as a plain
  column; otherwise the result is `arbitrary`. `replicated` survives unless an expression is
  `volatile` ([determinism classes](extension-points.md#determinism-classes)). `sorted(keys)`
  survives if every sort key passes through as a plain column. State types survive only on plain
  columns.

### Aggregate

Grouped or ungrouped aggregation in one of four phases.

| Phase | Builder | Turns | Input contract |
|---|---|---|---|
| single | `aggregate(keys, measures)`, `aggregate_sets(sets, measures)` | values into values | S |
| partial | `aggregate_partial(keys, measures)`, `aggregate_partial_sets(sets, measures)` | values into state columns | S |
| combine | `aggregate_combine()` | states into states | B per group |
| final | `aggregate_final()` | states into values | B per group |

- **Output schema:** keys first, then the grouping id if the aggregate has grouping sets, then one
  column per measure. In partial and combine phases each measure column has a
  [state type](#aggregate-state-types). A combine or final takes exactly the keys plus the state
  columns of one partial signature (V5), and reads every column without a state type as a key.
  Which measure columns are nullable is under [Aggregate state types](#aggregate-state-types).
- **Grouping sets:** a single or partial aggregate may carry a `GroupingSets` value: its keys and
  an explicit list of sets, each a subset of the keys. Every input row is aggregated once per set.
  A row of set i has null in every key outside that set and i in the **grouping id**, where i is
  the 0-based position of the set in the list. The grouping id is an `int32` column that is never
  null, named `grouping_id` unless `GroupingSets::with_id_name` renames it. The list is in the
  plan and the fingerprint covers it, so every worker numbers the sets alike, and two equal sets
  in the list stay apart. A key that some set leaves out is nullable in the output.
  `GroupingSets::rollup` and `GroupingSets::cube` only build the list. SQL's `GROUPING(...)` is
  fixed per set, so a front-end computes it from the grouping id in a `Project` above the
  aggregate.
- **The grouping id is a key.** To a combine or final it is one more column without a state type,
  so it is hashed, grouped and printed like any other key, and the later phases take no
  grouping-set parameter.
- **Partial rows:** a partial or combine emits one or more state rows per group per worker. How
  many is the engine's choice; the final merges them.
- **Ungrouped:** an ungrouped partial emits at least one row per worker (its identity-state row
  when the worker saw no input), and an ungrouped final emits exactly one row, so an empty input
  still produces one result row. A grouping set with no keys, `()`, follows the same rule: the
  result has exactly one row for it.
- **Distribution:** grouped single and final need `single`, or `hashed` on a subset of the keys
  (V6). For a single-phase aggregate with grouping sets, only keys that are in every set count, so
  with a `()` set it needs `single`; for a final, the grouping id counts as a key. Ungrouped
  single and final need `single`. A hash or range property survives only when the output retains
  the distribution keys with the same values. For a partial with grouping sets, those keys must occur in every set.
  A partial grouping by `a` on input hashed by `b` drops `b` and yields `arbitrary(W)`.
  Partial and single reject multi-worker replicated input (V7).
- **Prints as:** `Aggregate partial keys=[l_returnflag] measures=[sum_qty = sum(l_quantity)]`;
  the single phase prints as plain `Aggregate`, and an ungrouped aggregate omits `keys=`. Grouping
  sets print after the keys, `sets=[(l_returnflag, l_linestatus), (l_returnflag), ()]`.

### Join

- **Inputs:** probe S, build B, and for a co-partitioned `mark` join with a plain key pair
  `<~ Statistics`, the build row count (V10). **Parameters:** join type, key pairs (left, right),
  each plain or null-safe, an optional residual built from a `JoinScope`, build side `left`,
  `right` or `auto` (default `right`), and an optional build row count
  (`JoinOptions::with_build_rows`).
- **Key types:** both keys of a pair have the same `DataType` (V4), for decimals the same
  precision and scale; one key may be nullable and the other required. An int32 key with an int64
  key, or `decimal(15,2)` with `decimal(12,2)`, is `invalid_plan`. No cast is inserted: to join on
  keys of different types, cast one side in a `Project` below the join and below its hash exchange.
- **Null-safe pairs:** a plain pair compares with `==`, so a null key matches nothing. A null-safe
  pair (`JoinKey::null_safe`, printed `<=>`) treats two nulls as equal and a null and a value as
  different, so its comparison is never unknown. One join may mix both kinds. A runtime filter
  with a key from a null-safe pair lets null keys pass (V18).
- **Semantics:** a hash join on the key pairs plus the residual; without keys it is a nested-loop
  join. A join never chooses its distribution: its inputs' distributions determine its mode.
- **Build side:** `auto` is allowed only when the mode is `co_partitioned` or `single`, where the
  build side is local to each worker and changes nothing another worker receives. Under
  `broadcast_build` and `decided` the build side follows from the exchanges, so `Plan::create`
  rejects `auto` with `invalid_plan`, a [check without a number](#checked-by-plancreate).
  `prepare` resolves `auto` deterministically from the plan, the engine build and the bound
  devices, with no cost search. The fingerprint covers `auto` as built, so processes may resolve it
  differently. An explicit side the engine cannot honor fails `prepare` with `unsupported`. A
  `mark` join builds on its right input, because its left rows are the probe rows that get the
  mark: `BuildSide::left` and `BuildSide::auto_` fail at the builder call with `invalid_plan`.
- **Join types:** `inner`, `left`, `right`, `full`, `left_semi`, `left_anti`, `right_semi`,
  `right_anti`, `mark`.
- **Output schema:** left fields, then right fields; names must be unique, so rename first if
  needed. `left_semi` and `left_anti` output only left fields, the `right_*` kinds only right
  fields, and `mark` appends a nullable boolean `mark` column. Fields of a side that an outer join
  pads with nulls (the right side of `left`, the left side of `right`, both sides of `full`) are
  nullable in the output.
- **Mark:** the `mark` column follows SQL's `IN`: true when some build row matches, null when
  none matches but some comparison is unknown, and false otherwise. For one plain key pair without a residual, a
  non-matching probe row is null when the whole build holds a null key. A probe row with a null
  key is null when the whole build has any rows, and false when the build is empty. Under
  `co_partitioned` a worker sees only its share of the build, so the join has extra requirements
  (V10; see
  [Mark joins need the whole build](../part-2/03-exchanges.md#mark-joins-need-the-whole-build)).
  The build row count is a required value: the join holds back only its null-key probe rows until
  it arrives, and it adds an edge to the wait-for graph (V1). A null-safe comparison is never
  unknown, so a mark join with only null-safe key pairs and no residual answers only true or false, and V10
  does not apply to it. A mark join that mixes plain and null-safe pairs uses a broadcast or
  single build. The proof for an additional residual predicate is open. [Join coverage](open-questions.md#17-join-coverage)

The derived mode is one of four (V8):

| Mode | Requirement | Output distribution |
|---|---|---|
| `co_partitioned` | Both sides `hashed`, equal signature, keys aligned. A `mark` join with a plain key pair has extra requirements (V10). | `hashed`, on keys that depend on the join type (below). |
| `broadcast_build` | Build `replicated(W)`, probe on W. Illegal if the build side is preserved (V9). | The probe's distribution. |
| `single` | Both sides `single(W)`. | `single(W)`. |
| `decided` | Both input exchanges decided by the *same* Decision; checked per label (V10). | The meet of the alternatives, each derived for the join type. |

Under `co_partitioned`, the join type decides which keys the output is hashed on:

| Join type | Output distribution |
|---|---|
| `inner` | `hashed` on the left keys, or equivalently on the right keys. |
| `left`, `left_semi`, `left_anti`, `mark` | `hashed` on the left keys only. |
| `right`, `right_semi`, `right_anti` | `hashed` on the right keys only. |
| `full` | `arbitrary(W)`. |

An unmatched row of a preserved side has nulls in the other side's key columns but stays on the
worker its own key hashed to, so the output is hashed only on keys that are never null-padded. A
`full` join pads both sides, so neither key list holds.

A join whose inputs are on different worker sets is `invalid_plan`; the error says "insert an
exchange".

- **Prints as:** `Join inner keys=[l_orderkey == o_orderkey] build=right  mode=co_partitioned(x1, x2)`;
  a null-safe pair prints `<=>`, as in `keys=[c_custkey <=> o_custkey]`. The residual, `auto` and
  build-row-count forms are in [Plan notation](plan-notation.md#node-lines-by-kind).

### Sort

- **Input:** B. Sorts within each worker.
- **Distribution:** preserves distribution and adds `sorted(keys)`. A global order holds only on
  `single`, or on `ranged` followed by `gather order=by_worker`.
- **Order:** each key's `nulls_first` or `nulls_last` places the nulls; nulls last is the default,
  whatever the direction. Floating-point keys follow the total order of
  [Nulls and NaN](../part-1/04-schemas-and-expressions.md#nulls-and-nan): NaN comes after every
  other number under `asc` and before it under `desc`. `TopN` orders its keys the same way.

### TopN

- **Input:** B. Keeps the top n rows within each worker. `TopN` composes with itself, so one node
  serves as the local and the final step.
- **Per key:** built with `top_n_per_key(keys, n, order)`, it keeps the top n rows of each distinct
  key value within each worker. Null keys equal each other, as in grouping. The output schema is
  the input's. An empty order keeps any n rows of each key. This form also composes with itself:
  a local step, a hash exchange on the keys, and a final step.
- **Ties:** which rows are kept among rows that tie on the whole order, within one key in the
  per-key form, is the engine's choice.
- **Distribution:** preserves placement. With multi-worker replicated input, tied rows kept on
  each worker can differ, but simply gathering them still counts several copies ([open question 27](open-questions.md#27-requirements-as-data-and-equivalence-properties)). A global top-n holds only on `single`; a
  global top-n per key holds only on `single`, or on `hashed` on a subset of the keys (V14).
- **Prints as:** `TopN 10 [revenue desc, o_orderdate asc]`; per key,
  `TopN 1 [o_orderdate desc, o_orderkey asc]  per=[o_custkey]`, or `TopN 1  per=[o_custkey]` with
  an empty order.

### Limit

- **Input:** S, or B when its input is sorted. Passes the first n rows per worker. An `offset` is
  legal only on `single`.
- **Unsorted input:** which n rows pass is the engine's choice. The node works batch by batch and,
  when satisfied, **closes** its input ([Protocol facts](#protocol-facts)).
- **Sorted input:** the first n rows in that order pass; among rows that tie on the sort keys at
  the cut, which are kept is the engine's choice, as for `TopN`. The node is blocking and closes
  nothing, because the engine may deliver the batches of a sorted relation in any order.
- **Distribution:** as `TopN`. The output keeps the input's `sorted(keys)`.

### Window

Adds one column per function to every input row; no row is added or dropped. Rows with equal
window keys form a **window group** (SQL's `PARTITION BY`); without keys, all rows are one group.
A node has one of three forms:

| Form | Builder | Functions | Order keys | The value a row gets |
|---|---|---|---|---|
| whole | `window_whole(keys, measures)` | `count_star`, `count`, `sum`, `avg`, `min`, `max` | none | The measure over every row of its group. |
| ranking | `window_ranking(keys, order, rankings)` | `rank`, `dense_rank`, `row_number` | one or more | Its position in the group's order. |
| running | `window_running(keys, order, measures)` | `count_star`, `count`, `sum`, `min`, `max` | one or more | The measure over the rows of its group from the first, in the group's order, through the row itself. |

- **Input:** B. **Parameters:** window keys and order keys, which name plain columns of the
  input, and one or more functions of the node's form. Measure arguments are expressions in the
  input's scope. The IR has no other pairing of function and form: `avg` in the running form,
  `count_distinct` or `stddev_samp` in either measure form, a form without a function, and a
  ranking or running form without an order key each fail at the builder call with `invalid_plan`.
- **Output schema:** the input fields, then one column per function, in the order given; names
  are unique. Creates a new scope. A measure has the result type it has in a single-phase
  [`Aggregate`](#aggregate), and `sum`, `avg`, `min` and `max` are nullable. A ranking is a
  required `int64`.
- **Groups:** two rows are in one group when their window keys are equal as hash keys are equal
  ([Hash canonicalization in v1](extension-points.md#hash-canonicalization-in-v1)). A null key
  equals a null key.
- **Order:** the rows of a group are ordered key by key, each key with its direction and its null
  placement (nulls last unless the key says `nulls_first()`), in the sort order that
  [Hash canonicalization in v1](extension-points.md#hash-canonicalization-in-v1) defines. Rows of
  one group with equal order keys, nulls included, are **peers**.
- **Functions:** `rank` is 1 plus the number of rows of the group that sort before the row, so
  peers share a rank and a gap follows them. `dense_rank` is 1 plus the number of distinct order
  key values that sort before it. `row_number` is the row's position, from 1. A running measure
  is not shared among peers: each row's range ends at the row itself. As in an aggregate,
  `sum`, `avg`, `min` and `max` skip null arguments and are null while their rows hold none, and
  a sum outside its result type fails the execution with `execution`.
- **Ties:** which peer comes first is the engine's choice. It changes `row_number` and every
  running measure among peers, and nothing else. For a repeatable answer, add order keys until
  the order is total.
- **Distribution:** with keys, requires `single`, or `hashed` on a subset of the window keys;
  without keys, requires `single` (V27). Preserves the distribution.
- **Ordering:** requires none and keeps none; the output is unsorted.
- **Key preservation:** input columns pass through unchanged, but a filter applied below the node
  would change the results of the rows it keeps, so a `Window` ends a V18 lineage.
- **Determinism:** the most volatile class among its measure arguments. A node with `row_number`,
  and every running node, is `volatile`, because the plan cannot tell whether its order is total.
  **Side effects:** `none`.
- **Several functions:** the functions of one node share its form, keys and order. Functions that
  differ in any of the three go into stacked `Window` nodes; an upper node sees the columns of the
  lower one. Stacked nodes whose requirement the same exchange meets need no exchange between them.
- **Engine support:** a key, order key or argument type that an engine build does not run in a
  `Window` fails `check` and `prepare` with `unsupported` (V21).
- **Prints as:** `Window ranking keys=[o_custkey] order=[o_totalprice desc] rankings=[price_rank = rank()]`;
  the other forms are in [Plan notation](plan-notation.md#node-lines-by-kind).

### Union

- **Inputs:** k x S. A bag union. Input types are identical, names come from input 0, a field is
  nullable if it is nullable in any input, and all inputs are on the same set.
- **Distribution:** the common `hashed` if all signatures match and all arms hash on the same output positions in the same order; `replicated` if all inputs are
  replicated; otherwise `arbitrary`. Mixing replicated and non-replicated inputs is rejected (V11).
- **Set operations:** `Union` is the only set operation with a node of its own. The others are
  compositions: distinct `UNION`, `INTERSECT` and `EXCEPT` of `Union`, `Join` and `Aggregate`,
  and `INTERSECT ALL` and `EXCEPT ALL` of `Union`, `Aggregate` and [`Repeat`](#repeat)
  ([Set operations](../part-1/07-aggregates-joins-and-ordering.md#set-operations)).

### Repeat

Emits each input row as many times as an integer column of that row says, and drops that column.

- **Input:** S. **Parameters:** the count column, a plain column of the input with an integer
  type, signed or unsigned. A count column of another type, or an input with no other column,
  fails at the builder call with `invalid_plan`.
- **Semantics:** a row with count n appears n times in the output, so a count of 0 drops the row.
  A null or negative count fails the execution with `execution`.
- **Output schema:** the input fields without the count column, in input order. Creates a new
  scope.
- **Distribution:** requires none. Preserves the distribution, `replicated` included, unless the
  count column is one of its hash or range keys; then the output is `arbitrary(W)`.
- **Ordering:** requires none and keeps none; the output is unsorted.
- **Key preservation:** the kept columns pass through unchanged, but V18 does not name the node,
  so a `Repeat` ends a V18 lineage.
- **Determinism:** `deterministic`. **Side effects:** `none`.
- **Use:** `INTERSECT ALL` and `EXCEPT ALL` count the rows per distinct row and repeat each by a
  number computed from the counts
  ([Set operations](../part-1/07-aggregates-joins-and-ordering.md#set-operations)).
- **Prints as:** `Repeat count=copies`

### Choose

- **Inputs:** k arms, one per label of a Decision, plus `<~ Decision` and its deliveries. Every arm
  has the same schema, and every arm's root is on the same set.
- **Semantics:** passes on the rows of the arm the Decision chose. The other arms never run: a stage
  that belongs only to them ends `closed` without reading input, and a sink of theirs in a shared
  stage sends end of stream with no rows. Every stage of every arm is still prepared, bound and
  admitted, so the stage graph is the same under every label. Nodes shared by several arms run
  once per worker, and their rows wait for the label in memory that can spill. Exchanges inside an
  arm are digest-checked (see [Protocol facts](#protocol-facts)).
- **Distribution and ordering:** the meet of the arms, as for a decided distribution.
- **Validation:** V26. Each arm is also checked as a plan of its own.
- **Resolved form:** the `Choose` is replaced by the chosen arm.
- **Prints as:** `Choose <~ d1, d2`, with each arm headed by its label, `├─ [on_head] Sort ...`
  ([Choosing between subtrees](../part-2/06-adaptive-plans.md#choosing-between-subtrees)).

## Distribution nodes

### Exchange

Moves rows from the input's set S to a target set T. The senders are every worker of S. Keys are
checked when `relation.exchange(...)` runs and must be plain columns of the input.

| Mode | Builder | Semantics | Output distribution |
|---|---|---|---|
| gather | `Exchange::gather(to, order)` | \|T\| = 1. `any`; `by_worker` (sender 0 to completion, then 1, ...); or `merge(keys)`, which requires sorted input. | `single(T)`; sorted after `merge`. |
| hash | `Exchange::hash(keys, to, partitions, scheme)` | The [hash rule](#distributions). `NullRouting::to_all` sends null-key rows to every worker; only the build input of a `mark` join may read such an exchange (V28). | `hashed(keys, sig)` |
| broadcast | `Exchange::broadcast(to)` | Every row to every worker of T. | `replicated(T)` |
| range | `Exchange::range(keys, boundaries, to)` | Needs a global `Boundaries` with \|T\|-1 splitters c(1)..c(\|T\|-1); with c(0) minus infinity and c(\|T\|) plus infinity, partition i holds keys k with c(i) <= k < c(i+1), so a key equal to a splitter goes to the higher partition. Keys compare key by key in sort order, direction and null placement applied first, so `desc` puts the largest values on worker 0. Equal splitters leave an empty partition between them, and each key still lands in exactly one. | `ranged(keys, b)` |
| round_robin | `Exchange::round_robin(to)` | Spreads rows evenly. Which rows go where is the engine's choice. | `arbitrary(T)` |
| forward | `Exchange::forward()` | T = S; worker i sends to worker i, so nothing moves. | the input's distribution |
| decided_by | `Exchange::decided_by(d, alternatives)` | One labelled alternative per label of `d`, all with the same T. A non-moving alternative is `forward`, so the stage graph never changes. | `decided(d, [..])`, checked as the meet |

`Partitions` selects the partition count of a hash exchange: `one_per_worker()` (the default),
`fixed(count)` (partition p to worker p mod \|T\|), `decided_by(decision)`, or
`assigned_by(assignment)`, which takes its count from the histogram behind an
[`AssignPartitions`](#assignpartitions) value and sends each partition to the worker the
assignment names. The assignment is part of the partitioning signature.

Every exchange has a **durability**. Under `Durability::pipelined()`, the default, rows flow as
they are produced. Under `Durability::spooled()`, the sender's output is retained until the
execution ends, and a receiver consumes a sender attempt's output only after that attempt
completed. Set it with `with_durability` on any exchange description; on a decided exchange it is
set once, on the result of `decided_by`, and applies to every alternative. Durability is part of
the logical edge, so it is in the fingerprint and the edge signature. Where the spool lives is the
engine's choice
([Failure, cancellation and recovery](../part-5/07-failure-cancellation-and-recovery.md#durability-pipelined-or-spooled)).

An exchange fixes what each receiver ends up with; how the rows get there is the engine's choice.
Decided and assigned exchanges, and hash exchanges with decided partitions, are digest-checked. See
Physical shape and Digests under [Protocol facts](#protocol-facts).

- **Prints as:** `Exchange x1 hash(l_orderkey) -> compute[4]`; a spooled exchange ends its line
  with `durability=spooled`, in every form and on both halves. Decided, assigned and other forms
  are in [Plan notation](plan-notation.md#node-lines-by-kind).

### ExchangeSink and ExchangeSource pairs

At start, the [`ExchangeSink`](#exchangesink) and [`ExchangeSource`](#exchangesource) of an
exchange compare its **edge signature**:

- id and execution id;
- mode and parameters, schema, hash scheme, partition count (and assignment);
- ordered destinations, the sender set (name and size), and durability;
- the IR version, the wire version and the identities of everything on the edge: the types of its
  schema (including extension and state types), the hash scheme, the value kind and merge mode,
  the Decide policy of a decided edge and the `AssignPartitions` policy of an assigned edge (see
  [Extension points](extension-points.md#versioning));
- for a range exchange, per key, `asc` or `desc` and `nulls_first` or `nulls_last`, and the digest
  of its `Boundaries`; the tie rule is part of the range mode.

Value edges compare the same record. The signature describes the edge, not the worker sets: the
membership of the side that only sends is never compared, so a sender on an unsized hosting set
agrees with its receivers without knowing its siblings.

## Aggregate state types

A state column's `DataType` is a **state type**, `StateType{function, argument type, storage}`,
available through `DataType::state()`. It prints like `sum_state<decimal(38,2)>` and has a fixed
physical layout. A final aggregate needs only its input schema, because the measures travel in it.
The plan fixes the carrier of each state; it is never chosen per batch.

| Measure | State type | Storage | Combine | Final |
|---|---|---|---|---|
| `count_star`, `count(x)` | `count_state` | int64 | sum | value |
| `sum` of any integer type | `sum_state<decimal(38,0)>` | decimal(38,0) | sum | value |
| `sum(decimal(p,s))` | `sum_state<decimal(38,s)>` | decimal(38,s) | sum | value |
| `sum(float32)`, `sum(float64)` | `sum_state<float64>` | float64 | sum | value |
| `min`/`max(T)` | `min_state<T>`/`max_state<T>` | T | min/max | value |
| `avg(T)` | `avg_state<T>` | struct{sum: sum storage(T), count: int64} | fieldwise sum | sum / count, typed |
| `stddev_samp(T)` | `var_state` | struct{n int64, mean float64, m2 float64} | parallel-variance merge | sqrt(m2/(n-1)), null if n<2 |
| `count_distinct(T)` | `distinct_state<T>` | list\<T\> as a set | set union | cardinality |

- Floating-point sum, mean and variance can round differently when workers combine states in
  a different order. The reproducibility guarantee is [open](open-questions.md#16-expression-semantics).
- A sum's result type is its storage type. A sum over any integer type, signed or unsigned, returns
  `decimal(38,0)`. Its maximum is 10³⁸ − 1. DuckDB's signed 128-bit `HUGEINT` extends to
  2¹²⁷ − 1, so some values in DuckDB’s result type exceed this decimal range. A sum outside the proposed result
  type fails the execution with `execution`.
- `min` and `max` compare with the order of
  [Nulls and NaN](../part-1/04-schemas-and-expressions.md#nulls-and-nan). Over a floating-point
  type, the `max` of a group that holds a NaN is NaN, and its `min` is NaN only when every
  non-null value is.
- The engine flattens struct states into their components when they cross an exchange.
- `distinct_state` cannot cross an exchange (V21). Plan a distinct count as a two-level grouping
  instead, as in [Aggregation](../part-3/02-aggregation.md).
- A `sum_state`, `min_state`, `max_state` or `avg_state` column is nullable when the aggregate is
  ungrouped or the measure's argument is nullable. An aggregate with a grouping set that has no
  keys counts as ungrouped here. A null state means the measure has seen no value yet: it is the
  identity state, and combine skips it. `count_state`, `var_state` and `distinct_state` columns are
  never nullable, because their identity is a zero count or an empty set.
- The result of `sum`, `min`, `max` or `avg` is nullable under the same condition, and in a final
  it is nullable exactly when its state column is. A `stddev_samp` result is always nullable, and
  a count never is.
- A nullable state column prints with the `?` mark, as in `total_revenue sum_state<decimal(38,4)>?`.

## Runtime-value nodes

### Optional and required values

| Kind | Produced by | Optional or required | When it is missing |
|---|---|---|---|
| `Filter` | `RuntimeFilterBuild`, merged by `ValueExchange` | optional | Every row passes. A `wait=up_to(t)` apply first waits at most t. |
| `Statistics` | `Observe`, merged by `ValueExchange` | required | The consumer waits; if it fails or is skipped, the execution fails. |
| `Boundaries` | `ComputeBoundaries` | required | As Statistics. |
| `Decision` | `Decide` | required | As Statistics. |
| `Assignment` | `AssignPartitions` | required | As Statistics. |

Each producing worker ends exactly once, in one of four completion states: `published`, `skipped`,
`failed` or `cancelled`. A skipped or failed filter counts as missing; a skipped or failed required
value fails the execution with `execution`. While a consumer waits for a required value, its
pending input waits in memory that can spill.

### Value scope

Every value has a `ValueScope`:

- `partial`: built from this worker's rows only;
- `partition(xK)`: complete for the rows of the partitions of exchange xK that this worker holds;
- `global`: complete for all rows.

`merge_to(set)` inserts a [`ValueExchange`](#valueexchange) and produces a `global` value.

### RuntimeFilterBuild

- **In -> out:** data S -> the same data, plus `~> Filter`.
- **Parameters:** keys, `RuntimeFilterSpec` (`bloom(geometry)`, `in_list(max_values)`, `min_max()`,
  combined with `or_else`), and the spec's null rule, `NullKeys::discard` (the default) or
  `NullKeys::pass`. A filter never stores a null key.
- **Semantics:** publishes after it has seen its complete input on this worker. Data passes through
  unchanged, so distribution is preserved. Over an input closed before its end, it ends `skipped`:
  it never publishes a filter built from part of its input.
- **Scope:** `global` if the input is `replicated` or on a size-1 set; `partition(xK)` if the input
  is `hashed` by xK on exactly the filter keys; otherwise `partial`.
- **Prints as:** `RuntimeFilterBuild keys=[o_orderkey] spec=bloom(fixed 8MiB)  scope=partial ~> f1`;
  `NullKeys::pass` prints `nulls=pass` after the spec.

### RuntimeFilterApply

- **In -> out:** data S plus `<~ Filter` -> data.
- **Semantics:** drops only rows whose key is definitely absent, so there are no false negatives.
  A row with a null in a filter key is dropped under `NullKeys::discard`, because no plain key
  pair matches it, and passes under `NullKeys::pass`. `wait=never` takes a snapshot of the filter
  at each checkpoint. `wait=up_to(t)` holds the input for at most t; if the filter has not arrived
  by then, rows pass unfiltered, so a bounded wait never fails the execution.
  `push_filters_into_scans` fuses an apply into the `Scan` below it.
- **Legality:** V16-V18.
- **Prints as:** `RuntimeFilterApply [l_orderkey] <~ f2  wait=never`

### ValueExchange

- **In -> out:** values from every worker of S -> one value on every worker of T.
- **Semantics:** every receiver ends up with the merge of every sender's share, a `global` value.
- **Mode:** `merge`, with one rule per kind:
  - **Filter:** Bloom by OR (geometry, hash and key domain identical, V13); min-max by min and max;
    in-list by union, which on overflow degrades along `or_else` deterministically (if any share
    degrades, all do). The merged filter is `published` only if every partial is. The partials
    share one null rule (V13), and the merged filter keeps it.
  - **Statistics:** sums rows and bytes, pools samples. Each pooled sample keeps the row count
    of the worker that drew it. A pool is ordered by sender index before a policy or
    `ComputeBoundaries` reads it, so it does not depend on arrival order.
  - **Decision, Boundaries, Assignment:** delivered as-is, since they are already global.
- Every merge rule is associative and commutative, so the order and grouping of shares never
  change the result.
- With one sender it delivers the value to every receiver
  ([Deciding on a leader](../part-2/06-adaptive-plans.md#deciding-on-a-leader)).
- **Prints as** a `values:` line: `f2 = ValueExchange merge f1 -> compute[4]`

### Observe

- **In -> out:** data S -> the same data, plus `~> Statistics`.
- **Parameters:** `StatisticsSpec`: `rows()`, `bytes()`, `key_sample(keys, rows_per_worker)` or
  `partition_bytes(keys, P, scheme)`, a histogram of logical bytes per partition, hashed as an
  exchange with the same keys, scheme and partition count would hash. Histograms merge by
  element-wise sum. A `key_sample` share carries up to `rows_per_worker` keys and the number of
  rows the worker saw, so a later `ComputeBoundaries` can weight it.
- **Units:** `rows` counts rows. `bytes` counts **logical bytes**, computed from Arrow type widths:
  fixed-width columns as width times rows, plus the payload lengths of strings and lists. It never
  counts encoded, compressed or device bytes, so two engines, or a spilled and a resident batch,
  report the same number.
- **Semantics:** never blocks; publishes at its input's EOS. If its stage closes early, it
  publishes what it saw, flagged `truncated`. A merged value is truncated if any share is. A
  truncated count of zero cannot prove that a mark join's build was empty. If that proof is
  still required, the producer must finish counting or the execution must fail.
- **Prints as:** `Observe bytes ~> s1`, or `Observe partition_bytes(o_custkey, 16) ~> s1`

### ComputeBoundaries

- **In -> out:** `<~ Statistics` (a global key sample) -> `~> Boundaries`.
- **Semantics:** pure and deterministic; produces \|T\|-1 splitters for a range exchange to T. It
  reads the pooled sample in sender order and weights each worker's sample by the number of rows
  that worker saw. The proposal does not specify what the required value carries if the whole
  sample is empty and there is more than one destination. The value is digest-checked ([Protocol facts](#protocol-facts)).
- **Prints as** a `values:` line: `b1 = ComputeBoundaries s2 -> compute[4]`

### Decide

- **In -> out:** `<~ Statistics` (global) -> `~> Decision`.
- **Semantics:** a pure policy. `threshold(metric, at_most)` yields `[label_at_most, label_above]`;
  `partition_count(target_bytes, min, max)` yields ceil(bytes / target) clamped to [min, max]. Its
  input must be global wherever the policy runs (V15). Each dependent worker receives the same
  label, whether it evaluates the policy itself or receives a delivered decision. Policy inputs are integers or decimals only (`Metric::rows`,
  `Metric::bytes` and integer targets), so floating-point rounding cannot make two workers
  disagree. Each decision is recorded in the `ExecutionReport`.
- **Prints as** a `values:` line:
  `d1 = Decide threshold(s2.bytes, at_most=64MiB) -> [broadcast, partitioned]`

### AssignPartitions

- **In -> out:** `<~ Statistics` (a global `partition_bytes` histogram) -> `~> Assignment`.
- **Semantics:** a pure policy that maps each of the histogram's P partitions to a worker of the set
  it controls. `balanced` takes partitions largest first, ties by partition index, and gives each to
  the least-loaded worker so far, ties by worker index. Its inputs are integers, and its input must
  be global wherever the policy runs (V15). Every consumer must receive the same map. The `Assignment`
  is a required value and is digest-checked ([Protocol facts](#protocol-facts)).
- **Used by:** `Partitions::assigned_by(assignment)` on a hash exchange; both sides of a
  co-partitioned join use the same assignment (V20).
- **Prints as** a `values:` line: `a1 = AssignPartitions balanced(s2) -> compute[4]`; the resolved
  map is under [Resolved form](plan-notation.md#resolved-form)
  ([Running at scale](../part-5/08-running-at-scale.md#skew-more-partitions-than-workers)).

## Protocol facts

The exchanges and value edges of a plan imply a protocol between workers. It needs no
configuration.

- **EOS.** Every sender sends end of stream to every receiver of every exchange, even after zero
  rows. End of stream carries its sequence number, so a receiver can tell a complete stream from one
  with a gap. A receiver is complete after EOS from every sender and delivery of each stream through its final sequence number. Duplicates are idempotent.
  The non-target workers of a gather have no `ExchangeSource` instance.
- **Close.** A satisfied `Limit` over unsorted input closes its input. Across an exchange, close
  goes upstream to every sender. A sender drops further batches for that receiver, still sends
  EOS, and its stage stops early once every receiver has closed. Such a stage instance ends
  `closed`, which is not a failure. It still ends every value edge it produces: a filter ends
  `skipped`, an `Observe` publishes with a `truncated` flag, and a required value is published or
  the execution fails
  ([Close is not end of stream](../part-5/07-failure-cancellation-and-recovery.md#close-is-not-end-of-stream)).
- **Failure.** Any failure on any worker fails the whole execution, unless the host retries the
  failed stage instance. Failure travels along the plan's edges in both directions, downstream to
  receivers and upstream to senders, so an open plan needs no participant list. The instance where
  the error happened ends `failed`; every instance stopped by the failure ends `cancelled`. The
  origin is preserved in the diagnostic. If failures race, choosing one origin for all readers
  and `wait()` would require an agreement rule. [Coordination and failure](open-questions.md#4-coordination-and-failure). A failure that affects only a filter degrades that filter to `skipped`
  ([Failure, cancellation and
  recovery](../part-5/07-failure-cancellation-and-recovery.md#fail-stop-along-the-edges)).
- **Cancellation.** Cancellation is idempotent, and any participant can issue it. It travels along
  edges like a failure and reaches every peer of the execution.
- **Message keys.** Every message is keyed by (execution id, attempt, epoch, edge id, sender,
  receiver, sequence). The attempt is 0 unless the sending instance was retried; the epoch is
  reserved and always 0 ([Extension points](extension-points.md#reserved-now-epochs)); the sequence
  counts from 0 per stream and attempt. A receiver drops duplicates and ignores messages from
  attempts it is not consuming. An `ExecutionId` is single use per hosted worker: a context refuses
  a second start of an id for a worker that already ran under it. The wire knows only logical ids;
  endpoints come from bindings.
- **First-contact deadline.** After `start`, every peer of an edge must answer the handshake within
  the deadline set with `PrepareOptions::with_first_contact_deadline`, or the edge fails with
  `transport`.
- **Durability.** A `spooled` [exchange](#exchange) lets a host re-run one stage instance under a
  new attempt
  ([Durability](../part-5/07-failure-cancellation-and-recovery.md#durability-pipelined-or-spooled)).
- **Agreement.** Each process initiates peer agreement at `start`. An edge must agree before it carries rows.
  Agreement on one edge of an open plan provides no readiness guarantee for unrelated fragments.
  Closed plans compare (ExecutionId, plan fingerprint, bindings digest); the bindings digest covers
  the binding epoch, which a retry that moves a worker increases. Open plans compare the edge
  signature of every exchange and value edge they share, durability included; a dynamic scan's
  claims (the split list by digest, and the set) are agreed the same way. A mismatch is
  `binding_mismatch`.
- **Digests.** Every batch and end of stream on a decided exchange, on a hash exchange with
  decided partitions, or on any exchange inside an arm of a `Choose`, carries a digest of the label
  its sender used. Every delivered `Boundaries` or `Assignment` value carries a digest of its
  contents, and so does every batch on an assigned exchange. A re-published value share carries
  one too. A receiver that sees a digest other than its own fails the execution with `execution`.
- **Credits.** Transports grant credits per logical stream (one edge, one sender, one receiver).
  Only streaming queues are bounded by credits. A **barrier queue**, whose consumer may wait for
  something else first, accepts everything it is sent and spills: the router of an exchange that
  needs a required value, a join's build, a join's probe while its build is incomplete, and each
  per-sender queue of a gather that reads its senders in a fixed order, `merge` or `by_worker`
  ([Running at scale](../part-5/08-running-at-scale.md#credits-and-deadlock-freedom)).
- **Physical shape.** The engine chooses how each edge is carried out: transport, chunking,
  compression, flow control and credits, connection pooling, direct delivery or forwarding through
  an intermediate worker, a relay or multicast broadcast, a two-level shuffle that moves rows
  within a host before they cross the network, where a spool lives, how dynamic split claims are
  served, and the reduction shape of a value merge (all to all, a tree, a ring or through one
  host). None of these changes what a receiver ends up with. Where the plan says `arbitrary`, the
  engine also chooses batch boundaries, which rows a `round_robin` sends where, and which n rows a
  per-worker `Limit` keeps; in a `Window` it chooses which peer comes first. Results are
  deterministic up to the nondeterminism SQL already allows
  ([Where the plan says `arbitrary`](../part-2/01-everything-in-the-plan.md#where-the-plan-says-arbitrary)).

## Validator invariants

Each invariant has a number you can cite. The checks run at three gates: `Plan::create` checks the
plan on its own (V1-V20, V24 and V26-V28), `Context::check` checks it against this engine build
(V21, V22), and `prepare` checks it against the bound devices and the bindings (V21 and V22
again, V23). At start, the peers check that they agree (V25).
[The invariants](../part-2/07-validation.md#the-invariants) gives the full reason and an
example error for each.

### Checked by `Plan::create`

| # | Invariant | Why |
|---|---|---|
| V1 | The wait-for graph is acyclic: the data and value edges, the ordered waits (a required value before rows, a join's build before its probe, every sender before a merge step), and the credit waits into streaming queues. | Rules out cycles among the waits represented in the plan. Runtime scheduling, memory and spill progress are separate obligations. |
| V2 | Every referenced set is declared. All nodes of a stage share one set. | Every node needs a place to run, and a stage instance runs on one worker. |
| V3 | Output names are unique. Inputs are unique by name. Exchange and value ids are unique. | Readers, input bindings and wire messages address them by name or id. |
| V4 | Expressions reference only their input scope. Operands of arithmetic and comparisons have the same type. Predicates are boolean. Both keys of a join pair have the same type, and `Union` inputs have identical types. No cast is inserted. | Nothing is converted implicitly, and keys of one type hash and compare alike on every worker. |
| V5 | A combine or final input is exactly the keys plus the state columns of one partial signature. | The final phase reads its measures from the schema alone. |
| V6 | Grouped single or final input is `single`, or `hashed` on a subset of the group keys. For a single-phase aggregate with grouping sets the group keys are those in every set; for a final they include the grouping id. | Every group must be complete on one worker. |
| V7 | Partial and single never consume `replicated` on \|W\| > 1. | Each worker would count the same rows again. |
| V8 | A join is co_partitioned, broadcast_build, single or decided. | Matching rows must meet on the same worker. |
| V9 | No broadcast_build where the build side is preserved (right/full with build=right; left/full with build=left; right_semi/right_anti with build=right; left_semi/left_anti with build=left). | Every worker would emit the unmatched build rows, duplicating them. |
| V10 | `mark` with a co-partitioned build and a plain key pair requires exactly one key pair, (a) `NullRouting::to_all` on the build exchange, which may differ from the probe's `hashed` in that one field, and (b) a global `rows` Statistics of the build input through `with_build_rows`, merged from an `Observe rows()` that sits directly below the build exchange with no `Filter` or `Limit` between them. For this key-only test the join needs to know whether any build row exists. A truncated zero cannot establish this. Decided joins are checked per label, and both exchanges use the same Decision. A broadcast or single mark join needs neither, and neither does a mark join whose key pairs are all null-safe. The build is the right input, as for every `mark` join. | A probe row's mark depends on the whole build, not one worker's share; each label must be a legal join. |
| V11 | `Union` and `Output` reject a mix of `replicated` and non-replicated inputs. | Replicated rows would be counted once per worker. |
| V27 | The input of a `Window` with keys is `single`, or `hashed` on a subset of the window keys. The input of a `Window` without keys is `single`. | Every row of a window group must be on one worker. |
| V28 | A hash exchange with `NullRouting::to_all` is read only by a `mark` join, as its build input. | Rows with a null key are on every worker, so any other reader would see them once per worker. |
| V12 | A value edge between different sets, or from a non-global scope to another worker, passes through a `ValueExchange`. | Everything that moves between workers is a node. |
| V13 | Merged filter partials share kinds, geometry (`fixed`, or `sized_by` the same global statistic), hash, key domain and null rule. | Bloom OR and in-list union are only correct over identical layouts. |
| V14 | A TopN or Limit over multiple workers gives a per-worker bound. A single worker proves a global bound, and hashing on a subset of the group keys proves a per-key bound. An offset requires `single`. `gather merge` input is sorted on its keys. `range` uses a global Boundaries on the same keys. | Checks the guarantees the constructed plan supplies. Omitting a global step can produce a valid plan for a different query. |
| V15 | Inputs to `Decide`, `ComputeBoundaries` and `AssignPartitions` are global on the workers evaluating the policy. The resulting value reaches every worker it controls. | Computing the policy locally or delivering its result must produce one common value. |
| V16 | A `partial` filter is never applied. | It would drop rows that match on another worker. |
| V17 | A `partition(xK)` filter is applied only on the same set, to a relation `hashed` with xK's signature on the corresponding columns, on the same worker. | It is complete only for the rows that partition holds. |
| V18 | In a closed plan a filter is applied only on the probe-side lineage of the join whose build feeds it. The lineage passes only key-preserving nodes (Filter, Project passing the filter keys through unchanged, and Exchange of any mode, including decided exchanges), and the join type discards non-matching probe rows. A filter with a key from a null-safe pair of that join has `NullKeys::pass`. | Pruning must remove only rows the join would discard anyway, and a null-safe pair can match a null key. |
| V19 | `forward` has T = S. `gather` has \|T\| = 1. All alternatives of a decided exchange share T. | Forward moves nothing, a gather has one destination, and the stage graph must be the same under every label. |
| V20 | Partitions decided by a Decision, or assigned by an Assignment, are used identically on both sides of a co-partitioned join. | Both sides need the same partition count, and the same partition-to-worker map, to stay co-partitioned. |
| V26 | A `Choose` has one arm per label of its Decision, all with the same schema. Its handles are the Decision and one-sender deliveries of it, global together on every set that runs a node of any arm. The Decision's inputs come from outside every arm, and a value made inside an arm is consumed only inside it. | Every worker that might run an arm knows the label without asking, and an arm that is not chosen never publishes a value. |
| V24 | A set is sized wherever the plan depends on its size: every sender set, every target set of an exchange or value edge in the plan that sends on it (including a hosting set that receives a partitioned or gathered exchange), any set with a `files_round_robin`, `byte_ranges`, `explicit_splits` or `dynamic` scan, any set a `Decide`, `ComputeBoundaries` or `AssignPartitions` controls, and any set that sends to itself. Only the hosting set of an instance plan may be unsized. | EOS counts, routing, shard rules and decisions all need to know how many workers there are. |

`Plan::create` also makes three checks without a number: it rejects a shared node whose subtree
contains an `Input`, a `volatile` node or an `effectful` node, unless its consumers all sit in
different arms of one `Choose` ([Shared subplans](extension-points.md#shared-subplans)); a decided
exchange whose alternative sets its own durability; and a [join](#join) whose build side is `auto`
while its mode is `broadcast_build` or `decided`. All three fail with `invalid_plan`. Before that,
each builder call resolves the function, state type, policy, hash scheme, scan kind or value kind it
names in its [registry](extension-points.md#closed-and-open-lists); an unknown name fails with
`invalid_plan` too.

In an open fragment the join that justifies a runtime filter may lie outside the graph.
A matching value-edge signature identifies the key and filter layout. It cannot prove that an
unseen join discards nonmatches. The host and validator still need a contract for this proof.
[Open-plan composition](open-questions.md#9-open-plan-composition)

### Checked by `Context::check`

`prepare` runs these checks again for the bound devices. A plan that fails them fails with
`unsupported`; it is never replanned and never falls back.

| # | Invariant | Why |
|---|---|---|
| V21 | Every node, type, function and value kind is supported by this engine build, including types that cross exchanges and key types the hash scheme defines. | The host can report an unsupported operator or type before running the plan. |
| V22 | Registered scan kinds are implemented. | A host-registered source needs its reader in this process. |

### Checked by `prepare`

| # | Invariant | Why |
|---|---|---|
| V23 | Every worker of every set touched by a hosted stage is bound exactly once to an existing endpoint; of an unsized hosting set, only the hosting worker. Every `Input` instance is bound. | Every logical worker needs exactly one place to run. |

`prepare` also resolves relative scan paths. It opens no file unless `SourceCheck::at_prepare` asks
for the early source check. That check is not a plan invariant: it compares files with the plan
and fails with `source_mismatch` ([Scan](#scan)).

### Checked at start

| # | Invariant | Why |
|---|---|---|
| V25 | Peers agree (see [Protocol facts](#protocol-facts)) at start. | Workers with different plans would route rows differently. |
