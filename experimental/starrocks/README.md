# Sirius StarRocks compute node

The compute node translates each StarRocks plan fragment into two separate artifacts:

- a Substrait plan consumed by Sirius;
- CN-owned exchange metadata containing StarRocks fragment destinations, node ids, sender ids,
  sender counts, and partitioning semantics.

`SiriusEngine` runs a single coordinator on its dedicated engine thread. The coordinator owns the
only mutable `SiriusContext`, inspects `SubstraitPlan::input_streams` and `output_streams`, and binds
those opaque `StreamId` values to the CN metadata before creating a `StreamSession`. Sessions and
the context never leave the coordinator. Fragments execute back-to-back because Sirius does not
yet support concurrent queries.

Exchange I/O runs as Tokio tasks and communicates with the coordinator through channels carrying
owned Arrow batches. The current `LocalExchangeTransport` uses bounded in-process mailboxes. Its
async `ExchangeTransport` boundary is intentionally independent of Sirius so a Nixl-agent
implementation can replace the mailbox without moving StarRocks routing semantics into the Sirius
Rust crate.

The compatibility path currently supports one exchange input, one sender, one Arrow batch, one
Substrait output stream, and unpartitioned broadcast output. Merging exchanges, offsets,
sink-side projections/limits, and hash/random partitioning are rejected instead of silently
changing StarRocks semantics.
