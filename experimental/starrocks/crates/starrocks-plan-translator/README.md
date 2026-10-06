# StarRocks plan translation

`translate_fragment(&TExecPlanFragmentParams)` emits one Substrait fragment. Its
exchange boundaries use the shared
[Sirius exchange schema](../../../../proto/sirius/exchange/v1/exchange.proto).

`EXCHANGE_NODE` becomes a `ReadRel` containing the receiving fragment identity,
exchange node ID, expected sender IDs, and descriptor-derived schema.
`DATA_STREAM_SINK` becomes an `ExchangeRel`: one unpartitioned destination gathers,
several unpartitioned destinations broadcast, and `HASH_PARTITIONED` scatters by
plain slot references. Hash keys follow the final fragment output column order.
Sink projections cast each output to its declared exchange type, including the
`BIGINT` result of an integer sum. Character columns travel as `VARCHAR`; binary
columns are rejected because Sirius has no native binary column type.

The execution parameters must include query and fragment IDs, a sender ID for a
sink, destinations for a sink, and `per_exch_num_senders` entries for every source.
Both halves of each StarRocks identifier are preserved. A source expects sender
IDs from zero through its declared count minus one.

Each destination's BRPC address determines its peer name:
`starrocks://<hostname>:<port>`. The embedding caller must use this exact name when
enabling that worker's Sirius exchange agent, distribute its opaque bootstrap
metadata, and register the metadata with its senders. For example, the worker
whose destination address is `worker-0:8060` initializes itself with:

```rust,ignore
context.enable_exchange(
    "starrocks://worker-0:8060",
    64 * 1024 * 1024,
    std::time::Duration::from_secs(30),
)?;
let bootstrap = context.exchange_metadata()?;
// Give bootstrap to the coordinator for registration on the sending workers.
```

After registering peers, translate and build the fragment with
`context.fragment(&translate_fragment(&params)?.to_substrait_bytes())?`. Build
receivers before running senders. See the
[embedding lifecycle](../../../../docs/super-sirius/exchanges.md#embedding-lifecycle)
for execution and result collection. The CN service still executes only its
existing result fragments; distributed CN orchestration is separate from this
translator.

Merging exchanges, prefix-only input tuple layouts, multicast sinks, nonzero
pipeline driver sequences, sink column pruning/limits, and random/range/bucket
partitioning are rejected. Every worker in this protocol must be Sirius; these
plans do not implement StarRocks' native batch transport or hashing protocol.

Run the translator tests without a GPU or native Sirius build:

```bash
cd experimental/starrocks
pixi run -e cn cargo test -p starrocks-plan-translator
```

The Rust tests compare generated plans with the small fixtures in
[`test/cpp/exchange/data`](../../../../test/cpp/exchange/data). Native parser tests
consume those same files. After an intentional change to the plan format,
regenerate them by setting `UPDATE_EXCHANGE_FIXTURES=1` on the test command, then
rerun the tests without that variable.

The metadata Rust crate generates its types from the shared schema during Cargo
builds. C++ types are checked in for the bundled protobuf 3.19.4 runtime; regenerate
them from the repository root with:

```bash
pixi exec --spec protobuf=3.19.4 -- python scripts/generate_exchange_proto.py
```
