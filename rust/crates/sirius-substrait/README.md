# Minimal Sirius Substrait consumer

This prototype translates ordinary binary Substrait into immutable Sirius plan
handles through the public C ABI, then executes them on `sirius::Context` from Rust:

```text
Substrait protobuf → Rust consumer → public plan builder → Sirius GPU execution → Arrow batches
```

The consumer does not call `SubstraitToDuckDB` or the DuckDB Substrait extension.
The engine adapter still uses DuckDB's existing Parquet table-function binder and
scan planner internally. It constructs filter operators directly with Sirius AST
expressions and runs them through Sirius's scheduler and materialized collector.
This is a first replacement path, not removal of every internal DuckDB dependency.

## Supported slice

- One root and one local Parquet `uri_file` (`file:///...`).
- An explicit flat schema of nullable signed 64-bit fields, matching file names,
  order, and types. No implicit casts or schema inference.
- `ReadRel`, optional mandatory read filter, and `FilterRel`.
- A direct input column compared with an i64 literal using `equal`, `not_equal`,
  `lt`, `lte`, `gt`, or `gte` from `extension:io.substrait:functions_comparison`.
- Direct output mappings; root output names must match input field names.
- Blocking materialized execution with CPU Arrow output and no CPU query fallback.

The optional best-effort read filter and optimization hints may be ignored as allowed
by Substrait. Unsupported semantic features return an error: projection/emit
remapping, joins, aggregates, custom extensions, nonlocal files, byte ranges,
multiple files, nested fields, and other scalar expressions. This is not a sandbox:
plans may read local files accessible to the process. Embedders must authorize paths
before accepting plans from untrusted callers.

Construction performs no file access or GPU initialization. Files must remain
unchanged during execution. Relations, expressions, and plans own their descriptions,
are immutable, and support concurrent borrowing and cross-thread destruction.
`Context` remains thread-confined and supports one live engine per process.

The initial C execution entry point combines preparation and execution in one
blocking call. It returns completed data, with no public initialized/running flags.
It can later be factored into the proposed prepared-plan lifecycle without putting
execution state into the immutable plan. Live input/output channels are not part of
this slice.

C callers release the returned Arrow stream on its creation thread. The stream
retains its runtime resources, including if the context handle is dropped first;
release it before creating another context. The Rust wrapper drains and releases
the stream before returning owned Arrow batches.

## Example

Build the native library first, then run the example on a machine with a supported GPU:

```bash
pixi run cmake --build build/release --target sirius_shared -j 8
LD_LIBRARY_PATH="$PWD/build/release${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
  pixi run cargo run --manifest-path rust/Cargo.toml -p sirius-substrait --example scan_filter
```

An optional final argument is a YAML context configuration path. Set
`SIRIUS_BUILD_DIR` if the library is in a different build tree. The example creates a
temporary Parquet file with `[1, NULL, 20, 30]`, encodes a standard Substrait `id > 10`
filter, constructs the Sirius plan before initializing the context, and prints the
surviving values `20` and `30` (order is not guaranteed).

The application-facing sequence is:

```rust,no_run
# fn run(bytes: &[u8], context: &mut sirius::Context) -> Result<(), Box<dyn std::error::Error>> {
let plan = sirius_substrait::from_bytes(bytes)?;
let result = context.execute_plan(&plan)?;
for batch in result.batches {
    println!("{} rows", batch.num_rows());
}
# Ok(())
# }
```

The builder is also usable directly without Substrait; see the `sirius::Relation`
module example. Follow-ups should add one capability at a time rather than widen
this prototype into a complete SQL frontend.
