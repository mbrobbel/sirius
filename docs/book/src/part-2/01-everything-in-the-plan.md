# Everything in the plan

On one GPU a join can read both of its inputs directly. On four GPUs, matching rows can be in
different places. The proposed plan records how to bring them together and which properties the
next operator may rely on.

## The ownership rule

> The plan owns every fact two workers must agree on, and every distribution or ordering property
> later nodes rely on. The engine owns how each edge is carried out, and any choice the plan leaves
> open.

The plan's side covers every value computed, the types at every boundary, every exchange and every
runtime value that crosses workers. What the plan fixes about one edge is its **logical edge**. How
the engine carries it out is its **physical shape**. [The logical edge and its physical
shape](#the-logical-edge-and-its-physical-shape) lists both.

The test: *what each receiving worker ends up with is in the plan; the route the bytes take to get
there is not.* The plan may deliberately leave row placement open. A round-robin exchange, for instance,
promises to distribute its input but gives a downstream join no guarantee about matching keys.

The following rules apply to plans built with the proposed API.

1. **Everything that moves is a node.** Every movement of rows between workers is an `Exchange`.
   A [worker](02-workers-and-distribution.md#workers-and-worker-sets) is bound to exactly one GPU,
   so that includes a move between two GPUs in one process. Every runtime value that crosses
   workers travels through a `ValueExchange`. The front-end places these nodes, by hand or through
   helpers and rewrites it calls. Sirius adds none and checks them all. The transport (same
   device, peer copy or network) follows from the bindings and belongs to the physical shape.
2. **If two workers must agree on it, it is in the plan.** Examples: the hash scheme, the partition
   count, filter kinds and geometry, range boundaries, broadcast versus partitioned, sender sets
   and aggregate state types.
3. **Runtime choices are plan values.** An adaptive choice is a `Decide` node with a pure
   policy and enumerated, labeled alternatives: exchange modes, partition counts, or whole
   subtrees under a `Choose`. The `ExecutionReport` records the chosen label.
4. **Bindings say where, never what.** Preparation binds logical workers to endpoints and named
   inputs to sources. It never changes nodes, routing, partitioning or a scan's shard rule.
5. **Builders may help; plans may not hide.** Convenience helpers expand into ordinary nodes, and
   `explain()` shows the expansion.
6. **Rewrites are plan-to-plan.** Every rewrite produces another plan in the same IR that you can
   print, diff and execute. Preparation applies no rewrites: a host that wants one calls it
   before it prepares.

## The questions a single-worker plan leaves open

Take the urgent-March-revenue query from the running example: total discounted revenue per order,
for orders placed in March 1995 with priority "1-URGENT". On one worker the plan looks like this.

```text
Plan "q4_urgent_march_revenue" (joined)
worker sets: local[1]
Output "result" on local[1]
└─ Aggregate keys=[o_orderkey, o_orderdate] measures=[revenue = sum(rev)]
   └─ Project [o_orderkey, o_orderdate, rev = l_extendedprice * (1 - l_discount)]
      └─ Join inner keys=[l_orderkey == o_orderkey] build=right  mode=single
         ├─ Scan lineitem [l_orderkey, l_extendedprice, l_discount]  on local[1]  shards=files_round_robin
         └─ Filter o_orderdate >= 1995-03-01 and o_orderdate < 1995-04-01 and o_orderpriority == "1-URGENT"
            └─ Scan orders [o_orderkey, o_orderdate, o_orderpriority]  on local[1]  shards=files_round_robin
```

Running it on `compute[4]` raises questions the tree leaves open. Each answer changes which worker
sees which rows, and in Sirius each answer is a node or an edge.

| Open question | Sirius answer |
|---|---|
| Which worker reads which `lineitem` files? | `Scan ... on compute[4]  shards=files_round_robin`: a shard rule on the leaf, resolved into per-worker split lists when the scan is built |
| Where do `lineitem` rows go before the join? | `Exchange x1 hash(l_orderkey) -> compute[4]` |
| Broadcast `orders`, or hash both sides? | The exchange mode on each join input; the join's `mode=` is derived from them |
| How many partitions, and which worker owns partition 3? | `Partitions` on the hash exchange; partition p goes to worker `p mod 4` |
| Which hash function, and over which key types? | `HashScheme::v1`, part of the exchange's partitioning signature |
| Can the `lineitem` scan skip rows using what `orders` found? | `RuntimeFilterBuild ~> f1`, `ValueExchange` to `f2`, `RuntimeFilterApply <~ f2` |
| Is the aggregate one step or two? | A single-phase `Aggregate`, legal because its input is `hashed` on a group key (V6) |
| Where does the result land, and how many readers are there? | `Exchange x3 gather -> head[1]` and `Output "result" on head[1]` |
| When does a limit stop upstream work? | A `Limit` node; when satisfied it closes its input, and close travels upstream |

The front-end's planner answers these questions and Sirius checks the answers. The same query,
with the answers in the tree:

```text
Plan "q4_urgent_march_revenue" (joined)
worker sets: head[1], compute[4]
values:
  f2 = ValueExchange merge f1 -> compute[4]
Output "result" on head[1]
└─ Exchange x3 gather -> head[1]
   └─ Aggregate keys=[o_orderkey, o_orderdate] measures=[revenue = sum(rev)]
      └─ Project [o_orderkey, o_orderdate, rev = l_extendedprice * (1 - l_discount)]
         └─ Join inner keys=[l_orderkey == o_orderkey] build=right  mode=co_partitioned(x1, x2)
            ├─ Exchange x1 hash(l_orderkey) -> compute[4]
            │  └─ RuntimeFilterApply [l_orderkey] <~ f2  wait=never
            │     └─ Scan lineitem [l_orderkey, l_extendedprice, l_discount]  on compute[4]  shards=files_round_robin
            └─ Exchange x2 hash(o_orderkey) -> compute[4]
               └─ RuntimeFilterBuild keys=[o_orderkey] spec=bloom(fixed 8MiB)  scope=partial ~> f1
                  └─ Filter o_orderdate >= 1995-03-01 and o_orderdate < 1995-04-01 and o_orderpriority == "1-URGENT"
                     └─ Scan orders [o_orderkey, o_orderdate, o_orderpriority]  on compute[4]  shards=files_round_robin
```

The aggregate needs no second exchange: the inner join's output is already hashed on `o_orderkey`,
one of its group keys. [The Distribution
property](02-workers-and-distribution.md#the-distribution-property) follows the distributions up
this tree.

## Three examples that motivated the design

Engines usually handle the next three cases behind the scenes. In Sirius each is a small piece of
the plan.

The worker sets `compute` and `head` and the `TableSource`s come from [The running
example](../part-3/00-running-example.md). The relations `urgent_orders` (the filtered orders
scan), `lineitem_rows`, `building` (the filtered customer scan) and `priced` (the projected
lineitem rows) are built as in Part I.

### Exchanging filter information between workers

A runtime filter built on one worker knows only the rows that worker saw. Before another worker
uses it, the filter must travel, and that trip is a node. Three lines of the plan above carry it:
the merge, the consumer on the lineitem side and the producer on the orders side.

```text
values:
  f2 = ValueExchange merge f1 -> compute[4]
RuntimeFilterApply [l_orderkey] <~ f2  wait=never
RuntimeFilterBuild keys=[o_orderkey] spec=bloom(fixed 8MiB)  scope=partial ~> f1
```

```cpp
#include <sirius/sirius.hpp>
using namespace sirius;

Result<Relation> probe_lineitem(const Relation& urgent_orders, const Relation& lineitem_rows,
                                const WorkerSet& compute) {
  SIRIUS_TRY(geometry, BloomGeometry::fixed(8 << 20));
  SIRIUS_TRY(build, urgent_orders.build_runtime_filter({"o_orderkey"},
                                                       RuntimeFilterSpec::bloom(geometry)));
  SIRIUS_TRY(merged, build.filter.merge_to(compute));   // inserts the ValueExchange: f1 -> f2
  // keep building the orders side from build.relation: it passes the rows on towards x2
  return lineitem_rows.apply_runtime_filter(merged, {"l_orderkey"});
}
```
```rust
use sirius::prelude::*;

fn probe_lineitem(urgent_orders: &Relation, lineitem_rows: &Relation, compute: &WorkerSet)
    -> Result<Relation, PlanError> {
    let geometry = BloomGeometry::fixed(8 << 20)?;
    let build = urgent_orders.build_runtime_filter(&["o_orderkey"],
                                                   RuntimeFilterSpec::bloom(geometry))?;
    let merged = build.filter.merge_to(compute)?;   // inserts the ValueExchange: f1 -> f2
    // keep building the orders side from build.relation: it passes the rows on towards x2
    lineitem_rows.apply_runtime_filter(&merged, &["l_orderkey"], WaitPolicy::never())
}
```

If you apply `build.filter` without `merge_to`, `Plan::create` rejects the plan (V16). The
filter is `partial`: it knows only the keys one worker saw. The merged `f2` knows every key, so it
is `global`. [Runtime filters](05-runtime-filters.md) covers
[scopes](05-runtime-filters.md#scope-which-rows-does-a-filter-know-about), kinds and waiting.

### A broadcast exchange

When one side of a join is tiny, sending all of it to every worker is cheaper than hashing the big
side. That copy is an `Exchange` with mode `broadcast`, and the join derives
`mode=broadcast_build` from its inputs.

```text
Join inner keys=[c_nationkey == n_nationkey] build=right  mode=broadcast_build
├─ Filter c_mktsegment == "BUILDING"
└─ Exchange x1 broadcast -> compute[4]
```

```cpp
SIRIUS_TRY(nations, Relation::scan(nation, ScanOptions::on(head)
                                               .with_columns({"n_nationkey", "n_name"})));
SIRIUS_TRY(everywhere, nations.exchange(Exchange::broadcast(compute)));
SIRIUS_TRY(on, JoinOn::keys({{"c_nationkey", "n_nationkey"}}));
SIRIUS_TRY(joined, building.join(everywhere, JoinType::inner, std::move(on),
                                 JoinOptions::defaults()));
```
```rust
let nations = Relation::scan(&nation, ScanOptions::on(&head)
                                          .with_columns(&["n_nationkey", "n_name"]))?;
let everywhere = nations.exchange(&Exchange::broadcast(&compute))?;
let on = JoinOn::keys(&[("c_nationkey", "n_nationkey")])?;
let joined = building.join(&everywhere, JoinType::Inner, on, JoinOptions::default())?;
```

A `right` join over the same inputs fails construction, because a broadcast build cannot be the
preserved side (V9); see [Broadcast](03-exchanges.md#broadcast). [Broadcast
join](../part-3/03-broadcast-join.md) walks through this query end to end.

### Partial and final aggregation across workers

A grouped aggregate on four workers needs every row of a group on the same worker. The usual
answer is to aggregate locally into small states, move the states, then finish. Both phases and
the move between them are nodes.

```text
Aggregate final keys=[l_returnflag, l_linestatus]
└─ Exchange x1 hash(l_returnflag, l_linestatus) -> compute[4]
   └─ Aggregate partial keys=[l_returnflag, l_linestatus] ...
```

```cpp
SIRIUS_TRY(partial, priced.aggregate_partial({"l_returnflag", "l_linestatus"}, measures));
SIRIUS_TRY(moved, partial.exchange(Exchange::hash({"l_returnflag", "l_linestatus"}, compute)));
SIRIUS_TRY(totals, moved.aggregate_final());
```
```rust
let partial = priced.aggregate_partial(&["l_returnflag", "l_linestatus"], measures)?;
let moved = partial.exchange(&Exchange::hash(&["l_returnflag", "l_linestatus"], &compute,
                                             Partitions::one_per_worker(), HashScheme::v1()))?;
let totals = moved.aggregate_final()?;
```

The partial aggregate's output schema carries *state types* such as `sum_state<decimal(38,2)>`.
The bytes that cross the exchange therefore have a declared layout, and the final aggregate needs
only that schema. [State types travel in the
schema](03-exchanges.md#state-types-travel-in-the-schema) lists the types, and [Two-phase
aggregation, end to end](03-exchanges.md#two-phase-aggregation-end-to-end) builds this example in
full.

## Routing moves into the plan

Some execution APIs route outputs at prepare time: you tell `prepare` whether the output goes to
a single place, is broadcast, or is hash partitioned across destinations. Where result rows go
decides what each receiving worker ends up with, so Sirius keeps routing in the plan. Preparation
only binds readers to the `Output` workers the plan names.

| Routing at prepare time | Sirius (routing in the plan) | Readers |
|---|---|---|
| `OutputRouting::single()` | `Exchange::gather(head)`, then `Output` on `head[1]` | One, in the process hosting `head#0` |
| `OutputRouting::broadcast(destinations)` | `Exchange::broadcast(consumers)`, then `Output` on `consumers[n]` | One per worker of `consumers`; each sees every row |
| `OutputRouting::hash_partitioned(destinations, keys)` | `Exchange::hash(keys, consumers)`, then `Output` on `consumers[n]` | One per worker; each sees its partition |
| (no counterpart) | No exchange; `Output` on `compute[4]` | Four; each sees what its worker produced |
| `Destinations` | A `WorkerSet` in the plan, plus a `WorkerBinding` in the bindings | n/a |
| `InputBindings` | `BindingsBuilder::input(name, worker, source)` | n/a |

`Output` gives one `OutputReader` per worker of its set, delivered to the process hosting that
worker ([Readers go where the Output
lives](04-stages-and-bindings.md#readers-go-where-the-output-lives)). The exchange in front of
`Output` sets the shape of the result.

```cpp
SIRIUS_TRY(gathered, revenue_rows.exchange(Exchange::gather(head)));
SIRIUS_TRY(plan, Plan::from_root(gathered));          // adds Output "result" on head[1]
SIRIUS_TRY(prepared, context.prepare(plan, Bindings::local()));
SIRIUS_TRY(started, std::move(prepared).start());
for (auto& out : started.outputs) {
  // out.output == "result", out.worker is head#0: exactly one reader
}
```
```rust
let gathered = revenue_rows.exchange(&Exchange::gather(&head, GatherOrder::any()))?;
let plan = Plan::from_root(&gathered, "result")?;     // adds Output "result" on head[1]
let prepared = context.prepare(&plan, Bindings::local())?;
let started = prepared.start()?;
for out in &started.outputs {
    // out.output == "result", out.worker is head#0: exactly one reader
}
```

Producer counts stay out of the plan. How many writers feed a channel is a property of the channel,
so `ProducerCount` belongs to `InputChannel`, the live input that comes later.

## What stays implicit

The engine owns three things: how each edge is carried out, how one worker processes the rows it
holds, and the choices the plan leaves open.

### The logical edge and its physical shape

| The plan owns the logical edge | The engine owns its physical shape |
|---|---|
| Mode, keys, hash scheme, partition count, null routing and durability | Transport, chunking and compression |
| Target set, sender set and ordered destinations | Flow control, credits and connection pooling; where a spool lives |
| For a value edge: kind, merge rule and wait policy | Direct delivery or forwarding through an intermediate worker, and the shape of a merge or a shuffle |

[Exchanges in the
engine](../part-5/02-exchanges-in-the-engine.md#the-logical-edge-and-its-physical-shape) has the
full table for exchanges.

Two edges of the Q4 plan above show the difference.

- `f2 = ValueExchange merge f1 -> compute[4]` says that every compute worker ends up with the OR of
  all four partial filters. On two hosts, the engine may OR the two shares on each host and send
  one filter per host over the network, or reduce the shares in a tree or a ring. Bitwise OR
  is associative and commutative, so every worker ends up with the same `f2`.
- `Exchange x1 hash(l_orderkey) -> compute[4]` says that partition p lands on `compute#(p mod 4)`.
  The engine may run it as a two-level shuffle: between the GPUs of one host over NVLink, then
  across the network. `compute#2` still ends up with exactly partition 2.

Combining partial aggregate states on each host before they cross the network looks similar but
belongs to the plan. It changes which rows cross a worker boundary, so it is a plan rewrite that
inserts an explicit combine phase (`aggregate_combine`).

### Inside one worker

The engine owns everything that happens to the rows a worker holds.

- **Kernels and algorithm variants.** Which hash table, which sort, whether a join builds once and
  probes in place or splits itself into pieces inside the worker. The plan can fix a worker-local
  property such as a join's build side, or leave it `auto` for preparation to resolve
  ([Preparing a plan](../part-1/06-prepare-and-execute.md#preparing-a-plan)).
- **Batch sizes.** How many rows flow through an operator at a time.
- **Task scheduling.** Which pipeline runs next and on which stream.
- **Pipeline fusion.** Which nodes run back to back without materializing in between.
- **Memory tiers and spilling.** Moving data between GPU memory, pinned host memory and disk.
- **Local sub-partitioning inside one worker.** Splitting a worker's rows into smaller pieces to fit
  memory, for example to run a join or a sort out of core
  ([The seed rule](../part-5/02-exchanges-in-the-engine.md#the-seed-rule)).
- **The choice among a filter's allowed kinds, while the filter never leaves one worker.** Once a
  filter crosses workers the plan fixes its allowed kinds and merge rule so receivers can interpret it.

### Where the plan says `arbitrary`

Where a distribution is `arbitrary`, the plan states only that each worker ends up with some share
of the rows. The engine or the host picks which. Batch boundaries, which rows a `round_robin`
exchange sends to which worker, and which *n* rows a per-worker `Limit` keeps are such choices. No
later node can rely on them.

Results are therefore deterministic only up to the nondeterminism SQL already allows: row order
without `ORDER BY`, which rows a `LIMIT` returns without `ORDER BY`, which of two rows with equal
order keys a window numbers first, and floating-point summation order. Within those limits, engine
choices change only speed and memory use, so the engine can improve them without touching a plan
([Inside the engine](../part-5/01-from-plan-to-pipelines.md)).

## One plan, many forms

A plan passes through several *forms* on its way to execution. Every form is the same IR (rule 6),
so you can print and diff any two.

```mermaid
flowchart TB
  A["front-end planner"] -->|"writes, with its exchanges"| B["joined plan: Exchange nodes"]
  B -->|"rewrite::split"| C["split plan: one block per stage"]
  C -->|"rewrite::instance"| D["instance plan: one worker's view"]
  B -->|"Context::prepare + start"| E["Execution"]
  E -->|"report.resolved_plan()"| F["resolved plan: decisions substituted"]
```

- The **joined plan** is what the front-end writes, with every exchange in it. Validation rejects
  a missing or misplaced exchange with an error that says what to insert
  ([Validation](07-validation.md)).
- The **single-worker plan** of Part I is a joined plan whose only set is `local[1]`. It runs as
  is. Whether a helper should spread one over the GPUs of one machine is [open question
  36](../reference/open-questions.md#36-a-distribution-helper-for-one-machine).
- `rewrite::split` cuts the joined plan at every exchange into **stages**. Each exchange becomes an
  `ExchangeSink` at the root of the sending stage and an `ExchangeSource` in the receiving stage
  ([Stages and bindings](04-stages-and-bindings.md)).
- `rewrite::instance` shows one worker's view, with concrete files and partitions.
  `Plan::create_instance` builds the same form directly, for a host that plans one worker at a
  time.
- After execution, `resolved_plan()` gives the **resolved plan**, where every adaptive decision is
  replaced by the chosen alternative ([Adaptive plans](06-adaptive-plans.md)).

## Three hosts, one plan

Part IV proposes a common preparation API for three hosts. Each would supply a different view of the plan.

- sirius-duckdb translates the host's optimized plan with the builder.
- StarRocks expresses each fragment instance its scheduler placed as an instance plan.
- Every Distributed Sirius process builds the same joined plan, with no coordinator.

For the integration details see [sirius-duckdb](../part-4/01-duckdb-extension.md), [StarRocks](../part-4/02-starrocks.md),
[Distributed Sirius](../part-4/03-distributed-sirius.md), and [what they have in
common](../part-4/04-one-ir-three-integrations.md).
