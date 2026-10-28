# Public plan construction API proposal

**Status:** draft for discussion. Types and examples describe a proposed API,
not implemented contracts or finalized signatures. Introduce capabilities gradually
through small, independently reviewable changes to `main`.

## Relationship to execution

This proposal extends the [query execution API design](query-execution-api-design.md).
Programmatic construction and Substrait import produce the same immutable `Plan`:

```text
Substrait bytes → import ──────┐
                              ├→ Plan → PreparedPlan → Execution
C / C++ / Rust construction ───┘
```

Construction does not need a context, session, GPU, or active input channel. A plan
is a description; preparation binds it to execution resources and endpoints.
The existing execution ownership, cancellation, and streaming contracts apply
regardless of how the plan was created.

The C ABI exposes construction primitives. C++ and Rust provide idiomatic builders
on top, using the shared public error model (`std::expected` and `Result`). No C++
objects or exceptions cross the ABI. Binary packaging remains separate work.

## Objects and invariants

| Object | Responsibility |
|---|---|
| `DataType` / `Schema` | Owned immutable type and field descriptions, shared with execution and Arrow adapters. |
| `Expression` | Owned immutable expression with a known result type and explicit input scope. |
| `Relation` | Complete immutable relational operation, its input descriptions, and output schema. |
| `Plan` | Complete immutable description with named input requirements and an explicit output root. |

Start with one root. Multiple outputs, catalog-dependent relations, joins, and
extensible functions are separate additions, not mandatory fields in the initial API.

Factories accept all information needed for an operation and either return a valid
object or an error. There is no publicly observable incomplete relation, deferred
`set_input()`, or `initialize()` step. Construction objects have no public empty
constructor in the language wrappers. C++ moved-from values still need an explicit
restricted-use contract; Rust moves transfer ownership.

A C++ builder can return a new relation for each fluent operation. Rust can choose
consuming convenience methods. Neither requires a mutable C builder handle or a
shared cursor whose current relation changes after each call.

### Column scope

Resolve a column name or position against a relation to produce a scoped expression.
Reject ambiguous names and out-of-range positions at that point. A same-shaped
schema on an unrelated relation does not make its column references interchangeable.

A filter predicate must reference its input scope and have a boolean result type.
A projection accepts expressions from its input scope and establishes an output
schema and a new scope. Callers obtain subsequent column references from that
output relation. Literals have no input dependency; later expression factories
combine compatible scopes and reject incompatible ones.

Scope metadata is owned. Releasing a temporary relation handle cannot leave a
column reference dangling. Enforce these rules in the C implementation so all
wrappers inherit them; language-level typing can provide additional guarantees.

When joins are added, design left/right scope composition and self-join aliases
explicitly. Do not infer the side of a column solely from its name or data type.

### Immutable sharing

New relations retain their inputs and expressions. Reuse immutable nodes without
copying an entire plan. A parent is created only from existing complete children;
there is no API for mutating edges or introducing cycles afterward.

Sharing a description does not promise shared execution, caching, or evaluation
exactly once. Repeated reads of a named input also need a defined binding model;
initially reject plans that would require multiple consumers of one input endpoint.
Explicit broadcast and execution-graph sharing are later capabilities.

## Construction versus preparation

| Phase | What it establishes |
|---|---|
| Construction or import | Owned, structurally valid descriptions and all type/schema facts available without engine resources. |
| Preparation | Catalog and executable function resolution where needed, engine support, endpoint compatibility, resource snapshots, and output routing. |
| Execution | Performs the prepared work; reports allocation, I/O, device, and other operational failures. |

The first construction API requires explicit input schemas and supports a bounded
set of types and operations. It never guesses a schema from an unresolved catalog
name. Substrait import may carry references that require preparation to resolve;
schema-dependent construction methods must not pretend those references are resolved.

Both paths should converge on a common owned representation and preparation path.
Do not require a programmatically constructed plan to serialize to Substrait and
immediately parse again. Import adapters may initially support a smaller common
subset. Substrait export and arbitrary imported-plan editing are separate features.

## C ABI sketch

Proposed headers live under `sirius-c/plan/`; C++ wrappers under `sirius/plan/`.
Reuse shared schema/type objects rather than defining a second schema model for
construction. Exact schema header locations and function names remain to be settled.

```c
/* Opaque immutable handles; each returned handle owns one reference. */
typedef struct sirius_relation sirius_relation;
typedef struct sirius_expression sirius_expression;
typedef struct sirius_plan sirius_plan;

/* Schema construction/import belongs to the shared schema API. */
sirius_status sirius_relation_input(
    const char *name, size_t name_size, const sirius_schema *schema,
    sirius_relation **out_relation, sirius_error **out_error);

/* The returned expression owns its scope and type metadata. */
sirius_status sirius_relation_column(
    const sirius_relation *input, const char *name, size_t name_size,
    sirius_expression **out_expression, sirius_error **out_error);

/* Borrowed descriptors; projection copies names and retains expressions. */
typedef struct sirius_named_expression {
    const char *name;
    size_t name_size;
    const sirius_expression *expression;
} sirius_named_expression;

sirius_status sirius_relation_project(
    const sirius_relation *input,
    const sirius_named_expression *columns, size_t column_count,
    sirius_relation **out_relation, sirius_error **out_error);

void sirius_relation_retain(sirius_relation *relation);
void sirius_relation_release(sirius_relation *relation);
/* Expression and Plan have corresponding retain/release operations. */

/* Illustrative later operation, once boolean expressions are available. */
sirius_status sirius_relation_filter(
    const sirius_relation *input,
    const sirius_expression *predicate,
    sirius_relation **out_relation,
    sirius_error **out_error);

/* First stage: form a complete plan from a single output root. */
sirius_status sirius_plan_from_root(
    const sirius_relation *root,
    sirius_plan **out_plan,
    sirius_error **out_error);
```

The sketches omit export, result-use, and C/C++ linkage macros; actual headers use
the common C API declaration conventions. Every immutable handle has retain/release operations. Factories borrow arguments
for the duration of the call and retain or copy everything the result needs. Names
use explicit byte lengths with documented encoding and embedded-NUL rules; arrays
use pointer/count pairs. No caller-owned argument storage remains borrowed afterward.

Required result slots are cleared before construction. Failure leaves them empty,
releases partial internal work, and preserves input handles. Optional diagnostics
follow the existing C API convention. No exceptions cross the C boundary. Result
slots require exclusive access and must not alias input-handle storage.

Retaining a handle acquires another owning reference; releasing one reference does
not invalidate other owning references. Borrowed pointers and stale aliases are not
additional references. Document null handling for each function.

## C++ shape and execution integration

Illustrative signatures, with ownership boilerplate omitted:

```cpp
class Relation {
 public:
  static Result<Relation> input(std::string name, const Schema& schema);
  Result<Expression> column(std::string_view name) const;
  Result<Relation> project(std::span<const NamedExpression> columns) const;
  Result<Relation> filter(const Expression& predicate) const; // later stage
};

class Plan {
 public:
  static Result<Plan> from_root(const Relation& root);
  static Result<Plan> from_substrait(std::span<const std::byte> bytes);
};
```

`NamedExpression` pairs an owned output name with an expression. Projection defines
field order explicitly. The initial API should reject duplicate output names rather
than leave lookup ambiguous. The plan's named input requirements are derived from
its reachable relations; conflicting declarations fail construction.

For the first example, build an `orders` input from an explicit schema, select its
`order_id` column, project it, and call `Plan::from_root`. Each fallible step handles
its `Result`. Then use the existing execution API:

```cpp
// plan is a successfully constructed Plan; inputs owns its input endpoints.
auto prepared = session.prepare(plan, std::move(inputs), OutputRouting::single());
if (!prepared) { return std::unexpected(std::move(prepared.error())); }
auto started = std::move(*prepared).start();
```

Input relations own names and schemas, not `InputSource` or `InputWriter` instances.
Preparation consumes input endpoints as specified by the execution proposal. A
plan can be prepared again with fresh bindings; this does not imply concurrent
execution or reusable prepared templates.

## Thread safety and lifetimes

All contracts below are design targets requiring an implementation audit:

| Objects or operations | Intended contract |
|---|---|
| Types, schemas, expressions, relations, plans | Immutable, transferable, concurrently readable; Rust `Send + Sync`. |
| Construction factories | Independent calls may borrow the same immutable inputs concurrently. |
| Retain/release | Distinct owning references may be managed concurrently; keep a live reference throughout every use. |
| Destruction | Allowed on any thread; no GPU synchronization or foreign callbacks. |
| Output slots | One caller has exclusive access for the entire call. |

Construction may allocate and perform CPU work. It does not wait for query input,
perform catalog/network I/O, or initialize CUDA. Schema import copies the necessary
metadata instead of retaining foreign release callbacks. Plan descriptions retain
no engine runtime, session, live endpoint, or foreign data buffers.

Every shipped type and method must state its ownership, failure cleanup, concurrent
use, destruction-thread, blocking, and callback contracts in the public header.
Any future API retaining external resources must explicitly revise the relevant
contract rather than silently weakening these guarantees.

## Incremental delivery

Each step is a separate reviewable increment. Land only the operations needed for
its example and defer further vocabulary until there is a concrete use case.

1. **Shared schema foundation.** Establish owned schema/type descriptions, a small
   supported type subset, and C ownership/thread contracts. Reuse the execution
   schema work if it lands first. No new execution behavior.
2. **Smallest complete C plan.** Named input, scoped column reference, projection,
   one root, retain/release, and errors. Demonstrate construction and destruction
   without a context or GPU. Connect it to preparation when that API is available;
   do not add a temporary second execution interface.
3. **C++ convenience layer.** Wrap that exact subset with RAII and `std::expected`.
   Add a complete construction example and execution example when available.
4. **Typed expressions and filtering.** Add a small literal/comparison subset,
   explicit nullability and coercion semantics, and boolean filter predicates.
   Keep the C++ wrapper aligned with the capabilities added to C.
5. **Rust convenience layer.** Wrap the established subset with safe ownership,
   `Result`, and audited `Send`/`Sync`. It need not wait for a comprehensive builder.
6. **Use-case-driven extensions.** Consider aggregation, joins, sorting, limits,
   scans, function extensibility, and multiple roots independently. Define semantics
   and failure ownership for each increment before expanding its API.

There is no requirement to finish this list before merging the first useful step.
SQL parsing, optimizer hints, physical operator selection, custom callbacks, and
serialization round trips are outside the initial scope.

### Verification per increment

- C99 header and installed-library compile/link coverage.
- Successful construction and errors for scope, schema, names, and operand types.
- Partial-construction allocation failures with no leaks or input invalidation.
- Release temporary handles before their dependent expressions/relations/plans.
- Concurrent construction from shared immutable inputs and cross-thread destruction.
- C++ ownership behavior and Rust trait checks as each wrapper lands.
- Equivalent preparation and query results for supported imported and constructed
  plans once execution integration exists; compare semantics, not internal layout.
- Matching public examples and ownership/thread-safety documentation for every
  capability actually shipped.

## Decisions to settle before implementation

- The initial scalar type subset and schema construction/import API.
- Exact column scope identity and diagnostics; reserve self-join alias design for joins.
- Name encoding and duplicate-name rules across import and construction.
- The internal representation shared with Substrait import, including unresolved references.
- Nullability, comparison, and coercion rules before adding expression operators.

Prefer resolving these for the next small increment rather than designing every
future operator up front.
