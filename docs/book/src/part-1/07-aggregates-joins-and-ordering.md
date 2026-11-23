# Aggregates, joins and ordering

This chapter adds the remaining relational operations of the plan builder and ends with a query
over three tables, still on one worker.

`lineitem`, `orders` and `customer` are `TableSource`s built like `orders` in
[Relations and plans](05-relations-and-plans.md). The [running
example](../part-3/00-running-example.md) lists their schemas.

## One rule for every operator: keys are names, values are expressions

Where a node needs *keys* (group keys, join key pairs, sort keys, window keys), pass **column
names** of its input. Where it needs *values* (predicates, measure arguments, join residuals), pass
**scoped expressions**. For a computed key, compute it in a `project` first and name the new column.
[Conventions](../conventions.md#keys-are-names-values-are-expressions) explains why keys must be
names.

## Aggregate

The pricing summary from the running example groups the items shipped on or before 1998-09-02 by
return flag and line status. For each group it returns the total quantity, the total discounted
revenue, the average extended price and the number of lines.

`shipped` is `lineitem` scanned and filtered on the ship date. A `Measure` names an output column
and applies an aggregate function to an expression in the input's scope.

```cpp
#include <sirius/sirius.hpp>
using namespace sirius;

Result<Relation> pricing_summary(const Relation& shipped) {
  SIRIUS_TRY(price, shipped.column("l_extendedprice"));
  SIRIUS_TRY(discount, shipped.column("l_discount"));
  SIRIUS_TRY(one, Value::decimal("1", 15, 2));
  SIRIUS_TRY(kept, Expression::literal(one).sub(discount));
  SIRIUS_TRY(revenue, price.mul(kept));
  SIRIUS_TRY(quantity, shipped.column("l_quantity"));
  SIRIUS_TRY(sum_qty, Measure::sum("sum_qty", quantity));
  SIRIUS_TRY(sum_revenue, Measure::sum("sum_revenue", revenue));
  SIRIUS_TRY(avg_price, Measure::avg("avg_price", price));
  return shipped.aggregate({"l_returnflag", "l_linestatus"},
                           {sum_qty, sum_revenue, avg_price, Measure::count_star("count_order")});
}
```
```rust
use sirius::prelude::*;

fn pricing_summary(shipped: &Relation) -> Result<Relation, PlanError> {
    let price = shipped.column("l_extendedprice")?;
    let discount = shipped.column("l_discount")?;
    let one = Value::decimal("1", 15, 2)?;
    let kept = Expression::literal(one).sub(&discount)?;
    let revenue = price.mul(&kept)?;
    let quantity = shipped.column("l_quantity")?;
    let sum_qty = Measure::sum("sum_qty", &quantity)?;
    let sum_revenue = Measure::sum("sum_revenue", &revenue)?;
    let avg_price = Measure::avg("avg_price", &price)?;
    shipped.aggregate(&["l_returnflag", "l_linestatus"],
                      vec![sum_qty, sum_revenue, avg_price, Measure::count_star("count_order")])
}
```

The plan prints the measures on a continuation line:

```text
Aggregate keys=[l_returnflag, l_linestatus]
│  measures=[sum_qty = sum(l_quantity), sum_revenue = sum(l_extendedprice * (1 - l_discount)),
│            avg_price = avg(l_extendedprice), count_order = count_star()]
└─ Filter l_shipdate <= 1998-09-02
   └─ Scan lineitem [l_returnflag, l_linestatus, l_quantity, l_extendedprice,
                     l_discount, l_shipdate]  on local[1]  shards=files_round_robin
```

- **Output schema.** The keys come first, in the order given, then one column per measure. The
  output opens a new scope, as a projection does.
- **The measures.** `count_star`, `count`, `sum`, `avg`, `min`, `max`, `count_distinct` and
  `stddev_samp`. Every factory except `count_star` takes an argument expression and checks its
  type, so `sum` over a string fails at that call with `invalid_plan`.
- **No keys.** An empty key list gives an ungrouped aggregate. It returns exactly one row, even
  when its input is empty.

### Phases

`aggregate` is *single-phase*: it sees all the rows of each group and produces final values. When
the rows of a group are spread over several workers, the plan splits the aggregation into
`aggregate_partial`, an exchange and `aggregate_final`; see [Two-phase
aggregation](../part-2/03-exchanges.md#two-phase-aggregation-end-to-end) and
[Aggregation](../part-3/02-aggregation.md), which also covers counting distinct values across
workers.

### Grouping sets

*Grouping sets* add subtotals: here the pricing summary per return flag and line status, per
return flag alone, and as one grand total. Each set is a subset of the keys. A `GroupingSets`
value holds the keys and the list of sets, and `aggregate_sets` aggregates every input row once
per set.

```cpp
SIRIUS_TRY(sum_qty, Measure::sum("sum_qty", quantity));
SIRIUS_TRY(sets, GroupingSets::create({"l_returnflag", "l_linestatus"},
                                      {{"l_returnflag", "l_linestatus"}, {"l_returnflag"}, {}}));
SIRIUS_TRY(subtotals, shipped.aggregate_sets(sets, {sum_qty, Measure::count_star("count_order")}));
```
```rust
let sum_qty = Measure::sum("sum_qty", &quantity)?;
let sets = GroupingSets::try_new(&["l_returnflag", "l_linestatus"],
                                 &[&["l_returnflag", "l_linestatus"], &["l_returnflag"], &[]])?;
let subtotals = shipped.aggregate_sets(&sets, vec![sum_qty, Measure::count_star("count_order")])?;
```

```text
Aggregate keys=[l_returnflag, l_linestatus]
│  sets=[(l_returnflag, l_linestatus), (l_returnflag), ()]
│  measures=[sum_qty = sum(l_quantity), count_order = count_star()]
│  -> (l_returnflag string?, l_linestatus string?, grouping_id int32, sum_qty decimal(38,2)?,
│      count_order int64)
└─ Filter l_shipdate <= 1998-09-02
   └─ Scan lineitem [l_returnflag, l_linestatus, l_quantity, l_extendedprice,
                     l_discount, l_shipdate]  on local[1]  shards=files_round_robin
```

- **One row per group per set.** A row keeps the keys of its own set and has null in the others:
  the subtotal for return flag "R" has a null `l_linestatus`. A key that some set leaves out is
  therefore nullable in the output, whatever it was in the input.
- **The grouping id.** `grouping_id`, the column after the keys, is the position of the row's set
  in the list, starting at 0. It is an `int32` and never null. The list is part of the plan, so
  the numbers mean the same on every worker. Rename the column with `with_id_name` if the name
  clashes with one of yours.
- **Helpers.** `GroupingSets::rollup(keys)` builds the list of prefixes, longest first, and
  `GroupingSets::cube(keys)` every subset. `rollup({"l_returnflag", "l_linestatus"})` is the list
  above. The plan stores and prints the resulting sets, not the helper call.
- **The empty set.** `()` groups all rows together. Like an ungrouped aggregate, it produces
  exactly one row, even over an empty input, and its sum is then null.
- **`GROUPING(...)`.** The value of SQL's `GROUPING` function depends only on the row's set, so
  compute it from `grouping_id` in a projection above the aggregate.

Across workers the grouping id travels as one more key between `aggregate_partial_sets` and
`aggregate_final`; see
[Q2r](../part-3/02-aggregation.md#q2r-subtotals-with-grouping-sets).

## Join

This join returns every line item with the March 1995 order it belongs to. The left input `items`
is a scan of `lineitem` narrowed to four columns. The right input `march` is the filtered orders
from [Relations and plans](05-relations-and-plans.md). The join keys are a list of (left column,
right column) pairs.

```cpp
SIRIUS_TRY(on, JoinOn::keys({{"l_orderkey", "o_orderkey"}}));
SIRIUS_TRY(joined, items.join(march, JoinType::inner, std::move(on), JoinOptions::defaults()));
```
```rust
let on = JoinOn::keys(&[("l_orderkey", "o_orderkey")])?;
let joined = items.join(&march, JoinType::Inner, on, JoinOptions::default())?;
```

```text
Join inner keys=[l_orderkey == o_orderkey] build=right  mode=single
├─ Scan lineitem [l_orderkey, l_extendedprice, l_discount, l_shipdate]  on local[1]  shards=files_round_robin
└─ Filter o_orderdate >= 1995-03-01 and o_orderdate < 1995-04-01
   └─ Scan orders [o_orderkey, o_orderdate, o_orderpriority]  on local[1]  shards=files_round_robin
```

The tree prints the left input first. `build=` and `mode=` are explained below.

### The output schema

Output field names must be unique, so a join that outputs fields of both sides fails with
`invalid_plan` when its inputs share a column name, as in a self-join. Rename the columns of one
side with a `project` first. The join type decides the output fields:

| `JoinType` | Output fields |
|---|---|
| `inner`, `left`, `right`, `full` | Left fields, then right fields. |
| `left_semi`, `left_anti` | Left fields only. |
| `right_semi`, `right_anti` | Right fields only. |
| `mark` | Left fields, then a boolean column `mark`. |

Both keys of a pair must have the same type ([No implicit
conversion](04-schemas-and-expressions.md#no-implicit-conversion)). If they differ, cast one side
in a projection first.

### Null-safe keys

A plain key pair compares with `==`, so a row whose key is null matches nothing. A *null-safe* pair
treats two nulls as equal and a null and a value as different, like SQL's `IS NOT DISTINCT FROM`.
Build one with `JoinKey::null_safe`. A (left, right) pair of names, as in the join above, is the
short form of `JoinKey::equal`. One join may mix both kinds. Here `customer_keys` and `order_keys`
are scans of `c_custkey` and of `o_custkey`:

```cpp
SIRIUS_TRY(same_key, JoinOn::keys({JoinKey::null_safe("c_custkey", "o_custkey")}));
SIRIUS_TRY(unmatched, customer_keys.join(order_keys, JoinType::left_anti, std::move(same_key),
                                         JoinOptions::defaults()));
```
```rust
let same_key = JoinOn::keys([JoinKey::null_safe("c_custkey", "o_custkey")])?;
let unmatched = customer_keys.join(&order_keys, JoinType::LeftAnti, same_key,
                                   JoinOptions::default())?;
```

```text
Join left_anti keys=[c_custkey <=> o_custkey] build=right  mode=single
├─ Scan customer [c_custkey]  on local[1]  shards=files_round_robin
└─ Scan orders [o_custkey]  on local[1]  shards=files_round_robin
```

The tree prints a null-safe pair as `<=>`. The running example has no null keys, so here `<=>` and
`==` select the same rows.

With more workers, the exchanges below the join need nothing new for a null-safe pair ([The
partitioning signature and
co-partitioning](../part-2/03-exchanges.md#the-partitioning-signature-and-co-partitioning)). A
runtime filter built on its key must pass null keys on to the join ([Where a filter may be
applied](../part-2/05-runtime-filters.md#where-a-filter-may-be-applied)).

### Residuals and `JoinScope`

To keep only the line items that shipped after their order was placed, the join must compare a
column of each side. That condition is a *residual*: a boolean expression evaluated on each pair of
rows whose keys matched.

A residual cannot use `items.column(...)` or `march.column(...)`: those expressions belong to the
inputs, and Sirius never guesses a column's side from its name or type. `JoinScope::create(left,
right)` gives a scope that says which side each column comes from. In a self-join, only the scope
tells the sides apart.

```cpp
JoinScope scope = JoinScope::create(items, march);
SIRIUS_TRY(ship, scope.left("l_shipdate"));
SIRIUS_TRY(placed, scope.right("o_orderdate"));
SIRIUS_TRY(later, ship.gt(placed));
SIRIUS_TRY(keys, JoinOn::keys({{"l_orderkey", "o_orderkey"}}));
JoinOn late_on = std::move(keys).with_residual(later);
SIRIUS_TRY(late, items.join(march, JoinType::inner, std::move(late_on), JoinOptions::defaults()));
```
```rust
let scope = JoinScope::new(&items, &march);
let ship = scope.left("l_shipdate")?;
let placed = scope.right("o_orderdate")?;
let later = ship.gt(&placed)?;
let keys = JoinOn::keys(&[("l_orderkey", "o_orderkey")])?;
let late_on = keys.with_residual(later);
let late = items.join(&march, JoinType::Inner, late_on, JoinOptions::default())?;
```

```text
Join inner keys=[l_orderkey == o_orderkey] residual=(l_shipdate > o_orderdate) build=right  mode=single
├─ Scan lineitem [l_orderkey, l_extendedprice, l_discount, l_shipdate]  on local[1]  shards=files_round_robin
└─ Filter o_orderdate >= 1995-03-01 and o_orderdate < 1995-04-01
   └─ Scan orders [o_orderkey, o_orderdate, o_orderpriority]  on local[1]  shards=files_round_robin
```

`join` rejects a residual built from a scope for different inputs. A join with no key pairs is a
nested-loop join: the residual compares every left row with every right row.

### The build side

A hash join reads one input completely into a hash table (the *build* side), then streams the other
input past it (the *probe* side). The build input is blocking: the join emits nothing until the
whole build side has arrived.

The plan records the build side through `JoinOptions::with_build`:

| `BuildSide` | Tree | Who decides |
|---|---|---|
| `right`, the value in `JoinOptions::defaults()` | `build=right` | You. |
| `left` | `build=left` | You. |
| `auto_` (`Auto` in Rust) | `build=auto` | Preparation. The prepared plan's `explain()` shows the result, such as `build=auto->right`. |

Build on the smaller input, which is why the filtered orders sit on the right above. The engine
never swaps the sides: if it cannot honor an explicit side, preparation fails with `unsupported`.
[Preparing a plan](06-prepare-and-execute.md#preparing-a-plan) describes how `auto` is resolved.

```cpp
JoinOptions build_left = JoinOptions::defaults().with_build(BuildSide::left);
JoinOptions build_auto = JoinOptions::defaults().with_build(BuildSide::auto_);  // prepare picks
```
```rust
let build_left = JoinOptions::default().with_build(BuildSide::Left);
let build_auto = JoinOptions::default().with_build(BuildSide::Auto);    // prepare picks
```

`auto` is allowed here because the build side is local to each worker: whichever side a worker
builds, no other worker receives anything different. That holds for this `single` join and for the
co-partitioned joins of Part II. In the other join modes of Part II, the build side follows from
the exchanges in the plan, so it is never `auto`.

A `mark` join says, for each left row, whether the right input holds a match, so it always builds
on its right input. `BuildSide::left` or `BuildSide::auto_` on a `mark` join fails at the `join`
call with `invalid_plan`.

You do not choose `mode=single`. It is derived from where the inputs are: on `local[1]` both are on
the same worker. Part II shows the other modes (co-partitioned, broadcast build and decided),
derived the same way.

## Sort

This sort returns all 1996 orders by order date, most expensive first within a day. `orders_1996`
is `orders` scanned for three columns and filtered to 1996. Sort keys are column names with a
direction; nulls sort last unless you say `nulls_first()`. Floating-point keys follow the order in
[Nulls and NaN](04-schemas-and-expressions.md#nulls-and-nan).

```cpp
SIRIUS_TRY(sorted, orders_1996.sort({SortKey::asc("o_orderdate"), SortKey::desc("o_totalprice")}));
```
```rust
let sorted = orders_1996.sort(vec![SortKey::asc("o_orderdate"), SortKey::desc("o_totalprice")])?;
```

```text
Sort [o_orderdate asc, o_totalprice desc]
└─ Filter o_orderdate >= 1996-01-01 and o_orderdate < 1997-01-01
   └─ Scan orders [o_orderkey, o_orderdate, o_totalprice]  on local[1]  shards=files_round_robin
```

A sort is blocking: it needs all of its input before it emits a row. A `Sort` orders the rows
*within each worker*. On `local[1]` that is the whole result, and the `Output` delivers it in
order. With more workers the plan says how to get one global order; [Sort, top-N and
limit](../part-3/06-sort-top-n-and-limit.md) shows two ways.

## Top-N

`top_n` keeps up to `n` rows per worker using the supplied sort keys. This one returns the ten most expensive orders of
1996, ties broken by order key.

```cpp
SIRIUS_TRY(top, orders_1996.top_n(10, {SortKey::desc("o_totalprice"), SortKey::asc("o_orderkey")}));
```
```rust
let top = orders_1996.top_n(10, vec![SortKey::desc("o_totalprice"), SortKey::asc("o_orderkey")])?;
```

```text
TopN 10 [o_totalprice desc, o_orderkey asc]
└─ Filter o_orderdate >= 1996-01-01 and o_orderdate < 1997-01-01
   └─ Scan orders [o_orderkey, o_orderdate, o_totalprice]  on local[1]  shards=files_round_robin
```

`TopN` is blocking, but it keeps only the best `n` rows seen so far, so it is usually much cheaper
than a `Sort` followed by a `Limit`. Add a tie-breaking key, as here, to get the same answer every
time.

`TopN` *composes with itself*: the top 10 of several top-10 lists is the top 10 overall. Part III
uses that to run the same node twice, once per worker and once at the end.

### Top-N per key

`top_n_per_key` keeps the best rows of each group. This one returns each customer's most recent
order, ties broken by order key; `by_customer` is `orders` scanned for three columns.

```cpp
SIRIUS_TRY(latest, by_customer.top_n_per_key(
    {"o_custkey"}, 1, {SortKey::desc("o_orderdate"), SortKey::asc("o_orderkey")}));
```
```rust
let latest = by_customer.top_n_per_key(
    &["o_custkey"], 1, vec![SortKey::desc("o_orderdate"), SortKey::asc("o_orderkey")])?;
```

```text
TopN 1 [o_orderdate desc, o_orderkey asc]  per=[o_custkey]
└─ Scan orders [o_orderkey, o_custkey, o_orderdate]  on local[1]  shards=files_round_robin
```

Each kept row is a whole input row, which is what SQL's `DISTINCT ON` asks for. The engine picks
among rows of one key that tie on the whole order, as for `top_n`.

Without an order, `top_n_per_key({"o_custkey"}, 1, {})` keeps an arbitrary order per customer.
This implements a grouped `first()` only when the host allows any row, including one with nulls. For a
`first()` next to other measures, join the kept row back to the aggregate on the group keys, with
null-safe pairs.

## Limit

On an unordered input, `limit(n)` keeps at most `n` rows per worker. This one returns any 100 orders with status "F", where `finished` is
`orders` filtered on that status.

```cpp
SIRIUS_TRY(any_100, finished.limit(100));
```
```rust
let any_100 = finished.limit(100, 0)?;
```

```text
Limit 100
└─ Filter o_orderstatus == "F"
   └─ Scan orders [o_orderkey, o_orderstatus, o_totalprice]  on local[1]  shards=files_round_robin
```

This `Limit` streams, because its input has no order: it passes rows through until it has `n`, then
**closes** its input. Closing tells everything upstream that no more rows are needed, so the scan
stops reading files it has not started. Above an `Input`, the writers see the receiver close and
the execution still succeeds; see [Writing and
finishing](06-prepare-and-execute.md#writing-and-finishing).

Over a sorted input a `Limit` returns the first `n` rows in that order and waits for its whole
input first. `top_n` is the cheaper way to ask for that.

The second argument, optional in C++, is an offset: `limit(10, 20)` skips 20 rows and returns the
next 10. An offset is legal only when all rows are on one worker, as on `local[1]`; see [Sort, top-N
and limit](../part-3/06-sort-top-n-and-limit.md).

## Window

A window function computes a value for each row from other rows of the same input, and keeps every
row. This example computes, for each 1996 order, how its price ranks among its customer's orders
and what that customer had spent up to and including it. `customer_orders` is `orders` scanned for
four columns and filtered to 1996.

```cpp
SIRIUS_TRY(ranked, customer_orders.window_ranking(
    {"o_custkey"}, {SortKey::desc("o_totalprice")},
    {Ranking::rank("price_rank"), Ranking::row_number("price_position")}));
SIRIUS_TRY(price, ranked.column("o_totalprice"));
SIRIUS_TRY(spent, Measure::sum("spent_so_far", price));
SIRIUS_TRY(running, ranked.window_running(
    {"o_custkey"}, {SortKey::asc("o_orderdate"), SortKey::asc("o_orderkey")}, {spent}));
```
```rust
let ranked = customer_orders.window_ranking(
    &["o_custkey"], vec![SortKey::desc("o_totalprice")],
    vec![Ranking::rank("price_rank"), Ranking::row_number("price_position")])?;
let price = ranked.column("o_totalprice")?;
let spent = Measure::sum("spent_so_far", &price)?;
let running = ranked.window_running(
    &["o_custkey"], vec![SortKey::asc("o_orderdate"), SortKey::asc("o_orderkey")], vec![spent])?;
```

```text
Window running keys=[o_custkey] order=[o_orderdate asc, o_orderkey asc]
│  measures=[spent_so_far = sum(o_totalprice)]
└─ Window ranking keys=[o_custkey] order=[o_totalprice desc]
   │  rankings=[price_rank = rank(), price_position = row_number()]
   └─ Filter o_orderdate >= 1996-01-01 and o_orderdate < 1997-01-01
      └─ Scan orders [o_orderkey, o_custkey, o_orderdate, o_totalprice]
                     on local[1]  shards=files_round_robin
```

Each `Window` keeps its input columns and appends one column per function, so `running` has the
four scanned columns, then `price_rank`, `price_position` and `spent_so_far`.

- **Window keys.** Rows with the same values of the window keys form a *window group*, here one
  per customer. SQL calls the keys `PARTITION BY`. With no keys, all rows are one group. Null keys
  are equal to each other.
- **Three forms.** Nothing else exists in the IR, so `window_running` with an `avg` fails at the
  call with `invalid_plan`.
  - `window_whole(keys, measures)` gives every row a measure over its whole group: `count_star`,
    `count`, `sum`, `avg`, `min` or `max`.
  - `window_ranking(keys, order, rankings)` gives every row its position in the group's order:
    `rank`, `dense_rank` or `row_number`.
  - `window_running(keys, order, measures)` gives every row a measure over the rows from the
    first of its group through itself: `count_star`, `count`, `sum`, `min` or `max`.
- **Order keys and ties.** Order keys are sort keys. Rows of a group with equal order keys are
  *peers*. `rank` gives peers the same number and skips the numbers after it; `dense_rank` skips
  none. `row_number` and running measures take peers in an order the engine picks. Add a
  tie-breaking key, as `o_orderkey` above, to get the same answer every time.
- **One form, one node.** The functions of one node share its form, its keys and its order, as
  `price_rank` and `price_position` do. `spent_so_far` has another form and another order, so it
  gets its own node, stacked on the first.

A `Window` is blocking, and its output is in no particular order; sort afterwards if you need one.
With more workers, every row of a window group must be on the same worker before the node runs;
[Windows need whole groups](../part-2/03-exchanges.md#windows-need-whole-groups) shows the
exchange that makes it so.

## Union

`union_all` stacks relations with the same columns, keeping duplicates. Here historical orders from
files meet the `recent_orders` input from the previous chapter. `history` scans two columns. The
input carries all seven `orders` columns, so `recent` is that input projected to the same two.

```cpp
SIRIUS_TRY(all_orders, Relation::union_all({history, recent}));
```
```rust
let all_orders = Relation::union_all(vec![history, recent])?;
```

```text
Union
├─ Scan orders [o_orderkey, o_orderstatus]  on local[1]  shards=files_round_robin
└─ Project [o_orderkey, o_orderstatus]
   └─ Input "recent_orders" on local[1]
```

All inputs must have identical column types, in the same order ([No implicit
conversion](04-schemas-and-expressions.md#no-implicit-conversion)), and must be on the same worker
set. The output takes its names from the first input. Nullability is not part of the type: a
column is nullable in the output if it is nullable in any input.

## Set operations

`union_all` is the only set operation with a node of its own. The others compare whole rows, treat
nulls as equal, and are compositions. The distinct forms use the nodes above; the `ALL` forms add
`repeat`.

| SQL | Plan |
|---|---|
| `UNION` | `union_all`, then an aggregate on every column with no measures |
| `INTERSECT` | `left_semi` join on every column, every pair null-safe, then the same aggregate |
| `EXCEPT` | `left_anti` join on every column, every pair null-safe, then the same aggregate |
| `INTERSECT ALL` | `union_all` of the inputs, each tagged with its side; two aggregates that count the rows per side and take the smaller count; `repeat` |
| `EXCEPT ALL` | `union_all` of the inputs, each tagged with its side; one aggregate that counts the rows of each side; `repeat` by the difference |

An aggregate with keys and no measures keeps one row per distinct key. `customer_keys EXCEPT
order_keys`, the customer keys that appear in no order, is the anti join from
[Null-safe keys](#null-safe-keys) under such an aggregate:

```cpp
SIRIUS_TRY(without_orders, unmatched.aggregate({"c_custkey"}, {}));
```
```rust
let without_orders = unmatched.aggregate(&["c_custkey"], vec![])?;
```

```text
Aggregate keys=[c_custkey]
└─ Join left_anti keys=[c_custkey <=> o_custkey] build=right  mode=single
   ├─ Scan customer [c_custkey]  on local[1]  shards=files_round_robin
   └─ Scan orders [o_custkey]  on local[1]  shards=files_round_robin
```

The result has the left input's columns and names. A left semi or left anti join outputs only left fields,
so the two inputs may use the same column names.

The `ALL` forms keep duplicates. A row that appears m times in the left input and n times in the
right appears min(m, n) times in `INTERSECT ALL`, and m - n times in `EXCEPT ALL` when m is the
larger. A join cannot count like that, so these plans count with an aggregate and then call
`repeat`, which emits each row as many times as an integer column says and drops that column.

With one row per customer in `customer_keys`, `order_keys EXCEPT ALL customer_keys` returns
each customer key once for every order beyond the first.
`both` is `union_all` of the two inputs, each projected to its key and two marker columns:
`in_left` is true in the rows of `order_keys` and null in the rows of `customer_keys`, and
`in_right` is the reverse. The markers are literals, `Value::boolean(true)` and
`Value::null(DataType::boolean())`. `count` skips nulls, so each measure counts one side:

```cpp
SIRIUS_TRY(in_left, both.column("in_left"));
SIRIUS_TRY(in_right, both.column("in_right"));
SIRIUS_TRY(left_rows, Measure::count("left_rows", in_left));
SIRIUS_TRY(right_rows, Measure::count("right_rows", in_right));
SIRIUS_TRY(counted, both.aggregate({"o_custkey"}, {left_rows, right_rows}));
SIRIUS_TRY(columns, counted.pass_through({"o_custkey"}));
SIRIUS_TRY(m, counted.column("left_rows"));
SIRIUS_TRY(n, counted.column("right_rows"));
SIRIUS_TRY(difference, m.sub(n));
columns.push_back(difference.named("copies"));
SIRIUS_TRY(with_copies, counted.project(std::move(columns)));
SIRIUS_TRY(copies, with_copies.column("copies"));
SIRIUS_TRY(positive, copies.gt(Expression::literal(Value::int64(0))));
SIRIUS_TRY(surplus, with_copies.filter(positive));
SIRIUS_TRY(later_orders, surplus.repeat("copies"));
```
```rust
let in_left = both.column("in_left")?;
let in_right = both.column("in_right")?;
let left_rows = Measure::count("left_rows", &in_left)?;
let right_rows = Measure::count("right_rows", &in_right)?;
let counted = both.aggregate(&["o_custkey"], vec![left_rows, right_rows])?;
let mut columns = counted.pass_through(&["o_custkey"])?;
let m = counted.column("left_rows")?;
let n = counted.column("right_rows")?;
let difference = m.sub(&n)?;
columns.push(difference.named("copies"));
let with_copies = counted.project(columns)?;
let copies = with_copies.column("copies")?;
let positive = copies.gt(&Expression::literal(Value::int64(0)))?;
let surplus = with_copies.filter(&positive)?;
let later_orders = surplus.repeat("copies")?;
```

```text
Repeat count=copies
└─ Filter copies > 0
   └─ Project [o_custkey, copies = left_rows - right_rows]
      └─ Aggregate keys=[o_custkey]
         │  measures=[left_rows = count(in_left), right_rows = count(in_right)]
         └─ Union
            ├─ Project [o_custkey, in_left = true, in_right = null]
            │  └─ Scan orders [o_custkey]  on local[1]  shards=files_round_robin
            └─ Project [c_custkey, in_left = null, in_right = true]
               └─ Scan customer [c_custkey]  on local[1]  shards=files_round_robin
```

`later_orders` has one column, `o_custkey`. A count of 0 drops its row, and a null or negative
count fails the execution, so the `Filter` removes the customers without an order before the
`Repeat` sees them.

`INTERSECT ALL` starts from the same `both`. One aggregate counts the rows per key and side. A
second, on the key alone, takes the `min` of those counts as `copies` and counts the sides. A
`Filter` keeps the keys that both sides have, and `repeat` emits the rows.

## Putting it together: a query over three tables

For customers in the BUILDING segment, take their orders placed before 1995-03-15 and those
orders' line items shipped after 1995-03-15. Compute the revenue per order (order key, order date
and ship priority), and return the 10 orders with the highest revenue, ties broken by the earlier
order date.

`items` is `lineitem` filtered on the ship date and projected to the order key and
`rev = l_extendedprice * (1 - l_discount)`. `buyers` is `orders` filtered on the order date, joined
to the BUILDING customers, and projected to three order columns.

```cpp
SIRIUS_TRY(on, JoinOn::keys({{"l_orderkey", "o_orderkey"}}));
SIRIUS_TRY(joined, items.join(buyers, JoinType::inner, std::move(on), JoinOptions::defaults()));
SIRIUS_TRY(rev, joined.column("rev"));
SIRIUS_TRY(revenue, Measure::sum("revenue", rev));
SIRIUS_TRY(grouped, joined.aggregate({"l_orderkey", "o_orderdate", "o_shippriority"}, {revenue}));
SIRIUS_TRY(top, grouped.top_n(10, {SortKey::desc("revenue"), SortKey::asc("o_orderdate")}));
SIRIUS_TRY(plan, Plan::from_root(top));
```
```rust
let on = JoinOn::keys(&[("l_orderkey", "o_orderkey")])?;
let joined = items.join(&buyers, JoinType::Inner, on, JoinOptions::default())?;
let rev = joined.column("rev")?;
let revenue = Measure::sum("revenue", &rev)?;
let grouped = joined.aggregate(&["l_orderkey", "o_orderdate", "o_shippriority"], vec![revenue])?;
let top = grouped.top_n(10, vec![SortKey::desc("revenue"), SortKey::asc("o_orderdate")])?;
let plan = Plan::from_root(&top, "result")?;
```

`plan.explain()`, with the title `q7_shipping_priority`, prints the whole plan:

```text
Plan "q7_shipping_priority" (joined)
worker sets: local[1]
Output "result" on local[1]
└─ TopN 10 [revenue desc, o_orderdate asc]
   └─ Aggregate keys=[l_orderkey, o_orderdate, o_shippriority] measures=[revenue = sum(rev)]
      └─ Join inner keys=[l_orderkey == o_orderkey] build=right  mode=single
         ├─ Project [l_orderkey, rev = l_extendedprice * (1 - l_discount)]
         │  └─ Filter l_shipdate > 1995-03-15
         │     └─ Scan lineitem [l_orderkey, l_extendedprice, l_discount, l_shipdate]
         │                      on local[1]  shards=files_round_robin
         └─ Project [o_orderkey, o_orderdate, o_shippriority]
            └─ Join inner keys=[o_custkey == c_custkey] build=right  mode=single
               ├─ Filter o_orderdate < 1995-03-15
               │  └─ Scan orders [o_orderkey, o_custkey, o_orderdate, o_shippriority]
               │                 on local[1]  shards=files_round_robin
               └─ Project [c_custkey]
                  └─ Filter c_mktsegment == "BUILDING"
                     └─ Scan customer [c_custkey, c_mktsegment]  on local[1]  shards=files_round_robin
```

To run it, prepare it with `Bindings::local()` and collect it as in the [previous
chapter](06-prepare-and-execute.md).

## What this plan does not say

This plan is complete for one worker: everything runs on the one worker of `local[1]`, so no rows
are spread or moved. With more workers, it leaves these questions open:

- which worker reads which of the 256 `lineitem` files;
- how a line item on one worker meets its order on another;
- whether the BUILDING customers are copied to every worker or split among them;
- whether the orders join tells the `lineitem` scan which order keys can never match;
- where the per-worker top 10 lists become one.

In Sirius, every such choice is a node or an edge in the plan. Part II starts there, with [The
questions a single-worker plan leaves
open](../part-2/01-everything-in-the-plan.md#the-questions-a-single-worker-plan-leaves-open). [A
multi-stage query](../part-3/07-a-multi-stage-query.md) returns to this query and shows its
distributed plan.
