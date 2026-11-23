# One IR, three integrations

Who decides where an order and its line items meet? In DuckDB, the extension can plan both
sides of the join. In StarRocks, the frontend may already have sent the two sides to different
compute nodes. The proposed API needs to express both arrangements.

## Who owns what

| In the proposed integration | DuckDB extension | Distributed Sirius | StarRocks compute node |
|---|---|---|---|
| Who supplies the plan? | The extension translates an optimized DuckDB plan. | Each process runs the same builder. | The frontend schedules fragments. Each node translates the instances it receives. |
| How much can one caller see? | The whole query. | The whole query, including remote workers. | One instance and the edges it shares with peers. |
| Who assigns scan work? | DuckDB supplies a snapshot or files. The extension chooses a shard rule. | The builder supplies files and a shard rule. | The frontend supplies ranges for each instance. |
| What does the caller bind? | Workers to GPUs in this process. | Workers to endpoints across processes, with a common execution ID. | The local instance and its peers to endpoints, with the frontend's execution ID. |
| Who consumes the result? | The extension feeds batches into DuckDB. | The process hosting the output worker. | The compute node hosting the result fragment answers the frontend. |

An **exchange** appears wherever Sirius carries rows across workers. A **binding** identifies
the endpoint that hosts each worker. The kernel, transport and batch size can change without
changing which worker receives an order.

## One query, three plans

In [Q4](../part-3/04-partitioned-join-and-runtime-filters.md), four workers join March's urgent
orders to line items and compute revenue per order. The build side also produces a filter.
Applying it early saves shuffling line items that cannot match.

| Plan detail | DuckDB extension and Distributed Sirius | StarRocks example |
|---|---|---|
| Plans built | One closed plan with both ends of every edge | Thirteen open plans, one per fragment instance |
| Join inputs | Two hash exchanges to `compute[4]` | Exchanges `x11` and `x12` to `frag3[4]` |
| Filter | A Bloom filter built at each orders scan, merged for all four lineitem workers | An exact key list built *after* the orders exchange. Each join worker sees one partition, so the scan needs all four shares. |
| Filter wait | The scan starts immediately and uses the filter when available. | The frontend allows a 30-second wait. An oversized list falls back to key bounds. |
| Result | A gather to `head[1]` | A gather to `frag0[1]` |

DuckDB and Distributed Sirius can use exactly the same Q4 plan. Binding it to GPUs in different
processes changes which transfers cross the network. In the StarRocks example, the frontend
has changed the scan ranges, filter placement and timeout. Those differences belong in the
plan and remain visible when the thirteen instances are composed for inspection.

## Where the common API still needs work

- **A DuckDB snapshot needs an identity and a lifetime.** The host must keep the snapshot readable
  until Sirius finishes. Comparing plans requires an identity that remains meaningful outside
  the process that owns the handle. [Host-registered scan kinds](../reference/open-questions.md#14-host-registered-scan-kinds)
- **StarRocks needs independently built fragments to agree.** An exchange signature can catch a
  mismatch on a connected edge. It cannot tell whether the frontend launched every fragment
  or whether all local instances can run concurrently. [Open instance plans](02-starrocks.md#open-instance-plans) · [Admission across processes](../part-5/08-running-at-scale.md#admission-across-processes)
- **Distributed Sirius needs a recovery boundary.** If rank 0 has delivered rows to a client and a
  peer fails, rerunning the plan could deliver the same rows twice. Recovery depends on which
  outputs have been committed and who can discard or replay them. [Failure and recovery](../part-5/07-failure-cancellation-and-recovery.md)
- **A foreign shuffle needs compatible data.** Both sides must agree on partition IDs, nulls,
  types and any aggregate states on the wire. The name of a hash function alone is insufficient.
  [Hash scheme stability](../reference/open-questions.md#18-hash-scheme-stability) · [Dialect semantics](05-plans-from-other-systems.md#dialect-semantics)

## Beyond three hosts

The amount of a query a host gives Sirius determines who owns each exchange.

| Boundary | What crosses it |
|---|---|
| Whole query | A closed plan. Sirius owns every exchange in it. An Axiom runner could use this form for a supported multi-fragment plan. |
| Fragment instance | The host supplies placement and peers. Sirius can carry rows directly if both ends implement the same exchange protocol. |
| Host task | Rows enter through an `Input` and leave through an `Output`. A Spark task could use this form while Spark keeps its shuffle, retries and task scheduling. |
| Operator | The host keeps the surrounding pipeline. Each offloaded segment must justify the cost of moving its inputs and outputs. |

Arrow can carry batches at any of these boundaries. It describes the buffers and schema.
The hosts still have to agree on what the values mean, including a partition ID or the state
of an unfinished aggregate. [Plans from other systems](05-plans-from-other-systems.md)
