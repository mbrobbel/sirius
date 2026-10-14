# Delim CTE inversion

Sirius restores recognized DuckDB-generated delim CTEs before resolving logical column
bindings. This allows the existing delim join lowering, scan provenance, and dense count
join checks to handle correlated queries without changing `delim_join_as_cte`.

The matcher supports inlined and materialized deduplication, reordered join sides, producer
and consumer filters, and nested rewrites. It checks the whole candidate before mutating it.
Unrecognized shapes retain their CTEs and emit a debug-level decline reason.

The pinned DuckDB revision represents ordinary grouping with an empty `grouping_sets`
vector, and can leave the outer CTE's declared width stale after column pruning. The pass
uses the actual producer schema. An inlined dedup domain can also contain filters and
projections; those computations are retained above a delim scan over their required source
columns. When filters use exactly the grouping keys, the delim scan is already unique
and a projection replaces the redundant aggregate. This early deduplication is restricted to integral, decimal, and boolean inputs:
other types can have grouping equality that hides values a later expression distinguishes.

## Limits

A materialized dedup producer with nonidentity projection wrappers, a dedup scan inside a
retained materialized CTE, and joins buried under other joins are deliberately declined.
Producers replaced by a join during CTE join-filter pushdown are also left alone.
The pass does not reconstruct a completely inlined outer CTE. Physical CTE storage and
physical delim lowering are unchanged.

## Validation environment

Validated against Sirius base `7d8052639810218683b55b33c845845a43529e3b` and DuckDB
`88ecd0a49d7d3d838e002b2fbc0ce91ccf74110e`, using an RTX PRO 6000 Blackwell GPU.
TPC-H data was generated with the pinned DuckDB `tpch` extension at SF10.

Default DuckDB optimized logical plans at SF0.1 for [Q2](delim-cte-plans/q2.txt),
[Q17](delim-cte-plans/q17.txt), [Q20](delim-cte-plans/q20.txt),
[Q21](delim-cte-plans/q21.txt), and [Q22](delim-cte-plans/q22.txt) were saved before
implementation. They differ from the original expected pattern: Q2 has a retained CTE
whose aggregate computes MIN, and Q21 can already be a plain join. In Sirius's optimizer
configuration, Q2 retains a CTE alongside a restored delim join. Consequently this branch
does not claim that all five queries become CTE-free.

Observed Sirius physical operators with default settings at SF10:

| Query | Delim operator | CTE retained |
| --- | --- | --- |
| Q2 | RIGHT_DELIM_JOIN | Yes, MIN producer |
| Q17 | RIGHT_DELIM_JOIN | No |
| Q20 | LEFT_DELIM_JOIN | No |
| Q21 | None (plain joins) | No |
| Q22 | LEFT_DELIM_JOIN | No |

The benchmark runner was used with temporary output-only adjustments (`duckbox`, unlimited
rows, and wide columns), because DuckDB's agent-mode default is Markdown. The legacy
comparison additionally sets `delim_join_as_cte=false` in the benchmark session only.
`benchmark_and_validate.sh --report` and `compare_results.py` verified all five default
GPU results against CPU results. Timing measurements and remaining test limitations are
recorded below.

## Fuzz comparison

The bounded `siriusfuzz` gaps run used seed 42, 200 queries, a three-minute maximum, and
no respawns. The unhooked baseline and changed release extension executed exactly the same
SQL and produced identical per-query verdicts: 106 successes, 86 plan fallbacks, six CPU
errors, one result mismatch, and one GPU error. Neither run reported a runtime fallback.
The existing mismatch is an integer SUM result; the existing GPU error evaluates a
constant CASE/LIKE expression as a column. These are outside the inversion change.

## Review

Read-only reviews completed using Codex Sol, Astra, and Luna, and Claude Opus and Fable;
recorded session models matched all five requests. Follow-up Sol and Fable reviews checked
the materialized producer identity wrappers and the retained-CTE boundary decline. No
correctness blockers remained. Review findings led to additional scope checks, type checks,
and tests; debug-level logging is retained as required.

## SF10 timings

Milliseconds, median of eight warm iterations after one cold iteration, same release build.
The ranges show the warm minimum and maximum; these are local measurements, not a formal
performance equivalence test.

| Query | Default median (range) | `delim_join_as_cte=false` median (range) |
| --- | ---: | ---: |
| Q2 | 36 (32–40) | 37.5 (32–53) |
| Q17 | 54.5 (48–66) | 51 (49–64) |
| Q20 | 71 (62–88) | 71 (63–75) |
| Q21 | 143 (117–194) | 83 (80–91) |
| Q22 | 47.5 (38–73) | 32 (29–38) |

Q2, Q17, and Q20 have overlapping timing ranges. Q21 and Q22 remain slower under the
production default in these measurements, so the original all-five-within-noise acceptance
criterion is not met. Q21 has no generated delim CTE left to restore; the two settings can
also produce different join ordering and filter placement that this pass deliberately
preserves. Further performance work is needed for those remaining differences.

## Test limitations

Both debug and release builds succeed. The debug matcher tests pass with binding
verification enabled (218 assertions in eight cases). Release validation covers GPU/CPU
results for correlated EXISTS, NOT EXISTS, scalar aggregates, UNION consumers, expression
keys, nested subqueries, a filtered domain, and a similarly named user CTE. Mixed plain and
null-safe MARK keys retain the existing plan-time CPU fallback.

Debug integration is not green: the existing physical `verify()` call runs before pipeline
wrapping and parent linking in `create_plan`. Its `get_sources()` walk rejects ordinary
multi-input joins whose sink classification depends on those parent links. The logical
inversion does not change this ordering; that separate fix is not included.

The broad release runner also encounters unavailable Iceberg/Avro extensions for this
DuckDB revision (HTTP 404), single-GPU shard failures in tests requiring two GPUs, and an
unrelated pinned-table chunk-count expectation. These prevent an all-green integration run
in this environment. The matched-query and affected planner regressions are checked
separately from those failures.
