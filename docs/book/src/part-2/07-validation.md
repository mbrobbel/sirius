# Validation

Sirius states its plan rules as numbered **invariants**, V1 to V28. This chapter lists each with
its reason and the error a violation returns. The plan holds every fact two workers must agree on,
so most checks need only the plan.

## Three gates

Checks run at three points, in this order:

| Gate | Invariants | Error code | Needs |
|---|---|---|---|
| `Plan::create` (and `Plan::from_root`) | V1-V20, V24, V26-V28 | `invalid_plan` | nothing: no GPU, no context |
| `Context::check` | V21-V22 | `unsupported` | a context, so it knows this engine build |
| `Context::prepare` and start | V21-V22 again and V23 at `prepare`; V25 at start | `unsupported`, `binding_mismatch` | bindings, so it knows the bound devices, and at start, the peers |

The catalog below groups V24 with the worker rules, V26 with the exchange and decision rules, and
V27 and V28 with the distribution rules.

**A `Plan` value is always valid.** If `Plan::create` returns a plan, V1-V20, V24 and V26-V28 hold
on every form of it. You can build and check plans in a unit test or a planning service with no
GPUs.

Builder methods check what is local to the node they create. Column scope, types and key names
(V4, V5) fail on the call that introduces them. A node that cannot derive its output, such as a
join whose inputs sit on two different sets, fails at once with a hint to insert an exchange.

Rules that span more than one node are checked at `Plan::create`: a filter's apply and the join
that feeds it (V18), a decision and every set it controls (V15), both sides of a co-partitioned
join (V20).

Rewrites pass the same gate. Every `rewrite::` function returns `Result<Plan>`, validated like any
other plan (rule 6), so a rewrite cannot produce a plan you could not have built.

## Reading an error

Every error has a code and a message. Errors from the plan gates add a `PlanLocation`: the `NodeId`
of the offending node and, when the node belongs to a stage, its `StageId`. Errors from a running
execution also carry the worker they happened on.

In C++ these are the fields `code`, `message`, `location` and `worker` of `Error`. In Rust the
variant is the code and holds the rest, for example `PlanError::Invalid` at the first gate
([API at a glance](../reference/api-at-a-glance.md#errors) lists every code with its variant).

In this mistake, the lineitem side is hashed into 16 partitions and the orders side into the
default, one partition per worker:

```cpp
#include <sirius/sirius.hpp>
using namespace sirius;

Result<void> mismatched(const Relation& lineitem, const Relation& urgent,
                        const WorkerSet& compute, const WorkerSet& head) {
  SIRIUS_TRY(sixteen, Partitions::fixed(16));
  SIRIUS_TRY(left, lineitem.exchange(Exchange::hash({"l_orderkey"}, compute, sixteen)));
  SIRIUS_TRY(right, urgent.exchange(Exchange::hash({"o_orderkey"}, compute)));
  SIRIUS_TRY(on, JoinOn::keys({{"l_orderkey", "o_orderkey"}}));
  SIRIUS_TRY(joined, left.join(right, JoinType::inner, std::move(on), JoinOptions::defaults()));
  SIRIUS_TRY(gathered, joined.exchange(Exchange::gather(head)));

  auto plan = Plan::from_root(gathered);
  if (!plan && plan.error().code == ErrorCode::invalid_plan) {
    const std::optional<PlanLocation>& where = plan.error().location;   // the Join, stage S3
    std::cerr << plan.error().message << "\n";
  }
  return {};
}
```
```rust
use sirius::prelude::*;

fn mismatched(lineitem: &Relation, urgent: &Relation, compute: &WorkerSet, head: &WorkerSet)
    -> Result<(), PlanError> {
    let sixteen = Partitions::fixed(16)?;
    let left = lineitem.exchange(&Exchange::hash(&["l_orderkey"], compute, sixteen,
                                                 HashScheme::v1()))?;
    let right = urgent.exchange(&Exchange::hash(&["o_orderkey"], compute,
                                                Partitions::one_per_worker(), HashScheme::v1()))?;
    let on = JoinOn::keys(&[("l_orderkey", "o_orderkey")])?;
    let joined = left.join(&right, JoinType::Inner, on, JoinOptions::default())?;
    let gathered = joined.exchange(&Exchange::gather(head, GatherOrder::any()))?;

    match Plan::from_root(&gathered, "result") {
        Err(PlanError::Invalid { message, location, .. }) => {
            let _where = location; // the Join, stage S3
            eprintln!("{message}");
        }
        _ => {}
    }
    Ok(())
}
```

The message names the invariant, the node and the fix:

```text
invalid_plan: V8: Join inner keys=[l_orderkey == o_orderkey] has no legal mode: both inputs are
  hashed, but not co-partitioned. x1 hash(l_orderkey) partitions=16 -> compute[4] and
  x2 hash(o_orderkey) -> compute[4] (4 partitions) have different partitioning signatures.
  Use the same Partitions on both exchanges.
  at: Join inner keys=[l_orderkey == o_orderkey] (stage S3)
```

A tool can use the location to highlight the node in `explain()` output without parsing the
message. The code, the leading V-number and the location are part of the API. The wording of the
message is not.

## The invariants

Each entry gives the rule, its reason and an example error. The examples use the running example's
tables and worker sets.

### Structure and names (`Plan::create`)

**V1. The wait-for graph is acyclic.**

- *Why:* a cycle means some node waits for its own output. The builder alone cannot make one,
  because every handle exists before its consumer. A cycle can appear when `rewrite::join_stages`
  composes stage or instance plans by matching ids.
- *Example* (with host-chosen ids, as in open plans): `invalid_plan: V1: cycle through value
  f22 and exchange x12: RuntimeFilterApply <~ f22 feeds ExchangeSink x12, whose rows reach the
  RuntimeFilterBuild that produces f21.`
- *Waits:* the **wait-for graph** holds the data and value edges, the ordered waits, and a credit
  wait from each producer into each streaming queue it feeds, because with bounded buffers a
  sender can also wait for its receiver. Deadlock freedom within one execution is therefore a
  property of the plan
  ([Credits and deadlock freedom](../part-5/08-running-at-scale.md#credits-and-deadlock-freedom)).

**V2. Every referenced set is declared. All nodes of a stage share one set.**

- *Why:* a stage runs as one instance per worker of one set, so a stage spanning two sets has no
  instances. A set name must also mean one size everywhere.
- *Example:* `invalid_plan: V2: worker set "compute" is used with sizes 4 and 8.`

**V3. Output names are unique. Inputs are unique by name. Exchange and value ids are unique.**

- *Why:* readers, input bindings and wire messages are addressed by these names and ids.
- *Example:* `invalid_plan: V3: two Output nodes are named "result".`

### Scope and types (`Plan::create`, usually caught by the builder)

**V4. Expressions reference only their input scope. Operands of arithmetic and comparisons have
the same type. Predicates are boolean. Both keys of a join pair have the same type, and union
inputs have identical types.**

- *Why:* a column from another relation has no value here. No type is converted implicitly, so
  the plan shows every cast, and equal keys of one type hash to the same partition on both sides.
- *Example:* `invalid_plan: V4: join key pair (l_linenumber int32, o_orderkey int64) has different
  types; cast one side in a Project first.`

**V5. A combine or final input is exactly the keys plus the state columns of one partial
signature.**

- *Why:* a final aggregate has no measure list. It reads the measures from the state types in its
  input, so any other column between the phases makes them ambiguous. The grouping id of an
  aggregate with grouping sets has no state type, so the later phases read it as a key.
- *Example:* `invalid_plan: V5: Aggregate final input column revenue decimal(38,4) is not a state
  column; project after the final aggregate, not between the phases.`

### Distribution (`Plan::create`)

**V6. The input of a grouped single-phase or final aggregate is `single`, or `hashed` on a subset
of the group keys. For a single-phase aggregate with grouping sets the group keys are those in
every set; for a final they include the grouping id.**

- *Why:* a one-phase grouped aggregate must see every row of a group on one worker. With grouping
  sets that must hold for every set at once, so only keys in every set help, and a `()` set needs
  all rows on one worker.
- *Example:* `invalid_plan: V6: Aggregate keys=[o_orderkey, o_orderdate] reads
  arbitrary(compute[4]); a group's rows may be on several workers. Use aggregate_partial and
  aggregate_final with an exchange, or hash on the keys first.`
- *Example* (with grouping sets): `invalid_plan: V6: Aggregate keys=[l_returnflag, l_linestatus]
  sets=[(l_returnflag, l_linestatus), (l_returnflag), ()] reads hashed(l_returnflag) on
  compute[4]; no key is in every set, so the rows of () may be on several workers. Use
  aggregate_partial_sets and aggregate_final with an exchange on the keys and grouping_id.`

**V7. A partial or single-phase aggregate never consumes `replicated` rows on a set larger than
one.**

- *Why:* every worker holds every row, so each worker would count it again.
- *Example:* `invalid_plan: V7: Aggregate partial reads replicated(compute[4]) from Scan nation
  shards=replicated; every row would be counted 4 times.`

**V8. A join is co_partitioned, broadcast_build, single or decided.**

- *Why:* rows with equal keys must meet on one worker. These four modes guarantee it.
- *Example:* the co-partitioning error above.

**V9. A join that preserves its build side is never broadcast_build.**

- *Why:* each worker sees only its own probe rows, so it cannot know whether a build row matched
  on another worker. Unmatched build rows would be emitted once per worker, some of them wrongly.
- *Example:* `invalid_plan: V9: Join right keys=[c_nationkey == n_nationkey] build=right preserves
  build rows, so its build cannot be broadcast; hash both sides instead.`

**V10. `mark` with a co-partitioned build and a plain key pair requires exactly one key pair, (a)
`NullRouting::to_all` on the build exchange and (b) a global `rows` Statistics of the build input
through `with_build_rows`. Decided joins are checked per label, and both exchanges use the same
Decision.**

- *Rule:* V10 applies when the join has a plain key pair. A mark join whose pairs are all
  null-safe never answers null and is checked by V8 alone. The build is the join's right input,
  as for every `mark` join. (a) is the one signature difference co-partitioning allows: the build
  exchange routes nulls `to_all`, the probe exchange `hashed`, and every other field is equal.
  For (b), the value comes from an `Observe rows()` directly below the build exchange, with
  nothing between them that can change whether the input is empty (no `Filter` or `Limit`), merged
  into a global value.
- *Why:* with one key pair, a mark join's null and false answers depend on whether the whole build
  holds a null key and whether it is empty
  ([Mark joins need the whole build](03-exchanges.md#mark-joins-need-the-whole-build)). A join on
  several key pairs uses a broadcast or single build, which needs neither (a) nor (b). A decided
  join must be legal under every label it might take.
- *Example:* `invalid_plan: V10: mark Join keys=[c_custkey == o_custkey] is co_partitioned, but it
  reads no global row count of its build input; observe rows() below x2, merge it with
  merge_to(compute), and pass it with JoinOptions::with_build_rows.` Without (a):
  `invalid_plan: V10: mark Join is co_partitioned, but build exchange x2 routes null keys with
  NullRouting::hashed; use HashScheme::v1().with_nulls(NullRouting::to_all).`
- *Plan:* the fixed join reads the value next to its build side:

```text
Join mark keys=[c_custkey == o_custkey] build=right  build_rows<~s2  mode=co_partitioned(x1, x2)
```

**V11. `Union` and `Output` reject a mix of `replicated` and non-replicated inputs.**

- *Why:* the replicated input would appear once per worker next to the other input's single copy.
- *Example:* `invalid_plan: V11: Union input 1 is replicated(compute[4]) but input 0 is
  arbitrary(compute[4]).`

**V27. The input of a `Window` with keys is `single`, or `hashed` on a subset of the window keys.
The input of a `Window` without keys is `single`.**

- *Why:* a function's value for a row depends on the other rows of its window group, so a group's
  rows must be on one worker. Rows with null keys form a group like any other. Without keys, all
  rows are one group.
- *Example:* `invalid_plan: V27: Window ranking keys=[o_custkey] reads arbitrary(compute[4]); a
  window group's rows may be on several workers. Hash on the window keys first.` Without keys:
  `invalid_plan: V27: Window running order=[o_orderdate asc, o_orderkey asc] has no keys, so all
  rows are one window group, but its input is hashed(o_custkey) on compute[4]. Gather to one
  worker first.`

**V28. A hash exchange with `NullRouting::to_all` is read only by a `mark` join, as its build
input.**

- *Rule:* the exchange is itself the join's build input, which is its right input, with no node
  between them; in split form and in open plans the join reads its `ExchangeSource`. A decided
  exchange with a `to_all` alternative counts as a `to_all` exchange. The rule holds on a set of
  size 1 as well. The builder call that reads the exchange reports the error.
- *Why:* `to_all` puts every row with a null key on every worker, which the build of a mark join
  needs ([Mark joins need the whole build](03-exchanges.md#mark-joins-need-the-whole-build)). Any
  other reader would see such a row once per worker: an aggregate would count it on each, a join
  would match or emit it on each, and a `Window` or a per-key `TopN` would compute its group on
  each.
- *Example:* `invalid_plan: V28: Aggregate final keys=[o_custkey] reads x1 hash(o_custkey)
  nulls=to_all; rows with a null key are on every worker. Only the build input of a mark join
  reads a to_all exchange; use NullRouting::hashed.`

### Runtime values and ordering (`Plan::create`)

**V12. A value edge between different sets, or from a non-global scope to another worker, passes
through a `ValueExchange`.**

- *Why:* a value lives on the worker that produced it. Moving it is communication, and
  communication is a node (rule 1). The engine chooses how the shares travel, directly or reduced
  as a tree.
- *Example:* `invalid_plan: V12: f1 is produced on head[1] but RuntimeFilterApply [c_nationkey]
  runs on compute[4]; deliver it with merge_to(compute).`

**V13. Merged filter partials share kinds, geometry (`fixed`, or `sized_by` the same global
statistic), hash, key domain and null rule.**

- *Why:* Bloom filters can only be ORed bit by bit, and in-lists only unioned, when every worker
  built them the same way.
- *Example:* `invalid_plan: V13: f1 is merged, but its geometry is sized_by s1, which is partial;
  each worker would size its own bit array. Merge s1 first.`

**V14. Global TopN, a global Limit (over sorted input or not) and an ordered Output sit on
`single`. A global per-key TopN sits on `single`, or on `hashed` on a subset of its keys.
`gather merge` input is sorted on its keys. `range` uses a global Boundaries on the same
keys.**

- *Why:* "the top 10" over four workers is 40 rows, and so is "the first 10 in order" of four
  sorted workers. "The top order per customer" is right only where all of a customer's rows are.
  A merge of unsorted streams is not sorted. Range partitions line up only when every worker uses
  the same splitters.
- *Example:* `invalid_plan: V14: Exchange x2 gather order=merge[o_orderdate asc] reads input that is
  not sorted on [o_orderdate asc]; add a Sort below it.`

**V15. A `Decide`, `ComputeBoundaries` or `AssignPartitions` input is global on every set it
controls.**

- *Why:* without a coordinator, every worker must compute the same label, splitters or
  partition-to-worker map. A `Decide` on a leader such as `head[1]` reads a value that is global
  there, and its `Decision` reaches the controlled set through a one-sender `ValueExchange`, which
  is global by construction.
- *Example:* `invalid_plan: V15: Decide threshold(s1.bytes) reads s1, which is partial on
  compute[4]; workers could pick different labels. Use s1.merge_to(compute).`

**V16. A `partial` filter is never applied.**

- *Why:* a partial filter knows only the keys its own worker saw, so it would drop rows whose match
  lives on another worker.
- *Example:* `invalid_plan: V16: RuntimeFilterApply [l_orderkey] <~ f1 uses a partial filter;
  merge it first with merge_to(compute).`

**V17. A `partition(xK)` filter is applied only on the same set, to a relation `hashed` with xK's
signature on the corresponding columns, on the same worker.**

- *Why:* a partition filter is complete only for the keys of its own partition.
- *Example:* `invalid_plan: V17: runtime filter f1 has scope partition(x2), so it can only be
  applied on the same worker to rows hashed like x2; RuntimeFilterApply [l_orderkey] reads
  arbitrary(compute[4]) rows from Scan lineitem.`

**V18. A filter is applied only on the probe-side lineage of the join whose build feeds it. The
lineage passes only key-preserving nodes (Filter, plain Project, and Exchange of any mode,
including decided exchanges), and the join type discards non-matching probe rows. A filter with a
key from a null-safe pair of that join has `NullKeys::pass`.**

- *Why:* dropping a row early is harmless only if the join would have dropped it anyway and the
  filtered column is the value the join compares. A plain pair discards a null key, but a
  null-safe pair can match it, so there the filter must pass it.
- *Example:* `invalid_plan: V18: f2 is applied on the probe side of Join left keys=[l_orderkey ==
  o_orderkey]; a left join keeps probe rows without a match, so filtering them changes the result.`
- *Example* (with a null-safe pair): `invalid_plan: V18: f1 has key o_custkey, the build side of
  the null-safe pair c_custkey <=> o_custkey, but discards rows with a null key; build it with
  RuntimeFilterSpec::with_null_keys(NullKeys::pass).`

### Exchanges and decisions (`Plan::create`)

**V19. `forward` has T = S. `gather` has |T| = 1. All alternatives of a decided exchange share T.**

- *Why:* forward moves nothing, a gather ends on one worker, and a decided exchange must keep the
  same stage graph under every label.
- *Example:* `invalid_plan: V19: Exchange x2 gather -> compute[4] targets four workers; a gather
  goes to a set of size 1.`

**V20. Partitions decided by a Decision, or assigned by an Assignment, are used identically on both
sides of a co-partitioned join.**

- *Why:* the join pairs partition *p* with partition *p*, so both sides need the same number of
  partitions and must send partition *p* to the same worker. Both sides of a join over
  `Partitions::assigned_by(a1)` use the same `a1`.
- *Example:* `invalid_plan: V20: x1 uses partitions=<~d2 but x2 uses one_per_worker; a
  co-partitioned join needs the same decided partitions on both sides.`

**V26. A `Choose` has one arm per label of its Decision, all with the same schema. Its handles are
the Decision and one-sender deliveries of it, global together on every set that runs a node of any
arm. The Decision's inputs come from outside every arm, and a value made inside an arm is consumed
only inside it.**

- *Why:* every worker that might run an arm must know the label without asking anyone. An arm
  cannot wait for a decision that waits for it, and an arm that is not chosen never publishes a
  value. Each arm is checked as a plan of its own, and the facts above the `Choose` are the meet
  of the arms.
- *Example:* `invalid_plan: V26: Choose <~ d1 has an arm with Aggregate final on compute[4], but d1
  is global only on head[1]; deliver it with d1.merge_to(compute) and pass the delivery to choose.`

Three construction checks have no number. `Plan::create` rejects each with `invalid_plan`:

- **A shared node whose subtree contains an `Input`, a `volatile` node or an `effectful` node**,
  unless its consumers all sit in different arms of one `Choose`, so only the chosen arm reads it
  at run time. *Why:* the engine may recompute a shared node for each consumer instead of
  buffering it, and for such a subtree the two give different results
  ([Extension points](../reference/extension-points.md#shared-subplans)).
- **An alternative of a decided exchange that sets its own durability.** *Why:* durability is set
  once, on the result of `decided_by`.
- **A join whose build side is `auto` while its mode is `broadcast_build` or `decided`.** *Why:*
  there the build side follows from the exchanges in the plan, so it is not local to one worker
  ([The build side](../part-1/07-aggregates-joins-and-ordering.md#the-build-side)).

### Engine support (`check` and `prepare`)

**V21. Every node, type, function and value kind is supported by this engine build, including types
that cross exchanges.**

- *Why:* a valid plan can still ask for something this build cannot run. Reporting that before
  anything starts lets a host keep its own plan. The check covers a hash or range key whose type
  the hash scheme does not define
  ([Hash canonicalization in v1](../reference/extension-points.md#hash-canonicalization-in-v1)).
  A name unknown to the `Registry` used to build the plan fails earlier, at construction, with
  `invalid_plan`.
- *Example:* `unsupported: V21: Exchange x1 carries column customers distinct_state<int64>, which
  this build cannot send across an exchange; plan it as a two-level grouping.`

**V22. Registered scan kinds are implemented.**

- *Why:* a host-registered scan kind comes from the registry used to build the plan, but it runs
  only if this engine implements it.
- *Example:* `unsupported: V22: scan kind "warehouse.native" has no implementation in this engine.`

```cpp
auto support = context.check(plan);
if (!support && support.error().code == ErrorCode::unsupported) {
  std::cerr << support.error().message << "\n";   // keep the host's own plan
}
```
```rust
if let Err(PrepareError::Unsupported { message, .. }) = context.check(&plan) {
    eprintln!("{message}"); // keep the host's own plan
}
```

`prepare` repeats the check on the devices it binds and also returns `unsupported`. Sirius never
replans or falls back.

### Workers, sizes and peers (`prepare`; V24 at `Plan::create`)

**V23. Every worker of every set touched by a hosted stage is bound exactly once to an existing
endpoint; of an unsized hosting set, only the hosting worker. Every `Input` instance is bound.**

- *Why:* a stage instance needs an endpoint to run on, and its peers need an address for every
  worker they exchange with. An unsized set names no other workers, so there is nothing else to
  bind.
- *Example:* `binding_mismatch: V23: compute#3 is not bound to any endpoint.`

**V24. A set is sized wherever the plan depends on its size.**

- *Why:* only the hosting set of an instance plan may be unsized (`frag1[?]`). Its worker runs one
  stage instance and needs to know nothing about its siblings, unless the plan depends on how
  many there are. These sets are sized:
  - every sender set of an `ExchangeSource` or `ValueSource`, which counts one EOS or one share per
    sender;
  - every target set of an exchange or value edge the plan sends on, because the sender routes by
    its size: a gather needs one target (V19), a hash or range exchange a partition count. The
    hosting set counts too when its plan receives a partitioned or gathered exchange, so that
    receiver and senders agree on "partition k of N";
  - any set whose scans use a shard rule (`files_round_robin`, `byte_ranges`, `explicit_splits`,
    `dynamic`), which divides the files over the workers or lets them claim splits;
  - any set a `Decide`, `ComputeBoundaries` or `AssignPartitions` controls, and any set that is
    both sender and target of one edge (`forward`, or `merge_to` its own set).
- *Example:* `invalid_plan: V24: Scan lineitem uses files_round_robin on frag1[?]; a shard rule
  needs a sized set.`

**V25. Peers agree at start.**

- *Why:* without a coordinator, peers check agreement and never negotiate it. Closed plans
  compare the execution id, plan fingerprint and bindings digest (which covers the binding epoch
  after a retry). Open plans compare the **edge signature** of each exchange and value edge ([Every
  process prepares the same plan](04-stages-and-bindings.md#every-process-prepares-the-same-plan)),
  which includes the partition assignment, plus the version identities listed in
  [Extension points](../reference/extension-points.md#versioning). Peers agree per edge: the
  membership of a side that only sends is never compared, so an instance on `frag1[?]` and the
  receivers on `frag3[4]` agree without the sender knowing its siblings.
- *Example:* `binding_mismatch: V25: the peer hosting compute#2 prepared a plan with a different
  fingerprint.` In an open plan: `binding_mismatch: V25: exchange x11: frag1#2 hashes into 16
  partitions, but frag3#2 expects 4.`

## What validation does not check

The invariants guarantee that a plan means something and that every worker agrees on what it
means. They do not check plan quality: a broadcast of a huge table or a filter that prunes nothing
passes. The `ExecutionReport` is where you find those.

Whether the files match the schemas and sizes a plan declares is not a plan property. The engine
checks each file when a worker opens it and fails with `source_mismatch`
([The source check](../part-1/05-relations-and-plans.md#the-source-check)).

Failures that appear only at run time, such as an I/O error, a device error the engine recovers
from, or a required value that fails, are `execution` errors with the worker and location they
happened at. A fatal device error makes the engine unavailable for the whole process: every later
call on a handle returns `runtime_unavailable` until the process restarts
([How long a context lives](../part-1/03-context-and-session.md#how-long-a-context-lives)).

One kind of agreement can be checked only while the plan runs. V15 makes every worker decide from
a global value, but the label is computed at run time. The label on a decided exchange and every
delivered `Boundaries` or `Assignment` value therefore travel with a digest, and a receiver that
sees a digest other than its own fails with `execution` ([Why every worker picks the same
label](06-adaptive-plans.md#why-every-worker-picks-the-same-label)).
