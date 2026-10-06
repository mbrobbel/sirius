# Exchange plans and operators

Exchange boundaries describe GPU batches moving between Sirius fragments. The
physical operators `EXCHANGE_SOURCE` and `EXCHANGE_SINK` inherit the repository,
partitioning, and sender completion mechanics described in
[Streaming Sessions](streaming-sessions.md). `stream_boundary::exchange` selects
these operators when constructing a native streaming fragment. Exchange sources
preserve their bound scan contracts and prohibit CPU replay.

## Plan format

The shared schema is [exchange.proto](../../proto/sirius/exchange/v1/exchange.proto).
Its versioned messages are carried in Substrait `Any` values:

| Boundary | Substrait location | Metadata |
|---|---|---|
| Source | `ReadRel.extension_table.detail` | `sirius.exchange.v1.ExchangeSource` |
| Sink | `ExchangeRel.advanced_extension.enhancement` | `sirius.exchange.v1.ExchangeSink` |
| Destination | `ExchangeRel.targets[i].extended` | `sirius.exchange.v1.ExchangeDestination` |

The type URL prefix is `type.googleapis.com/`. Query and fragment IDs retain both
64-bit halves. Incoming routes are identified by query ID, receiver fragment ID,
and exchange ID. The source declares its expected sender IDs and flat scalar
schema in `ReadRel.base_schema`. A destination's `peer_id` identifies its receiving
peer; peer discovery belongs to the embedding caller.

A fragment has one `PlanRel.root`. Its root may be an `ExchangeRel` sink, with one
explicit destination per partition. Supported routing is:

- Gather: one partition, `single_target` with a constant zero bucket.
- Hash: `scatter_by_fields` with direct references to output columns.
- Broadcast: `broadcast`, with every target receiving the complete output.

Nested sinks, computed partition expressions, round-robin routing, and additional
enhancements on exchange boundaries are rejected. Hash key indices refer to the
bound output schema. The streaming sink normalizes supported key types before
hashing so independent senders use the same representation.

`rewrite_substrait` extracts typed exchange declarations, replaces source reads
with named stream views, and removes the root exchange sink. It also normalizes
constant fetch expressions to the representation supported by the bundled DuckDB
Substrait importer. Plans without exchange boundaries retain their original bytes.
The returned declarations let an embedding caller bind the stream views and
configure the fragment's exchange operators before importing the rewritten plan.
The DuckDB Substrait importer does not need changes.

The schema has generated C++ bindings and a shared Rust crate,
`sirius-exchange-proto`. Native tests cover metadata parsing, rewriting, routing,
operator selection, and preservation of scan contracts.
