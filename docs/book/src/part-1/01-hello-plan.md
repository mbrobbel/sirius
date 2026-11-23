# Your first plan

This example uses the proposed plan API to read two columns from a Parquet file and keep nations
with a key below five. It runs on one logical worker. Building and inspecting the plan requires
only a CPU. Preparing and running it requires an engine and a GPU.

## The whole program

```cpp
#include <sirius/sirius.hpp>
using namespace sirius;

Result<void> hello() {
  SIRIUS_TRY(config, ContextConfigBuilder{}.build());
  SIRIUS_TRY(context, Context::create(config));

  SIRIUS_TRY(schema, Schema::create({Field::required("n_nationkey", DataType::int32()),
                                     Field::required("n_name", DataType::string())}));
  SIRIUS_TRY(nation, TableSource::parquet("nation", {"data/nation.parquet"}, schema));

  SIRIUS_TRY(scan, Relation::scan(nation, ScanOptions::defaults()));
  SIRIUS_TRY(key, scan.column("n_nationkey"));
  SIRIUS_TRY(small, key.lt(Expression::literal(Value::int32(5))));
  SIRIUS_TRY(filtered, scan.filter(small));
  SIRIUS_TRY(plan, Plan::from_root(filtered));
  std::cout << plan.explain();

  SIRIUS_TRY(prepared, context.prepare(plan, Bindings::local()));
  SIRIUS_TRY(result, std::move(prepared).collect());
  std::cout << result.batches.size() << " batches\n";
  return {};
}
```

```rust
use sirius::prelude::*;

fn hello() -> sirius::Result<()> {
    let config = ContextConfigBuilder::new()?.build()?;
    let context = Context::try_new(&config)?;

    let schema = Schema::try_new(vec![Field::required("n_nationkey", DataType::int32()),
                                      Field::required("n_name", DataType::string())])?;
    let nation = TableSource::parquet("nation", ["data/nation.parquet"], &schema)?;

    let scan = Relation::scan(&nation, ScanOptions::default())?;
    let key = scan.column("n_nationkey")?;
    let small = key.lt(&Expression::literal(Value::int32(5)))?;
    let filtered = scan.filter(&small)?;
    let plan = Plan::from_root(&filtered, "result")?;
    println!("{}", plan.explain(&ExplainOptions::default()));

    let prepared = context.prepare(&plan, Bindings::local())?;
    let result = prepared.collect()?;
    println!("{} batches", result.batches.len());
    Ok(())
}
```

`TableSource::parquet` records the file path and the schema. A file can contain additional
columns. `Field::required` promises that the named column contains no nulls.

`scan.column("n_nationkey")` looks up the column in the scan's schema. A misspelling fails here.
`lt` checks that the key and the literal have the same type. The filter keeps rows where the
comparison is true. Each call returns a new value, so the original scan can feed another branch.

`Plan::from_root` adds an output named `result` and checks the graph. See
[Relations and plans](05-relations-and-plans.md) for the checks and the rules for sharing nodes.

## Reading the plan

`plan.explain()` prints the plan as a text tree. Above the tree it prints a header with the plan's
form and its worker sets, which here starts `Plan (joined)`. Most trees in Part I leave the header
out; [Plan notation](../reference/plan-notation.md#header) describes it.

```text
Output "result" on local[1]
└─ Filter n_nationkey < 5
   └─ Scan nation [n_nationkey, n_name]  on local[1]  shards=files_round_robin
```

Read from the leaves up. The scan assigns its files round-robin to the workers in `local[1]`.
Here the set has one worker, `local#0`. The filter and output run on that worker too.

## Running the plan

The plan contains the logical worker's name. At `context.prepare(plan, Bindings::local())`, the
context assigns it to the first selected GPU. Preparation checks support, binds resources and
resolves any local property the plan leaves as `auto`. This example has none.

`collect()` consumes the prepared plan, reads the batches for `result` and waits for the outcome.
Batch sizes may vary. You can reuse the immutable plan with fresh bindings to run it again.
[Prepare and execute](06-prepare-and-execute.md) shows how to read large results as a stream.

## When something goes wrong

Every step can fail. The error has a code you can match on and a message for humans. In C++ the
code is a field of the one `Error` type. In Rust every step has its own error enum, and the
variant is the code ([Results and errors](../conventions.md#results-and-errors)).

Sirius reports each mistake at the earliest step that can detect it:

| Step | Calls | Typical error codes | Rust enum |
|---|---|---|---|
| Configuration | `ContextConfigBuilder::from_yaml`, `build` | `configuration_io`, `malformed_yaml`, `invalid_configuration` | `ConfigError` |
| Engine start | `Context::create` | `context_initialization` | `ContextError` |
| Plan construction | `Schema::create`, `column`, `lt`, `filter`, `Plan::from_root` | `invalid_plan` | `PlanError` |
| Support check | `Context::check`, `Context::prepare` | `unsupported` | `PrepareError` |
| Binding | `Context::prepare` | `binding_mismatch` | `PrepareError` |
| Running | `collect`, readers, `wait` | `execution`, `source_mismatch`, `transport`, `cancelled` | `ExecutionError` |

A plan that reaches `prepare` is already well-formed. Prepare checks only whether this engine can
run it and whether the bindings fit.

Four more codes are not tied to a step. `allocation_failure`, `invalid_argument` and
`internal_error` come from the C ABI underneath and can appear anywhere;
[Errors](../reference/api-at-a-glance.md#errors) lists their Rust variants. `runtime_unavailable`
can appear on any call that uses the engine. It means the engine failed for good, for example
after a fatal device error, and stays that way until the process restarts (see
[How long a context lives](03-context-and-session.md#how-long-a-context-lives)).

An error about a plan can carry a `PlanLocation` that names the node at fault, so a tool can
point at the right line of the tree.
