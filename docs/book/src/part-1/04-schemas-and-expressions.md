# Types, schemas and expressions

This chapter builds the discounted revenue of a line item, `l_extendedprice * (1 - l_discount)`,
to introduce **data types**, **schemas** and **expressions**.

## Data types

A `DataType` is a small immutable value. Factories build the scalar types:

| Factory | Type |
|---|---|
| `DataType::boolean()` | true or false |
| `DataType::int8()`, `int16()`, `int32()`, `int64()` | 8-, 16-, 32- and 64-bit signed integers |
| `DataType::uint8()`, `uint16()`, `uint32()`, `uint64()` | 8-, 16-, 32- and 64-bit unsigned integers |
| `DataType::float32()`, `DataType::float64()` | 32- and 64-bit floating point |
| `DataType::decimal(precision, scale)` | an exact decimal number |
| `DataType::string()` | UTF-8 text |
| `DataType::date32()` | a calendar date |
| `DataType::timestamp(unit)` | a date and time of day, without a time zone |

Of these, only `DataType::decimal` returns a `Result`, because not every precision and scale fit
together. `decimal(15, 2)`, the type of every money column in the running example, holds up to
15 digits, 2 of them after the decimal point.

A timestamp counts in a `TimeUnit` (seconds, milliseconds, microseconds or nanoseconds) from
1970-01-01 00:00:00. Timestamps of different units are different types.

The names are Arrow's, and each type crosses the Arrow boundary as the Arrow type of the same
name. Times of day, timestamps with a time zone, intervals and binary strings have no
`DataType` yet; see [Additive later](../reference/extension-points.md#additive-later).

Two factories build nested types: `DataType::list(element)` and `DataType::struct_of(fields)`.
`struct_of` can fail for the same reasons a schema can (next section).

A `DataType` can also carry a **state type**, read with `state()`. Partial and combine aggregates produce these columns. For example, a `sum_state<decimal(38,2)>` column holds a running sum that another worker
can finish. See [Exchanges](../part-2/03-exchanges.md) and
[Aggregation](../part-3/02-aggregation.md). The single-phase aggregates in this tutorial return ordinary scalar types.

## Fields and schemas

A `Field` is a name, a type and a nullability. `Field::required(name, type)` declares a column
that never holds nulls, `Field::nullable(name, type)` one that may. A printed schema marks a
nullable field with a `?` after its type, `name type?`
([Plan notation](../reference/plan-notation.md#body)).

A `Schema` is an ordered list of fields. `Schema::create` rejects an empty name and a duplicate
name.

Sirius never guesses a schema: every table source and every input takes an explicit one. Here is
the `lineitem` table of the running example, with every field required:

```cpp
#include <sirius/sirius.hpp>
using namespace sirius;

Result<Schema> lineitem_schema() {
  SIRIUS_TRY(money, DataType::decimal(15, 2));
  return Schema::create({
      Field::required("l_orderkey", DataType::int64()),
      Field::required("l_partkey", DataType::int64()),
      Field::required("l_suppkey", DataType::int64()),
      Field::required("l_linenumber", DataType::int32()),
      Field::required("l_quantity", money),
      Field::required("l_extendedprice", money),
      Field::required("l_discount", money),
      Field::required("l_tax", money),
      Field::required("l_returnflag", DataType::string()),
      Field::required("l_linestatus", DataType::string()),
      Field::required("l_shipdate", DataType::date32()),
  });
}
```

```rust
use sirius::prelude::*;

fn lineitem_schema() -> Result<Schema, PlanError> {
    let money = DataType::decimal(15, 2)?;
    Schema::try_new(vec![
        Field::required("l_orderkey", DataType::int64()),
        Field::required("l_partkey", DataType::int64()),
        Field::required("l_suppkey", DataType::int64()),
        Field::required("l_linenumber", DataType::int32()),
        Field::required("l_quantity", money.clone()),
        Field::required("l_extendedprice", money.clone()),
        Field::required("l_discount", money.clone()),
        Field::required("l_tax", money),
        Field::required("l_returnflag", DataType::string()),
        Field::required("l_linestatus", DataType::string()),
        Field::required("l_shipdate", DataType::date32()),
    ])
}
```

`Schema::from_arrow` converts an existing Arrow schema.

## Columns are scoped

Expressions start from a relation. The simplest one is an **input**, a named stream of batches
that your program supplies when it runs the plan
([Prepare and execute](06-prepare-and-execute.md)). This one has the `lineitem` schema and runs
on the default worker set `local[1]`:

```cpp
SIRIUS_TRY(schema, lineitem_schema());
SIRIUS_TRY(lineitem, Relation::input("lineitem", schema));
SIRIUS_TRY(price, lineitem.column("l_extendedprice"));
```

```rust
let schema = lineitem_schema()?;
let lineitem = Relation::input("lineitem", &schema, &WorkerSet::local())?;
let price = lineitem.column("l_extendedprice")?;
```

`lineitem.column("l_extendedprice")` returns an `Expression` of type `decimal(15,2)`. The
expression records that its column belongs to `lineitem`: that is its **scope**.

The name resolves at `column()`. A misspelled name fails there with `invalid_plan` and a message
that lists the available columns:

```text
invalid_plan: no column "l_extendprice" in Input "lineitem"
  (l_orderkey, l_partkey, l_suppkey, l_linenumber, l_quantity, l_extendedprice, ...)
```

Programs can match on the error code. Diagnostic text may change.

A lookup is unambiguous within one relation. When a join outputs columns from both sides,
rename any colliding fields before building it. A condition over
both sides uses a `JoinScope` to say which side each column comes from; see
[Residuals and `JoinScope`](07-aggregates-joins-and-ordering.md#residuals-and-joinscope).

**Ask the relation you are about to extend for its columns.** To filter `lineitem`, build the
predicate from `lineitem.column(...)`. To project the filtered relation, use its columns.

## Literals and arithmetic

A literal is a typed `Value` wrapped with `Expression::literal`. It has no scope, so it combines
with an expression of any scope. Each type has a `Value` factory:

- `Value::boolean`, `Value::string`, one factory per integer and float type (`Value::int8` to
  `Value::uint64`, `Value::float32`, `Value::float64`) and `Value::timestamp(ticks, unit)` cannot
  fail.
- `Value::date32(year, month, day)` returns a `Result`, because February 30 is not a date.
- `Value::decimal(digits, precision, scale)` takes its digits as text, such as `"0.05"`, so the
  value never passes through a binary floating-point number. It returns a `Result` because the
  digits must fit the precision and scale.
- `Value::null(type)` is a null of a specific type. A null always has a type.

Arithmetic uses the named methods `add`, `sub`, `mul` and `div`; there is no operator
overloading. Each returns a `Result<Expression>`: both operands must be numbers of the same type,
from compatible scopes. Here is the discounted revenue as a helper that works on any relation
with the right columns:

```cpp
Result<Expression> revenue_of(const Relation& input) {
  SIRIUS_TRY(price, input.column("l_extendedprice"));
  SIRIUS_TRY(discount, input.column("l_discount"));
  SIRIUS_TRY(one, Value::decimal("1", 15, 2));                // 1 as a decimal(15,2)
  SIRIUS_TRY(kept, Expression::literal(one).sub(discount));   // 1 - l_discount
  return price.mul(kept);                                     // l_extendedprice * (1 - l_discount)
}
```

```rust
fn revenue_of(input: &Relation) -> Result<Expression, PlanError> {
    let price = input.column("l_extendedprice")?;
    let discount = input.column("l_discount")?;
    let one = Value::decimal("1", 15, 2)?;                 // 1 as a decimal(15,2)
    let kept = Expression::literal(one).sub(&discount)?;   // 1 - l_discount
    price.mul(&kept)                                       // l_extendedprice * (1 - l_discount)
}
```

The result is scoped to the relation the helper is given.

The literal `1` is a `decimal(15,2)` because `l_discount` is one
([No implicit conversion](#no-implicit-conversion)). With `Value::int32(1)` the subtraction
fails:

```text
invalid_plan: sub: operands have different types, int32 and decimal(15,2);
  cast one of them, or write the literal as a decimal(15,2)
```

The difference is a `decimal(15,2)` again, so it can meet `l_extendedprice` in `mul`, and the
product has scale 4. Every expression knows its result type; ask with `type()` in C++ or
`data_type()` in Rust.

## Predicates

Comparisons use `eq`, `ne`, `lt`, `le`, `gt` and `ge`. Each takes two operands of the same type
and returns a boolean. Booleans combine with `and_`, `or_` and `not_` (`and`, `or` and `not` in
Rust).

Here are two predicates over `lineitem`:

```cpp
SIRIUS_TRY(shipdate, lineitem.column("l_shipdate"));
SIRIUS_TRY(cutoff, Value::date32(1998, 9, 2));
SIRIUS_TRY(shipped, shipdate.le(Expression::literal(cutoff)));

SIRIUS_TRY(discount, lineitem.column("l_discount"));
SIRIUS_TRY(low, Value::decimal("0.05", 15, 2));
SIRIUS_TRY(high, Value::decimal("0.07", 15, 2));
SIRIUS_TRY(above, discount.ge(Expression::literal(low)));
SIRIUS_TRY(below, discount.le(Expression::literal(high)));
SIRIUS_TRY(band, above.and_(below));
```

```rust
let shipdate = lineitem.column("l_shipdate")?;
let cutoff = Value::date32(1998, 9, 2)?;
let shipped = shipdate.le(&Expression::literal(cutoff))?;

let discount = lineitem.column("l_discount")?;
let low = Value::decimal("0.05", 15, 2)?;
let high = Value::decimal("0.07", 15, 2)?;
let above = discount.ge(&Expression::literal(low))?;
let below = discount.le(&Expression::literal(high))?;
let band = above.and(&below)?;
```

In a text tree these print in infix form: `l_shipdate <= 1998-09-02` and
`l_discount >= 0.05 and l_discount <= 0.07`. Each literal has the type of the column it meets.

Types are checked at each step. These fail with `invalid_plan`:

- `discount.and_(below)`, because `and_` needs two booleans and `l_discount` is a decimal.
- `l_orderkey` compared with `Value::int32(1)`, because `l_orderkey` is an `int64`. Write
  `Value::int64(1)`.
- `lineitem.filter(discount)`, because a filter predicate must be boolean.

[Nulls and NaN](#nulls-and-nan) covers comparisons with nulls and NaN, and which rows a filter
keeps.

## Projection creates a new scope

Now filter the shipped items, compute the revenue *on the filtered relation*, and project three
columns:

```cpp
SIRIUS_TRY(shipped_rows, lineitem.filter(shipped));
SIRIUS_TRY(revenue, revenue_of(shipped_rows));
SIRIUS_TRY(columns, shipped_rows.pass_through({"l_orderkey", "l_linenumber"}));
columns.push_back(revenue.named("revenue"));
SIRIUS_TRY(projected, shipped_rows.project(std::move(columns)));
SIRIUS_TRY(plan, Plan::from_root(projected));
```

```rust
let shipped_rows = lineitem.filter(&shipped)?;
let revenue = revenue_of(&shipped_rows)?;
let mut columns = shipped_rows.pass_through(&["l_orderkey", "l_linenumber"])?;
columns.push(revenue.named("revenue"));
let projected = shipped_rows.project(columns)?;
let plan = Plan::from_root(&projected, "result")?;
```

`pass_through` turns column names into named expressions that copy those columns unchanged.
`named` gives a computed expression its output name. A projection outputs its columns in the
order listed and rejects duplicate output names.

Here is the plan, trimmed to its body:

```text
Output "result" on local[1]
└─ Project [l_orderkey, l_linenumber, revenue = l_extendedprice * (1 - l_discount)]
   └─ Filter l_shipdate <= 1998-09-02
      └─ Input "lineitem" on local[1]
```

This is Q1 of [Scan, filter and project](../part-3/01-scan-filter-project.md), on a single worker
and fed by an input instead of a scan.

A projection **creates a new scope.** `projected` has its own schema, `(l_orderkey int64,
l_linenumber int32, revenue decimal(...))`, and you refer to its columns with
`projected.column("revenue")`. An expression scoped to `lineitem` or to `shipped_rows` cannot be
used on top of `projected`, even for a column such as `l_orderkey` that passed through unchanged.

## A scope mismatch

Suppose a second input, `lineitem_archive`, has the same schema, and your program projects the
archive's revenue on top of `lineitem`:

```cpp
SIRIUS_TRY(archive, Relation::input("lineitem_archive", schema));   // same schema
SIRIUS_TRY(archived_revenue, revenue_of(archive));                   // scoped to archive
auto wrong = lineitem.project({archived_revenue.named("revenue")});
if (!wrong) {
  // wrong.error().code == ErrorCode::invalid_plan
  std::cerr << wrong.error().message << '\n';
}
```

```rust
let archive = Relation::input("lineitem_archive", &schema, &WorkerSet::local())?;
let archived_revenue = revenue_of(&archive)?; // scoped to archive
let wrong = lineitem.project(vec![archived_revenue.named("revenue")]);
if let Err(error) = &wrong {
    // error is PlanError::Invalid { .. }
    eprintln!("{error}");
}
```

The projection fails with `invalid_plan`:

```text
invalid_plan: Project: expression "revenue" refers to columns of Input "lineitem_archive",
  but the input of this Project is Input "lineitem"
```

The two relations have the same column names and types. Without scopes, this plan would be
accepted and would compute revenue over the wrong rows.

The same check applies inside expressions: multiplying `lineitem.column("l_extendedprice")` by
`archive.column("l_discount")` fails at the `mul`, because no single relation could evaluate
both.

The expression owns its scope. It stays valid after your program releases the relation handle
the column came from.

## Casts, null tests and function calls

Three more methods complete the expression vocabulary:

- **`cast(type)`** converts explicitly, for example `quantity.cast(DataType::float64())`.
- **`is_null()`** returns a boolean that is true where the value is null. `not_()` turns it into
  "is not null".
- **`Expression::call(function, args)`** calls a named function, such as a string or date
  function. Sirius looks the name up in the built-in *registry*, `Registry::builtin()`, to get the
  call's result type. An unknown name fails at the call with `invalid_plan`. Each registry entry
  also carries a determinism class; see
  [Determinism classes](../reference/extension-points.md#determinism-classes).

A registry is an immutable value that needs no context. A host with functions of its own extends
the built-in registry with `Registry::builder()` and passes the result to
`Expression::call_in(registry, function, args)`.

The registry only says what a name means. Whether this engine build implements the function is
answered later, by [`context.check`](03-context-and-session.md#checking-a-plan) and preparation,
with `unsupported`. The built-in function catalog is
[open question 16](../reference/open-questions.md#16-expression-semantics).

## No implicit conversion

Sirius never converts a value to another type on its own. If two types differ, you write a
`cast` or the plan is not built.

- **Operands have the same type.** `add`, `sub`, `mul` and `div` take two numbers of the same
  `DataType`, and the six comparisons take two values of the same `DataType`. For decimals that
  means the same precision and scale. An `int32` with an `int64`, an integer with a decimal, or
  a `decimal(15,2)` with a `decimal(12,4)` fails at the call with `invalid_plan`.
- **A literal has the type of its `Value`**, whatever it is combined with.
- **`cast` is the only conversion.** To combine two columns of different types, cast one to the
  type of the other. The cast is an expression, so the plan prints it.
- **Arguments match exactly.** A function's registry entry states its argument types, and `call`
  and `call_in` convert nothing to fit. The same holds for the argument of a measure.

A front-end whose language promotes types, as SQL does, writes those promotions as casts when it
builds the plan. No type is left for an engine to choose, so every process that builds the query
gets the same types.

Result types need few rules. A comparison returns a boolean. `add`, `sub` and `mul` return the
type of their operands, except `mul` on decimals: the product of two `decimal(p,s)` values has
scale 2s, which is why `revenue` has scale 4. The precision of that product, the result of `div`
and overflow behavior are part of
[open question 16](../reference/open-questions.md#16-expression-semantics).

**Join keys, hash keys and union inputs never coerce** either. Keys are column names, so there is
no expression to put a cast in, and Sirius never inserts one. Union inputs must have identical
types, column by column, and so must the two keys of a join pair.

To join on keys of different types, cast one of them in a projection first, so the plan shows the
conversion as a column of its own. With more workers, cast before the rows are spread by hashing
that key: the cast column is a new column, and rows hashed on the old one are not hashed on it.

Identical key types also let workers agree: every worker must hash equal keys to the same bits,
or equal keys land on different workers. With one type on both sides, the hash depends on the
type alone.
[Hash canonicalization in v1](../reference/extension-points.md#hash-canonicalization-in-v1) says
how each type hashes.

## Nulls and NaN

**A comparison with a null is null.** `eq`, `ne`, `lt`, `le`, `gt` and `ge` return null when
either operand is null. The result is nullable if either operand is nullable, and required
otherwise. A column is as nullable as its field, and a literal is nullable only when it is
`Value::null`. A projection that names the comparison gets a field of the same nullability. To
test for null, use `is_null()`, which never returns null.

**A filter keeps only true rows.** `filter` drops a row when its predicate is false or null. If
`l_discount` were nullable, `l_discount >= 0.05` and `l_discount < 0.05` would both drop the rows
where it is null. A join residual follows the same rule: a pair of rows matches only when the
residual is true.

**Floating-point values compare in one total order.** `-0.0` equals `+0.0`. Every NaN equals
every other NaN and is greater than every other value, positive infinity included. So `x.eq(x)`
is true for every `x` that is not null, and NaN sorts after every other number under `asc` and
before them under `desc`.

Everything that compares values uses this order: the six comparisons (in a filter, a residual or
a predicate pushed into a scan), sort keys, `min` and `max`, group keys, window keys and join
keys. Equal values also hash alike
([Hash canonicalization in v1](../reference/extension-points.md#hash-canonicalization-in-v1)).
Where two equal values differ in their bits, as `-0.0` and `+0.0` do, which one a `min`, a `max`
or a group key returns is the engine's choice.

Null is not part of this order. A sort key places nulls with `nulls_first` or `nulls_last`, group
keys and window keys put all nulls in one group, and a null join key matches nothing unless its
pair is null-safe ([Null-safe keys](07-aggregates-joins-and-ordering.md#null-safe-keys)).

How `and`, `or`, arithmetic and function calls treat nulls is part of
[open question 16](../reference/open-questions.md#16-expression-semantics).

## Keys are names, values are expressions

Everything built in this chapter is a *value*, a scoped expression. *Keys* are different: group
keys, window keys, join keys, hash keys, runtime-filter keys and sort keys are plain column
**names** of the input. To group or join on something computed, project it first and use its
name. [Conventions](../conventions.md#keys-are-names-values-are-expressions) explains why.
