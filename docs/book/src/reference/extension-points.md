# Extension points

This page lists where the Plan IR can grow and what a new member must declare so that validation,
rewrites and the agreement check keep working. The node vocabulary is in the
[Node catalog](node-catalog.md).

One rule governs every entry: an extension states, as data, every fact two workers must agree on
and every property later nodes may rely on. This is the
[ownership rule](../part-2/01-everything-in-the-plan.md#the-ownership-rule) applied to growth.
Something that cannot state those facts cannot be in a plan.

## The node contract

Every node declares the same ten facts, whether it is built in, added in a later IR version or
registered by a host.

| Declaration | What it states | Read by |
|---|---|---|
| Schema rule | The output schema as a function of the parameters and the input schemas. | Construction; V4, V5 |
| Required input distribution | Per input, the distributions it accepts, as data (see [Requirements are data](#requirements-are-data)). | Construction (V6-V11, V14, V27, V28); front-end planners |
| Required input ordering | Per input, `sorted(keys)` or nothing. | Construction (V14); front-end planners |
| Output distribution | How the output's [distribution](node-catalog.md#distributions) derives from the inputs'. | Every node above it |
| Output ordering | How `sorted(keys)` derives from the inputs'. | Every node above it |
| Key preservation | Whether columns that pass through keep their values row for row, so a runtime filter may be applied below the node instead of above it. | V18 |
| Value edges | The values it consumes and produces, the scopes it accepts, and the scope rule for what it produces. | V12, V15-V18 |
| Input contract | Per input: streaming (S), blocking per group, or blocking (B). A contract may depend on whether the input is sorted, as `Limit`'s does. | Lowering: a blocking input is a `FULL` barrier, a streaming one `PARTIAL` across pipelines and `PIPELINE` inside one |
| Determinism class | `deterministic`, `per_execution` or `volatile` ([below](#determinism-classes)). | Distribution derivation; sharing; retry |
| Side-effect class | `none`, `idempotent` or `effectful` ([below](#side-effect-classes)). | Sharing; retry |

**Fail closed.** An omitted declaration takes its weakest value: output `arbitrary(W)`, unsorted,
not key-preserving, `volatile`, `effectful`. A required input distribution has no default: a node
that does not state one cannot be registered. An unmet requirement fails construction with
`invalid_plan`. Nothing re-routes rows to make a node legal.

### How built-in nodes answer

| Declaration | `Scan` | `Project` | `Join` | `RuntimeFilterBuild` |
|---|---|---|---|---|
| Schema rule | The selected columns of the source schema. | The named expressions, in order; a new scope. | Left fields, then right; `left_semi`/`left_anti` only left, `right_*` only right, `mark` appends a boolean. | The input schema, unchanged. |
| Required input distribution | None (leaf). A per-set shard rule needs a sized set (V24). | Any. | Same set on both sides, and one of the four modes (V8-V10); see [Join](node-catalog.md#join). | Any. |
| Required input ordering | None. | None. | None. | None. |
| Output distribution | `arbitrary(W)`; `replicated(W)` with `Shards::replicated()`; `single(W)` on a size-1 set. | `hashed` and `ranged` survive only if every key passes through as a plain column, else `arbitrary`. `replicated` survives unless an expression is `volatile`. | By mode and, under `co_partitioned`, by join type ([Join](node-catalog.md#join)). | The input's. |
| Output ordering | Unsorted. | `sorted(keys)` survives if every sort key passes through as a plain column. | Unsorted. | The input's. |
| Key preservation | Not applicable (leaf). | Yes for plain columns ("plain Project" in V18). | No. A V18 lineage starts at the join whose build feeds the filter. | Rows pass through unchanged; it sits on the build side, outside any V18 lineage. |
| Value edges | After `push_filters_into_scans`, consumes the filters of the applies fused into it, under the same V16-V18 rules. | None. | May consume a global `rows` Statistics, a required value, through `with_build_rows`; a co-partitioned `mark` join with a plain key pair needs it (V10). | Produces a `Filter`: `global` on a `replicated` input or a size-1 set, `partition(xK)` on an input `hashed` by xK on exactly the filter keys, otherwise `partial`. |
| Input contract | None (leaf). | S. | Probe S, build B. Null-key probe rows of a `mark` join wait for the build row count. | S; publishes after its complete input on this worker. |
| Determinism class | `deterministic` when the splits are fixed in the plan. A `Shards::dynamic()` scan is `per_execution`: its claims are fixed once per execution, and a retried attempt re-reads the splits its failed attempt claimed. | The most volatile class among its expressions. | `deterministic`, or its residual's class if higher. | `deterministic`. |
| Side-effect class | `none` for Parquet; a registered scan kind declares its own. | `none`. | `none`. | `none`. |

The node catalog's [protocol facts](node-catalog.md#protocol-facts) are part of every contract. A
new node inherits them, cannot override them and adds no protocol of its own.

### Requirements are data

A required input distribution is one of a small set of values, each satisfied by a fixed set of
derived distributions. Validation checks it, and a front-end's planner can read the same value to
place the exchange that meets it.

| Requirement | Satisfied by | Stated by |
|---|---|---|
| any | Every distribution. | `Filter`, `Project`, `Aggregate` combine, `Exchange` |
| not replicated | Anything but `replicated(W)` on \|W\| > 1. | `Aggregate` partial and single (V7) |
| `single(W)` | `single(W)`, or any non-replicated distribution on \|W\| = 1. | Ungrouped `Aggregate` single and final; global `TopN` without keys and global `Limit`, and a `Limit` with an offset (V14); a `Window` without keys (V27) |
| hashed on a subset of keys | `single(W)`, or `hashed(k, sig)` with k a subset of the keys. | Grouped `Aggregate` single and final (V6; which keys count with grouping sets is under [Aggregate](node-catalog.md#aggregate)); a global per-key `TopN` (V14); `Window` with keys (V27) |
| co-partitioned pair | Both inputs `hashed`, equal signatures, keys aligned positionally. | `Join`, mode `co_partitioned` (V8) |
| replicated build | Build `replicated(W)`, probe on W, build side not preserved. | `Join`, mode `broadcast_build` (V8, V9) |
| `sorted(keys)` | An input sorted on those keys within each worker. | `Exchange` with `gather order=merge` (V14) |
| global value | A value of scope `global` on every set it controls. | `Decide`, `ComputeBoundaries`, `AssignPartitions` (V15) |

One distribution is an exception to every row: `hashed` under `NullRouting::to_all` satisfies a
requirement, `any` included, only as the build input of a `mark` join (V10, V28).

DataFusion's `EnforceDistribution` and `EnforceSorting` passes place exchanges from requirements
like these (see
[Related designs](../appendix/related-designs.md#requirements-and-the-passes-that-enforce-them)).
Sirius ships no such pass: the front-end writes every exchange. How far the property model goes,
including equivalence classes, is
[open question 27](open-questions.md#27-requirements-as-data-and-equivalence-properties).
A helper that spreads a plan over the GPUs of one machine is
[open question 36](open-questions.md#36-a-distribution-helper-for-one-machine).

## Determinism classes

Every function and every node has a determinism class. The classes run from least to most
volatile, and a composite takes the most volatile class of its parts.

| Class | Meaning | Examples | On a `replicated` input |
|---|---|---|---|
| `deterministic` | The same inputs give the same outputs, on every worker and in every execution. | Arithmetic, comparisons, `cast`, `is_null` | Output stays `replicated`. |
| `per_execution` | Constant within one execution. For a function, the builder replaces the call with a literal of its value when the plan is built; for a node, the engine fixes its choice once per execution. | `now()`, `current_date()`; a `Shards::dynamic()` scan | Output stays `replicated`: by then the call is a literal. |
| `volatile` | May differ per row, per call or per worker. | `random()` | Output degrades to `arbitrary(W)`. |

- **Functions.** Each function's registry entry carries its class, and `Expression::call` and
  `Expression::call_in` read it ([open question 16](open-questions.md#16-expression-semantics)).
- **`per_execution` in the plan.** The plan stores and prints the literal and the fingerprint
  covers it, so processes that build the same closed plan independently (SPMD) must agree on the
  value. Such a host reads the value once, shares it like the `ExecutionId`, and has every process
  build the literal with `Expression::literal` instead of calling the function.
- **Nodes.** A node's class is the most volatile among its expressions and its own. The engine
  picks the rows kept by a `Limit` on an unsorted input, a `Limit` on a sorted input whose cut
  falls among ties, and a `TopN` with ties, per key or not. For a per-key `TopN` with an empty
  order, the engine picks every row kept. All of these are `volatile`, so a shared subtree that
  contains one is rejected. A `Window` with `row_number` and every running `Window` are `volatile`
  too: which peer comes first is the engine's choice.
- **Where `replicated` is required.** A `volatile` node over a broadcast build leaves the join
  without a legal mode:
  `invalid_plan: V8: Join has no legal mode: build is arbitrary(compute[4]) after volatile Project
  over replicated input; move random() above the join`.
- **Policies and merge rules** are always `deterministic`. A `Decide` policy, a `ComputeBoundaries`
  rule, an `AssignPartitions` policy or a value kind's merge rule of another class cannot be
  registered.

## Side-effect classes

Every node has a side-effect class. A scalar function never has side effects: a registry accepts
only functions whose class is `none`.

| Class | Meaning | Built-in members | Retry | Sharing |
|---|---|---|---|---|
| `none` | No effect outside the execution. | Every relational, distribution and runtime-value node; `Values`; `Scan` of Parquet; `ExchangeSink` and `ExchangeSource` | May re-run. | Allowed. |
| `idempotent` | Has an effect, but repeating it within one execution leaves the same state. | None today. A registered scan kind that takes a lease keyed by the `ExecutionId` would be one. | May re-run. | Allowed. |
| `effectful` | Repeating it changes the outside world, or cannot be done. | `Input` (a channel is read once); `Output` once its first batch has reached the host | Never re-run. | Rejected. |

A registered scan kind declares its class at registration. The retry rules are in [Failure,
cancellation and
recovery](../part-5/07-failure-cancellation-and-recovery.md#side-effects-decide-what-may-rerun) and
[open question 28](open-questions.md#28-retry-api-and-who-decides).

Both classes are public values:

```cpp
enum class Determinism { deterministic, per_execution, volatile_ };
enum class SideEffects { none, idempotent, effectful };

Determinism Expression::determinism() const;   // most volatile class among its calls
Determinism Relation::determinism() const;     // most volatile class in the subtree
SideEffects Relation::side_effects() const;    // strongest class in the subtree
```

```rust
#[non_exhaustive]
pub enum Determinism { Deterministic, PerExecution, Volatile }
#[non_exhaustive]
pub enum SideEffects { None, Idempotent, Effectful }

impl Expression { pub fn determinism(&self) -> Determinism; }
impl Relation {
    pub fn determinism(&self) -> Determinism;   // most volatile class in the subtree
    pub fn side_effects(&self) -> SideEffects;  // strongest class in the subtree
}
```

C++ spells the enumerator `volatile_` because `volatile` is a keyword.

## Closed and open lists

A **closed** list gains members only through a new IR version. An **open** list accepts members
under a host's or library's own namespace. Its members live in a `Registry`, an optional input to
plan construction:

- **Construction resolves names in it.** A registry is immutable and needs no `Context`.
  `Registry::builtin()` is the default, and `Registry::builder()` extends it.
  `Expression::call_in(registry, function, args)` resolves a function in a host registry;
  `Expression::call` uses the built-in one. An unknown name fails construction with
  `invalid_plan`.
- **The engine must implement them.** Registering a member does not make an engine run it. The
  engine of every process that runs the plan must implement every identity the plan uses.
  `Context::check` and `prepare` reject a member this engine build does not implement with
  `unsupported` (V21, V22).

| List | Today's members | Closed or open | How a member is added | Carried in the fingerprint and edge signature |
|---|---|---|---|---|
| Node kinds | The nodes of the [node catalog](node-catalog.md#node-index) | Open, later | A registered node declares the full [node contract](#the-node-contract). | Its identity and parameters. |
| Functions | The expression combinators (`add`, `eq`, `cast`, ...) and the registry entries behind `Expression::call` and `Expression::call_in` | Open | A registry entry: identity, argument and result types, determinism class; side effects `none`. | The identity of every function called. |
| Data types | `boolean`, `int8` to `int64`, `uint8` to `uint64`, `float32`, `float64`, `decimal`, `string`, `date32`, `timestamp`, `list`, `struct` | Open, via extension types | An extension type is a name plus a storage type; hashing, comparison and the wire go through the storage type. | Each column's type identity, including its storage type. |
| Aggregate state types | `count_state`, `sum_state`, `min_state`, `max_state`, `avg_state`, `var_state`, `distinct_state` ([table](node-catalog.md#aggregate-state-types)) | Open | An Arrow schema for the state plus named, versioned combine and final functions; combine is associative and commutative. | The state type identity in the schema. |
| Value kinds | `Filter`, `Statistics`, `Boundaries`, `Decision`, `Assignment` | Open interface | A layout, a merge rule (associative, commutative, deterministic), optional or required, and scope rules. | The kind identity and its layout. |
| `ValueExchange` modes | `merge` | Open tag | A new mode with its delivery rule. `prefix` is the planned next one ([question 29](open-questions.md#29-windows-over-a-global-order-prefix-value-mode)). | The mode tag. |
| Decide policies | `threshold`, `partition_count` | Open | A deterministic function from a global `Statistics` to one label, over integer or decimal inputs. | The policy identity with its version, and its parameters. |
| `AssignPartitions` policies | `balanced` | Open | A deterministic function from a global `partition_bytes` histogram to a partition-to-worker map, over integer inputs ([Running at scale](../part-5/08-running-at-scale.md#skew-more-partitions-than-workers)). | The policy identity with its version, and its parameters. |
| Hash schemes | `v1` | Open; `v1` frozen for the types it defines | A new scheme under a new name, such as Spark's Murmur3 hash or Iceberg bucket hashing. | The scheme identity and null routing, in the partitioning signature. |
| Exchange modes | `gather`, `hash`, `broadcast`, `range`, `round_robin`, `forward`, `decided_by` | Closed for now | A new IR version. Skew-aware hashing is [question 26](open-questions.md#26-skew-aware-exchanges). | Mode and parameters. |
| Partition rules | `one_per_worker`, `fixed`, `decided_by`, `assigned_by` | Closed | A new IR version. | The partition count, and the decision or assignment behind it, in the partitioning signature. |
| Durability | `pipelined`, `spooled` | Closed | A new IR version. | The durability of each exchange. |
| Shard kinds | `files_round_robin`, `byte_ranges`, `replicated`, `explicit_splits`, `splits`, `dynamic` | Closed for now | A new IR version. Partitioning declared by the source is later ([question 13](open-questions.md#13-hints-and-certified-properties)). | The shard kind and the resolved split lists, or a manifest digest; every declared file size. |
| Scan kinds | Parquet; registered kinds such as `duckdb.native` | Open | `TableSource::registered` with a registered kind ([question 14](open-questions.md#14-host-registered-scan-kinds)). | The kind identity; never the `ScanHandle`. |
| Transports | In process, network | Outside the plan | `TransportConfig` on the `Context`. | Nothing; the wire version is in the edge signature. |
| Rewrites | `split`, `instance`, `join_stages`, `resolve`, `push_filters_into_scans`, `insert_runtime_filters`, `collapse`; no distribution pass ([question 36](open-questions.md#36-a-distribution-helper-for-one-machine)) | Open | Any function from plan to plan. It reads the plan through the [node view](api-at-a-glance.md#node-view) (`Plan::terminals()`, `Relation::kind()`, `Relation::children()`), within that view's limits, and builds its result with the builder. | Nothing: the output is an ordinary plan with its own fingerprint. |
| Join types | The nine of `JoinType` | Closed | A new IR version. | The join type. |
| Wait policies | `never`, `up_to(t)` | Closed | A new IR version. | Nothing: a wait is local to the consumer. |
| Determinism and side-effect classes | Three each | Closed | A new IR version. | Through the members that carry them. |

## Versioning

| Versioned thing | Identity | Example |
|---|---|---|
| The IR | One IR version number for the node vocabulary and its semantics. | IR version 1 |
| Functions, state types, `Decide` and `AssignPartitions` policies, hash schemes, scan kinds, value kinds, registered nodes | (namespace, name, signature, version) | (`sirius`, `random`, `() -> float64`, 1); (`sirius`, `sum_state`, `decimal(38,0)`, 1) |
| The wire | One wire version for the message format between engines. | Wire version 1 |
| The C ABI | `sirius_abi_version()`. Independent of the other three. | 0 |

- **Where identities go.** The fingerprint of a closed plan covers the IR version and the identity
  of every member the plan uses. The edge signature of an open plan carries the IR version, the wire
  version and the identities of everything on that edge. The last column of the
  [table above](#closed-and-open-lists) gives each list's contribution;
  [ExchangeSink and ExchangeSource pairs](node-catalog.md#exchangesink-and-exchangesource-pairs)
  lists the whole record.
- **Meaning never changes under one identity.** A changed meaning is a new version, and for a hash
  scheme a new name. Two peers that use different versions of one identity fail the agreement check
  at `start` with `binding_mismatch`.
- **Text trees print short names** and never versions; see [Plan notation](plan-notation.md#naming).
- **Enums are non-exhaustive.** Every public enum that names list members (`JoinType`, `Metric`,
  `NullRouting`, `PlanForm`, `Determinism`, `SideEffects`, ...) is `#[non_exhaustive]` in Rust, and
  C++ code that switches on one keeps a default branch, so a new member never breaks callers,
  closed list or not. The same holds for `ErrorCode` in C++ and the error enums in Rust.

[Open question 25](open-questions.md#25-open-registries-and-versioning) covers how members and
their identities are registered, how every process's engine learns of the same registry, and how
far plans built by one release must run on another. How plans travel between processes is
[question 1](open-questions.md#1-how-plans-travel).

## Reserved now: epochs

Every [message key](../part-5/07-failure-cancellation-and-recovery.md#every-message-has-a-key),
every end of stream and every value completion carries an **epoch**, always 0 today. A
receiver is complete for epoch e after one end of stream for e from every sender, and a value is
complete for e after one completion for e from every producing worker.

The reserved field lets iteration (recursive CTEs, iterative algorithms) and streaming arrive
later without changing what peers agree on today. A future `Iterate` node is a nested region whose
body runs once per epoch. Data and value edges inside it carry the epoch, and the edge that feeds
one iteration's result into the next goes from epoch e to e+1. V1 then holds per epoch: the graph of
one epoch stays acyclic. Until iteration exists, an engine that receives a nonzero epoch fails the
execution with `transport`, as for any protocol mismatch. See
[open question 24](open-questions.md#24-iteration-and-epochs).

## Shared subplans

A node with several consumers prints once, marked `#A`; later uses print `Ref #A` (see
[Plan notation](plan-notation.md)). The rules:

- **Evaluated once per worker.** Every consumer of a shared node on one worker sees the same rows,
  as if the node ran once on that worker. The engine may recompute a shared subtree instead of
  buffering its output when the whole subtree is `deterministic` or `per_execution` and `none` or
  `idempotent`, because that gives the same rows.
- **Never across workers.** All consumers of a shared node run on its set. Rows reach another set
  only through an `Exchange`.
- **Rejected until multicast is designed.** `Plan::create` rejects with `invalid_plan` a shared node
  whose subtree contains an `Input`, a `volatile` node or an `effectful` node. Recomputing any of
  them would be visible: an `Input` channel can be read only once, a `volatile` node gives
  different rows, and an `effectful` node repeats its effect. Sharing them needs an explicit
  evaluate-once-and-fan-out node, which is [open question
  10](open-questions.md#10-shared-subplans-and-multicast).
- **Except between the arms of one `Choose`.** A node whose consumers all sit in different arms of
  the same `Choose` has one reader at run time, the chosen arm, so it is accepted whatever its
  subtree contains. This lets a host's `Input` feed every arm ([Choosing between
  subtrees](../part-2/06-adaptive-plans.md#choosing-between-subtrees)).
- **Decorrelated subqueries are shared subplans.** The IR has no node for a correlated or
  decorrelated subquery. The outer relation is a shared node with two consumers. A keys-only
  `Aggregate` gives the subquery the distinct correlated values. An ordinary semi, anti, mark or
  inner join on null-safe key pairs brings the subquery's result back. The rules above apply to
  the shared node.

## Hash canonicalization in v1

`HashScheme::v1` hashes each key in the canonical form of its type, combined in key order; the
function and seed are fixed by v1. The form defines how a value hashes and when two values of one
type are equal. It is used only for hashing and comparison: no cast is inserted into the data.

| Key type | Canonical form | Equality |
|---|---|---|
| `boolean` | One byte, 0 or 1 | Value. |
| `int8`, `int16`, `int32`, `int64` | int64, sign-extended | Numeric. |
| `uint8`, `uint16`, `uint32`, `uint64` | uint64, zero-extended | Numeric. |
| `float32`, `float64` | float64, to which a `float32` widens exactly; -0.0 becomes +0.0, every NaN becomes one canonical NaN, then the IEEE bits | -0.0 equals +0.0; all NaNs are equal. |
| `decimal(p,s)` | 128-bit unscaled integer at scale s | Numeric. |
| `string` | The UTF-8 bytes | Byte-wise. No collation, case folding or trimming. |
| `date32` | Days since 1970-01-01 as int32 | Value. |
| `timestamp(unit)` | The stored int64 count of `unit` since 1970-01-01 00:00:00 | Value. |
| Extension types | The canonical form of the storage type | As the storage type. |
| null | A fixed null marker under `NullRouting::hashed`; under `to_all` the row goes to every worker | Not applicable. |

A form never lets two types meet. Both keys of a join pair have the same `DataType` (V4), and the
key types are part of the partitioning signature. Two exchanges whose keys differ in type are
never co-partitioned, even where the forms coincide, as for `int32` and `int64`. To bring two
types together, cast one side in a `Project`.

Each of these types may be a join key, a group key, a window key and a hash key. v1 defines no
other key type: `Context::check` and `prepare` reject any other, including `list`, `struct` and
every state type, with `unsupported` (V21). A new scheme may define more types; v1 never does.

The same types may be sort keys: in a `Sort`, a `TopN`, the order of a `Window`, a merging gather
and a range exchange. They order by the same canonical forms: false before true, numbers by
value, with -0.0 equal to +0.0 and NaN greater than every other number, dates and timestamps by
time, and strings byte-wise. A key's direction applies to that order, and its null placement says
where the nulls go. Expression comparisons, `min` and `max` use the same order
([Nulls and NaN](../part-1/04-schemas-and-expressions.md#nulls-and-nan)). The edge signature of a
range exchange carries each key's direction (`asc` or `desc`) and null placement (`nulls_first` or
`nulls_last`), next to the digest of its `Boundaries`. See
[open question 18](open-questions.md#18-hash-scheme-stability).

## Additive later

Each of these fits the [node contract](#the-node-contract) without changing the meaning of any plan
valid today. None is designed yet.

| Feature | How it fits | What it needs |
|---|---|---|
| More window functions and frames | Further forms of [`Window`](node-catalog.md#window), such as `lag`, `lead`, `ntile`, bounded frames and frames that share a value among peers, with the same input requirement. | A tie rule per form. A window without keys over a `ranged` input needs the `prefix` value mode ([question 29](open-questions.md#29-windows-over-a-global-order-prefix-value-mode)). |
| More data types | A time of day, a timestamp with a time zone, an interval, a binary string, a map, a fixed-size list and 128-bit integers, each as a new `DataType`. | Per type: its Arrow form and its comparison rule, and to be a key a canonical form in a hash scheme. Until then a host passes a map through as a list of key-value structs, a fixed-size list as a list, and a signed 128-bit integer as `decimal(38,0)`. |
| Unnest and table functions | A node whose function comes from the registry, with its own determinism class. | Output `arbitrary`; not key-preserving. |
| Sampling | A `Sample` node. A sample with a seed fixed in the plan is `deterministic`; one without is `volatile`. | Nothing new. |
| ASOF and inequality joins | New join types (an IR version). Inequality joins use a broadcast build and a nested-loop join ([question 17](open-questions.md#17-join-coverage)). | ASOF needs equality keys co-partitioned and each side sorted. |
| Writes with a commit step | A per-worker write node writes files named by attempt and produces a manifest. It is `idempotent`, because only files listed in a committed manifest become visible. The manifests are gathered to one worker, where an `effectful` commit node publishes them. | Retry re-runs writes, never the commit. The gather into the commit is spooled, so the commit sees only the manifests of completed attempts. |
| Refinable values | A value kind that publishes more than once, each time tighter, such as a top-N bound; consumers read the latest snapshot, as under `wait=never` ([Related designs](../appendix/related-designs.md#what-sirius-borrows)). | A flag on the value kind and a monotone merge rule. |
| Equivalence classes and constants | The property model learns that `a == b` after an inner join, or that a filter fixed a column to one constant, so more requirements are met without an exchange ([question 27](open-questions.md#27-requirements-as-data-and-equivalence-properties)). | No new node; derivation rules only. |
