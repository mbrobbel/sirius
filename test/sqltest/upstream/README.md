# Corpus sources

TPC-H and TPC-DS queries come from DuckDB 1.5.5's `tpch_queries()` and `tpcds_queries()` generators. DuckDB's [MIT notice](licenses/DuckDB.txt) and the [TPC license](licenses/TPC.txt) are retained here. Copyright Transaction Processing Performance Council (TPC).

THE TPC SOFTWARE IS AVAILABLE WITHOUT CHARGE FROM TPC.

ClickBench queries come from [ClickHouse/ClickBench](https://github.com/ClickHouse/ClickBench/tree/9699b7a36a208a65028394e5f39f40e21bda2d91), `duckdb/queries.sql`, by the ClickBench contributors. Its [CC BY-NC-SA 4.0 license](licenses/ClickBench.txt) applies to the imported queries and their adaptations.

The `.sql` files retain the upstream SQL. Corresponding `.slt` files under `../suites/` add fixture includes, runner metadata, and deterministic ordering for correctness comparisons. These adaptations retain their respective upstream licenses. They are not official performance benchmark submissions.

See [provenance.json](../provenance.json) for the import's runtime checksum and source revision.
