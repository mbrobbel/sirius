# SQLLogicTest

The Rust `sirius-sqltest` binary checks self-contained tests against DuckDB or Sirius using
the [original SQLLogicTest format](https://www.sqlite.org/sqllogictest/doc/trunk/about.wiki).
The file format has no custom directives. The `sqllogictest` Rust crate supplies parsing, execution, sorting,
and failure diagnostics; a small compatibility layer selects the original
records, compares individual values, and checks repeated query labels.

From the repository root:

```sh
pixi run --manifest-path tools/sqltest/pixi.toml sqltest test/sqltest/*.slt
```

The command builds the binary on demand. The isolated Pixi environment supplies
Rust and DuckDB 1.5.6 for Linux x86-64 and ARM64. It does not build Sirius or the
C++ tests. Each file gets a fresh temporary database. Files run sequentially;
a failure stops that file, and the remaining files still run. Any failure exits
nonzero. The shell expands globs; the Pixi task runs from the repository root.

## Test format

Use `statement ok` and `statement error` for setup and expected failures. Use
`query` with `I` (integer), `R` (real), and `T` (text) result columns. Column types
are checked against DuckDB's result schema. Cast expressions in SQL when a
particular result type is needed.

Non-numeric values use Arrow's text formatting. For dates, timestamps, nested
values, or other native types, cast to `VARCHAR` to check DuckDB's SQL text
representation instead.

Results follow `----`, with **one value per line**, including multi-column rows.
Integers are decimal, reals have three decimal places, NULL is `NULL`, and an
empty string is `(empty)`. Control and non-ASCII bytes in text become `@`.
Printable whitespace is significant. These are the original lossy conventions;
use SQL predicates when checking distinctions that their text representation
cannot express. Results are compared to the file, not to a second running engine.

Scripts use ASCII text. Supported controls are `nosort`, `rowsort`, `valuesort`, `hash-threshold`, `halt`,
query labels, `onlyif`, and `skipif`. The engine name is `duckdb`, or `sirius` when an
extension is provided. Repeated query
labels must have matching results, including expected results of skipped queries.
A hash result uses `N values hashing to DIGEST` with the MD5 of each rendered
value followed by a newline. Comments start with `#`, including within SQL.
`hash-threshold` is accepted as a completion hint; validation always checks the
explicit expected values or digest without treating actual SQL text as a digest.
Unlike the original command-line verifier, it does not require the expected
representation to agree with the active threshold.

Separate records with truly empty lines. Conditional prefixes apply to
statements and queries; control records such as `halt` are unconditional.

Each record should contain one SQL statement. The two examples generate their
own data and need no external fixtures. This binary validates existing expected
results; it does not yet generate them.

RisingLight additions such as `include`, named connections, `statement count`,
error-message matching, `query error`, retries, substitution, shell commands,
and extra result modes are rejected before SQL executes. Those conveniences
can be introduced explicitly in later changes. Rejection also applies to
skipped sections. `halt` stops execution and parsing of subsequent records.

## Testing the runner

```sh
pixi run --manifest-path tools/sqltest/pixi.toml cargo test \
  --locked --manifest-path rust/Cargo.toml -p sirius-sqltest
```

## Sirius execution

Provide an extension built for the same DuckDB version:

```sh
pixi run --manifest-path tools/sqltest/pixi.toml sqltest \
  --extension build/release/extension/sirius/sirius.duckdb_extension \
  test/sqltest/*.slt
```

Sirius requires a supported GPU and its shared libraries in the environment.
Its usual configuration applies, including `SIRIUS_CONFIG_FILE`. The runner
loads the artifact, enables GPU execution, and disables DuckDB fallback before
running any test records. Initialization failures fail the file independently
of expected SQL errors.
`SIRIUS_DISABLE` must be unset or `0` when using `--extension`, so a disabled
runtime cannot silently run the tests on DuckDB instead.

The examples use standard `onlyif sirius` records to checkpoint their generated
data before queries. Sirius handles their CREATE and INSERT setup on the CPU.
No additional SQLLogicTest directives are introduced. Tests that explicitly
disable GPU execution must restore it before their GPU queries.

An opt-in GPU check runs both examples and checks the initial settings:

```sh
SIRIUS_SQLTEST_EXTENSION=/absolute/path/to/sirius.duckdb_extension \
  pixi run --manifest-path tools/sqltest/pixi.toml cargo test \
  --locked --manifest-path rust/Cargo.toml -p sirius-sqltest \
  --test sirius -- --ignored
```
