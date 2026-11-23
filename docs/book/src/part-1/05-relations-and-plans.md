# Relations and plans

A `Relation` is one node of a query with the schema it produces. A `Plan` is a set of relations
that ends in one or more `Output` nodes, checked as a whole and ready to prepare on a context.

This chapter builds a small plan over the `orders` table, on one worker.

## What a relation is

A `Relation` is a builder handle for one node of the Sirius Plan IR:

- **It is complete.** Every factory takes everything the node needs and returns a valid relation
  or an error. There is no `set_input()` or `init()`.
- **It is immutable.** `filter` returns a new relation whose input is the old one. The old handle
  stays usable.
- **It retains its inputs.** You can drop intermediate handles.

A relation knows these facts about itself, without a GPU or a context:

| Fact | Accessor | What it tells you |
|---|---|---|
| Output schema | `schema()` | The fields this node produces, in order. |
| Worker set | `workers()` | Which logical workers run this node. |
| Distribution | `distribution()` | How the rows are spread over those workers. |
| Column scope | `column(name)` | A scoped `Expression` for one of its output columns. |

Column references belong to the relation they came from; see
[Columns are scoped](04-schemas-and-expressions.md#columns-are-scoped).

## Declaring a table

A scan needs a `TableSource`, and a table source needs an explicit schema
([Fields and schemas](04-schemas-and-expressions.md#fields-and-schemas)). Building a plan therefore
never opens a file, lists a directory or contacts a catalog.

This is the `orders` table of the [running example](../part-3/00-running-example.md), a subset of
the TPC-H schema:

```cpp
#include <sirius/sirius.hpp>
using namespace sirius;

Result<Schema> orders_schema() {
  SIRIUS_TRY(money, DataType::decimal(15, 2));
  return Schema::create({Field::required("o_orderkey", DataType::int64()),
                         Field::required("o_custkey", DataType::int64()),
                         Field::required("o_orderstatus", DataType::string()),
                         Field::required("o_totalprice", money),
                         Field::required("o_orderdate", DataType::date32()),
                         Field::required("o_orderpriority", DataType::string()),
                         Field::required("o_shippriority", DataType::int32())});
}
```
```rust
use sirius::prelude::*;

fn orders_schema() -> Result<Schema, PlanError> {
    let money = DataType::decimal(15, 2)?;
    Schema::try_new(vec![Field::required("o_orderkey", DataType::int64()),
                         Field::required("o_custkey", DataType::int64()),
                         Field::required("o_orderstatus", DataType::string()),
                         Field::required("o_totalprice", money),
                         Field::required("o_orderdate", DataType::date32()),
                         Field::required("o_orderpriority", DataType::string()),
                         Field::required("o_shippriority", DataType::int32())])
}
```

`TableSource::parquet(name, files, schema)` names the table, lists its files and attaches the
schema. A file is a `DataFile`: a URI and, optionally, the file's size in bytes. A plain string is
a file without a size, which is enough unless the scan cuts byte ranges
([Sized files](../part-2/02-workers-and-distribution.md#sized-files)).

The `orders` table is 64 Parquet files under `data/orders/`. Your application lists them:
`order_files` is a `std::vector<DataFile>` in C++ and a `Vec<String>` in Rust.

```cpp
SIRIUS_TRY(schema, orders_schema());
SIRIUS_TRY(orders, TableSource::parquet("orders", order_files, schema));
```
```rust
let schema = orders_schema()?;
let orders = TableSource::parquet("orders", order_files, &schema)?;
```

Two rules govern URIs:

- **A URI is literal.** One URI names one file. Sirius expands no glob and lists no directory.
- **A relative path is resolved at prepare**, against the working directory of the preparing
  process. The plan and its fingerprint hold the path as written.

Use absolute URIs when a plan spans processes. With a relative path, processes started in
different directories compute the same fingerprint but open different files.

Where the endpoint, region and credentials of a remote location live is
[open question 37](../reference/open-questions.md#37-remote-source-settings-and-credentials).

### The source check

A file is first compared with its declared schema when a worker opens it, during execution.
Before it reads a row, the engine checks that:

- every column the scan reads is in the file under its declared name and type, with no type
  conversion;
- a file with a declared size has that size.

The file may hold more columns than the schema lists, and columns the scan does not read are not
checked. A null in a `required` column is detected when the column is read.

A failed check fails the execution with `source_mismatch`. The error names the file, the scan and
the worker that opened it. The check always runs, because a file can change between prepare and
read. To also run it at `prepare`, before anything starts, see
[Checking sources early](06-prepare-and-execute.md#checking-sources-early).

## The four leaves

A leaf is a node with no relational input. Part I uses three; `exchange_source` belongs to
[Part II](../part-2/04-stages-and-bindings.md).

| Leaf | Factory | Where the rows come from |
|---|---|---|
| `Scan` | `Relation::scan(source, options)` | Files of a `TableSource`. |
| `Input` | `Relation::input(name, schema, on)` | A named stream your application feeds at run time. |
| `Values` | `Relation::values(schema, batches, on)` | Constant rows embedded in the plan. |
| `ExchangeSource` | `Relation::exchange_source(...)` | Another stage of the same plan (Part II). |

### Scan

`Relation::scan` takes the source and a `ScanOptions`. `ScanOptions::defaults()` runs the scan on
`local[1]`, reads all columns and assigns files to workers round-robin. `with_columns` narrows the
columns; the others are never read.

```cpp
SIRIUS_TRY(scan, Relation::scan(orders, ScanOptions::defaults().with_columns(
                                             {"o_orderkey", "o_orderdate", "o_orderpriority"})));
```
```rust
let scan = Relation::scan(&orders, ScanOptions::default()
    .with_columns(&["o_orderkey", "o_orderdate", "o_orderpriority"]))?;
```

`scan.schema()` has three fields, in the order you asked for them.

### Input

`Relation::input` declares a stream by name and schema. The plan records only that requirement;
`plan.inputs()` lists a plan's inputs. You bind the batches when you prepare the plan
([next chapter](06-prepare-and-execute.md)), and a plan with an input cannot run without them.

This input reuses the `orders` schema:

```cpp
SIRIUS_TRY(recent, Relation::input("recent_orders", schema));   // on local[1] by default
```
```rust
let recent = Relation::input("recent_orders", &schema, &WorkerSet::local())?;
```

### Values

`Relation::values` embeds rows in the plan, for small lookup tables or test data. The rows are
`RecordBatch`es you already hold, for example batches adopted from Arrow. Here `priority_schema`
and `priority_batches` are a small table of order priorities.

```cpp
SIRIUS_TRY(priorities, Relation::values(priority_schema, priority_batches, WorkerSet::local()));
```
```rust
let priorities = Relation::values(&priority_schema, priority_batches, &WorkerSet::local())?;
```

## Building on top: filter and project

First, keep the orders placed in March 1995:

```cpp
SIRIUS_TRY(date, scan.column("o_orderdate"));
SIRIUS_TRY(march_1, Value::date32(1995, 3, 1));
SIRIUS_TRY(april_1, Value::date32(1995, 4, 1));
SIRIUS_TRY(from, date.ge(Expression::literal(march_1)));
SIRIUS_TRY(until, date.lt(Expression::literal(april_1)));
SIRIUS_TRY(in_march, from.and_(until));
SIRIUS_TRY(march, scan.filter(in_march));
```
```rust
let date = scan.column("o_orderdate")?;
let march_1 = Value::date32(1995, 3, 1)?;
let april_1 = Value::date32(1995, 4, 1)?;
let from = date.ge(&Expression::literal(march_1))?;
let until = date.lt(&Expression::literal(april_1))?;
let in_march = from.and(&until)?;
let march = scan.filter(&in_march)?;
```

`filter` checks that the predicate is boolean and that every column in it belongs to `scan`. A
filter keeps its input's schema, so `march.schema()` is still the three scanned columns.

Next, project the key and the date, and replace the priority string with a boolean `is_urgent`:

```cpp
SIRIUS_TRY(columns, march.pass_through({"o_orderkey", "o_orderdate"}));
SIRIUS_TRY(priority, march.column("o_orderpriority"));
SIRIUS_TRY(urgent, priority.eq(Expression::literal(Value::string("1-URGENT"))));
columns.push_back(urgent.named("is_urgent"));
SIRIUS_TRY(projected, march.project(std::move(columns)));
```
```rust
let mut columns = march.pass_through(&["o_orderkey", "o_orderdate"])?;
let priority = march.column("o_orderpriority")?;
let urgent = priority.eq(&Expression::literal(Value::string("1-URGENT")))?;
columns.push(urgent.named("is_urgent"));
let projected = march.project(columns)?;
```

### A projection opens a new scope

The output of `project` has a new scope, even for columns that pass through unchanged
([Projection creates a new scope](04-schemas-and-expressions.md#projection-creates-a-new-scope)).
`urgent` still belongs to `march`, so using it above the projection fails with `invalid_plan`
([A scope mismatch](04-schemas-and-expressions.md#a-scope-mismatch)). Ask `projected` for its
column instead:

```cpp
auto wrong = projected.filter(urgent);     // invalid_plan: `urgent` is scoped to the Filter below
SIRIUS_TRY(flag, projected.column("is_urgent"));
SIRIUS_TRY(urgent_rows, projected.filter(flag));
```
```rust
let wrong = projected.filter(&urgent);    // PlanError::Invalid: `urgent` is scoped to the Filter below
let flag = projected.column("is_urgent")?;
let urgent_rows = projected.filter(&flag)?;
```

## From relations to a plan

`Plan::from_root` turns a tree of relations into a plan. It adds an `Output` node on top of your
root (named `"result"` unless you pass another name), assigns every node a deterministic id and
validates the graph. A root that already ends in `output(name)` is used as is.

```cpp
SIRIUS_TRY(plan, Plan::from_root(projected));
std::cout << plan.explain(ExplainOptions::defaults().with_title("march_orders"));
```
```rust
let plan = Plan::from_root(&projected, "result")?;
println!("{}", plan.explain(&ExplainOptions::default().with_title("march_orders")));
```

`explain()` prints the plan as a text tree:

```text
Plan "march_orders" (joined)
worker sets: local[1]
Output "result" on local[1]
└─ Project [o_orderkey, o_orderdate, is_urgent = o_orderpriority == "1-URGENT"]
   └─ Filter o_orderdate >= 1995-03-01 and o_orderdate < 1995-04-01
      └─ Scan orders [o_orderkey, o_orderdate, o_orderpriority]  on local[1]  shards=files_round_robin
```

- The quoted name is the title from `with_title`, a label only. Without one the header reads
  `Plan (joined)`.
- `joined` is the plan's *form*. The examples here use joined form. Part II also builds stages and individual worker instances.
- The second line lists the plan's worker sets. The full format is in
  [Plan notation](../reference/plan-notation.md#header).

The text is for people and its format may change between releases, so do not parse it. A program
reads a plan through its accessors, such as `fingerprint()` and the
[node view](../reference/api-at-a-glance.md#node-view).

`with_schemas(true)` adds a line with each node's schema:

```cpp
std::cout << plan.explain(ExplainOptions::defaults().with_schemas(true));
```
```rust
println!("{}", plan.explain(&ExplainOptions::default().with_schemas(true)));
```

```text
Output "result" on local[1]
└─ Project [o_orderkey, o_orderdate, is_urgent = o_orderpriority == "1-URGENT"]
   │  -> (o_orderkey int64, o_orderdate date32, is_urgent boolean)
   └─ Filter o_orderdate >= 1995-03-01 and o_orderdate < 1995-04-01
      │  -> (o_orderkey int64, o_orderdate date32, o_orderpriority string)
      └─ Scan orders [o_orderkey, o_orderdate, o_orderpriority]  on local[1]  shards=files_round_robin
            -> (o_orderkey int64, o_orderdate date32, o_orderpriority string)
```

Every field here is required. A nullable field prints as `name type?`.

`with_expression_types(true)` prints the type of every expression: each column reference, literal
and operator or call result is followed by `:type`. Literals otherwise print bare, so this is the
quickest way to find the operand behind a type mismatch
([No implicit conversion](04-schemas-and-expressions.md#no-implicit-conversion)).

```cpp
std::cout << plan.explain(ExplainOptions::defaults().with_expression_types(true));
```
```rust
println!("{}", plan.explain(&ExplainOptions::default().with_expression_types(true)));
```

```text
Output "result" on local[1]
└─ Project [o_orderkey:int64, o_orderdate:date32,
   │        is_urgent = (o_orderpriority:string == "1-URGENT":string):boolean]
   └─ Filter ((o_orderdate:date32 >= 1995-03-01:date32):boolean
      │       and (o_orderdate:date32 < 1995-04-01:date32):boolean):boolean
      └─ Scan orders [o_orderkey, o_orderdate, o_orderpriority]  on local[1]  shards=files_round_robin
```

The two options combine.

### What `Plan::from_root` checks

Each relation factory already checked its own node: scopes, types, unique names. `Plan::from_root`
and its general form `Plan::create` (below) check the graph: it is acyclic, output and input names
are unique, and every node's requirements on its input hold.
[Validation](../part-2/07-validation.md#the-invariants) lists all the invariants.

## Fingerprints

`fingerprint()` is a hash of the plan's canonical form. Node ids are assigned children before
parents and left before right. Two builders that produce the same canonical graph and parameter
values produce the same fingerprint under the same IR version.

```cpp
SIRIUS_TRY(again, Plan::from_root(projected));
bool same = plan.fingerprint() == again.fingerprint();   // true: same graph, same ids
```
```rust
let again = Plan::from_root(&projected, "result")?;
let same = plan.fingerprint() == again.fingerprint();   // true: same graph, same ids
```

Processes running a closed plan compare fingerprints as part of the agreement check. The
fingerprint covers plan content, including literals and shard descriptors. It identifies a plan,
but a result cache also needs input versions. Replacing a file at the same URI can change the
result without changing the plan.

The fingerprint covers the plan as you built it. A property that preparation resolves, such as a
join's `auto` build side ([The build side](07-aggregates-joins-and-ordering.md#the-build-side)),
counts as `auto`, whatever preparation picks.

## Several outputs and shared nodes

For more than one result, end each branch with `output(name)` and pass all terminals to
`Plan::create`. This plan returns every March order and, separately, the urgent ones:

```cpp
SIRIUS_TRY(all_out, projected.output("march"));
SIRIUS_TRY(urgent_out, urgent_rows.output("urgent"));
SIRIUS_TRY(views, Plan::create({all_out, urgent_out}));
```
```rust
let all_out = projected.output("march")?;
let urgent_out = urgent_rows.output("urgent")?;
let views = Plan::try_new(vec![all_out, urgent_out], vec![])?;
```

The empty second argument in Rust is the list of value sinks
([Values across stage boundaries](../part-2/05-runtime-filters.md#values-across-stage-boundaries)),
which C++ defaults.

`projected` is one node with two parents. `explain()`, here with the title `march_views`, prints
it once, marks it `#A` and refers back to it:

```text
Plan "march_views" (joined)
worker sets: local[1]
Output "march" on local[1]
└─ Project [o_orderkey, o_orderdate, is_urgent = o_orderpriority == "1-URGENT"]  #A
   └─ Filter o_orderdate >= 1995-03-01 and o_orderdate < 1995-04-01
      └─ Scan orders [o_orderkey, o_orderdate, o_orderpriority]  on local[1]  shards=files_round_robin
Output "urgent" on local[1]
└─ Filter is_urgent
   └─ Ref #A
```

**A shared node is evaluated once per worker:** both outputs see the same rows of `#A`, as if it
ran once. The engine may run the shared part once and feed both branches, or recompute it, because
for this subtree both give the same rows.

Recomputing does not give the same rows for every subtree. `Plan::create` rejects a shared node
whose subtree contains an `Input` (a channel is read once), a `volatile` node such as one that
calls `random()`, or an `effectful` node. Sharing those needs an explicit "evaluate once and fan
out" node, which is
[open question 10](../reference/open-questions.md#10-shared-subplans-and-multicast). The rule is in
[Shared subplans](../reference/extension-points.md#shared-subplans).

## One worker: `local[1]`

Every leaf and every `Output` in this chapter printed `on local[1]`. That is the *implicit worker
set*: a set named `local` with one worker, `local#0`. `ScanOptions::defaults()` and
`WorkerSet::local()` put their leaves there. In C++ the worker set of `Relation::input` defaults to
`WorkerSet::local()`; Rust spells it out. Every node above a leaf runs on its input's set.

A worker is a logical slot, bound to exactly one GPU when you prepare the plan. With one worker,
every row is on that worker, so every relation has the distribution `single(local[1])`:

```cpp
const WorkerSet& on = projected.workers();   // on.name() == "local", on.size() == 1
const Distribution& spread = projected.distribution();   // single(local[1])
```
```rust
let on = projected.workers();   // on.name() == "local", on.size() == Some(1)
let spread = projected.distribution();   // single(local[1])
```

Larger worker sets and their distributions are in
[Workers and distribution](../part-2/02-workers-and-distribution.md).
