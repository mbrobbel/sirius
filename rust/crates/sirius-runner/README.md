# sirius-runner

`sirius-runner` runs reproducible Sirius and DuckDB benchmarks for developers,
CI, and benchmarking systems. It owns the full path from dataset preparation
through validation and an evidence-rich result bundle.

The current interface is deliberately small:

```text
sirius-runner list
sirius-runner show <benchmark>
sirius-runner run <benchmark> [options]
sirius-runner doctor [options]
```

Run it from the Sirius checkout with `pixi run runner`.

## Quick start

Inspect the built-in benchmarks and the exact work a run would perform:

```bash
pixi run runner list
pixi run runner show tpch-sf1
pixi run runner run tpch-sf1 --dry-run
```

Run a small DuckDB-only subset without requiring a Sirius build or GPU:

```bash
pixi run runner doctor --engine duckdb
pixi run runner run tpch-sf1 \
  --engine duckdb \
  --queries q1,q6 \
  --iterations 3
```

Run both engines with the release build:

```bash
pixi run runner doctor --engine both
pixi run runner run tpch-sf1 --engine both --preset release
```

`doctor` defaults to the Sirius/GPU workload. Use `--engine duckdb` when only
DuckDB prerequisites should be required.

Progress and logs go to stderr. The final human or JSON result goes to stdout,
so `--json` is safe for automation:

```bash
pixi run runner run tpch-sf1 --engine duckdb --json >summary.json
```

## Execution contract

Every engine/query trial runs in a fresh Python process with a fresh DuckDB
connection. Loading the Sirius extension, registering dataset views, applying
the pin policy, and warm-ups happen outside measured time. A measurement covers
query execution through complete result materialization.

The DuckDB thread count is set explicitly from the process CPU affinity (or the
available CPU count), `preserve_insertion_order` is enabled explicitly, and the
timezone is UTC. These settings are applied to every connection, returned by
the worker for verification, recorded in `run.json`, and included in expected
result cache keys.

Runs are query-major. When comparing both engines, their order alternates for
successive queries to reduce systematic drift. `timeout_s` is one deadline for
the complete trial—setup, warm-ups, measurements, and result encoding. The
worker first interrupts DuckDB and the parent then hard-kills an unresponsive
worker after a short grace period.

One benchmark preparation/execution slot is available per Unix user and host.
Runs and Pixi Pack construction wait for that slot with progress heartbeats.
Remote pack upload and unpack use the same host slot, so environment work cannot
silently contend with measured trials.

Sirius trials disable DuckDB fallback. A successful pin request records that
the pin setup SQL succeeded; it does not claim direct physical-residency
measurement.

## Data and caches

Without `--data`, datasets are generated atomically below:

```text
<data-root>/.sirius/datasets/
```

TPC-H Parquet generation uses the repository's
`test/tpch_performance/generate_tpch_data.sh` workflow. The default Pixi
environment includes its Python/PyArrow dependency. Concurrent processes use a
per-dataset lock, incomplete temporary data is never published, and immutable
receipts contain file hashes and generator provenance. The managed recipe pins
the `tpchgen-rs` revision in `tpchgen-revision.txt`, always runs Cargo's
incremental release build, and records the exact generator executable hash.
Recipe changes select a new cache entry rather than silently reusing old data.
Incomplete staging directories for the same dataset are scavenged under its
lock. If a runner-owned managed entry is corrupt, it is quarantined and
regenerated atomically.

Routine cache hits compare paths, sizes, and modification times. This avoids
rehashing large datasets on every benchmark. Use `--verify-data` to rehash all
files against a managed receipt, or to give an external dataset a content-based
identity:

```bash
pixi run runner run tpch-sf1 --verify-data
pixi run runner run tpch-sf1 --data /datasets/tpch-sf1 --verify-data
```

A verified external identity uses the dataset specification plus relative
paths, sizes, and hashes. Moving or touching byte-identical data therefore
reuses expected results, while a separate path/mtime stability identity still
detects changes during a run.

Expected query results are generated with the exact DuckDB Python runtime and
cached below:

```text
<data-root>/.sirius/expected/
```

The key includes the dataset identity, SQL, validation protocol, DuckDB module
hash and version, Python executable hash and version, explicit DuckDB settings,
and embedded worker hash. Entries are immutable and published atomically. A
per-entry lock prevents concurrent runs from doing the same reference work.
Malformed runner-owned entries are treated as misses and repaired only after
that lock is acquired; read-only inspection never mutates them. The dataset
inventory is checked around reference generation and measured trials; a
detected mutation fails the run before stale expected results or measurements
are recorded.

## Builds and configuration

Sirius runs invoke the selected incremental build, normally:

```bash
pixi run make release
```

Use `--preset debug|release|relwithdebinfo`, or `--build-dir` to use existing
artifacts without attributing them to the current checkout. DuckDB-only runs
skip the Sirius build and reject Sirius-only options.

Packed SSH builds refresh the repository CMake presets and force a fresh CMake
configure under the selected packed toolchain before the incremental build.
The Pixi Pack key, target platform, environment, CUDA profile, exact artifact
hashes, and source revision are retained in bundle provenance.

Configuration follows Sirius's real resolution order:

1. `--config`
2. the benchmark manifest
3. `SIRIUS_CONFIG_FILE`
4. `<repo>/sirius.yaml`
5. `~/.sirius/sirius.yaml`
6. Sirius built-in defaults

When a file is selected, the runner records its SHA-256 identity and verifies
that it does not change during build or execution. The raw YAML is deliberately
not copied into the bundle because Sirius configs may contain object-store
credentials.

## Validation and results

Expected results use a typed canonical representation rather than lossy string
conversion. Validation supports exact digests, ordered or unordered rows, and
per-query floating-point tolerances. Every measured iteration is validated.
The reported status is explicitly `disabled`, `passed`, or `failed`.

The exact `--output` directory must not exist. Without it, a unique directory is
created under `<repo>/benchmark-runs/`. A bundle contains:

```text
run.json                 plan, provenance, preparation state, results, validation
runtimes.csv             one row per measured iteration
logs/                    worker requests, responses, and engine logs
```

`run.json` is flushed atomically after each meaningful stage and measurement.
Failures retain a marked partial bundle. SIGINT, SIGTERM, and SIGHUP cancel the
complete child process group and allow the bundle to be marked failed; sending
a hard termination may still prevent cleanup.

Exit codes are stable:

- `0`: success, including validation disabled
- `1`: runtime failure or blocked prerequisites
- `2`: command-line usage error
- `3`: completed run with a validation mismatch
- `130`: graceful user interruption

## Remote execution over SSH

Remote execution is a run option, not a service:

```bash
pixi run runner run tpch-sf1 \
  --remote developer@gpu-host \
  --remote-repo /srv/sirius \
  --remote-data-root /datasets \
  --output benchmark-runs/remote-sf1
```

`--remote-repo` must be an absolute, complete Sirius checkout. The client
checks its `pixi.toml` and `pixi.lock` hashes against the local checkout before
uploading or building anything. By default both checkouts must also be clean
and at the same Git commit. The remote state is checked again before and after
plan resolution and immediately before a bundle can be marked complete, so a
checkout change cannot silently produce a mixed-source result.

Use `--allow-remote-source-difference` only when intentionally benchmarking a
different or dirty remote checkout. The override and exact remote revision are
recorded; the remote state must still remain unchanged throughout the run. SSH
host trust must already be configured; all SSH calls are noninteractive and use
connection/liveness timeouts.

Path locality is explicit:

- `--repo-root` and `--output` are local client paths.
- `--remote-repo` and `--remote-data-root` are absolute remote paths.
- `--config`, `--build-dir`, and `--data` refer to the selected execution host;
  for SSH they are resolved against `--remote-repo` unless already absolute.

The runner selects the locked CUDA 13.2 environment when supported and falls
back to the locked CUDA 12.9 environment. DuckDB-only remote runs do not require
an NVIDIA GPU. Sirius checks the default GPU's compute capability rather than
accepting an unrelated secondary GPU. Remote runner upload currently requires
Linux, `flock`, the same CPU architecture, and a compatible glibc.

The runtime environment is packed with Pixi Pack 0.7.10. Its content-addressed
cache key covers `pixi.toml`, `pixi.lock`, environment, platform, CUDA profile,
pack format, and Pixi Pack version. Local packs live under
`$XDG_CACHE_HOME/sirius-runner/v1/pixi-packs` (or `~/.cache`); remote packs and
isolated jobs live under `~/.cache/sirius-runner/v1/`. Pack receipts and
checksums are validated before reuse; corrupt local or remote pack state is
rebuilt under the relevant lock. Content-addressed packs are retained for reuse.

After a successful checksum-verified download and atomic local extraction, the
remote job is removed. Failed jobs are retained privately (directories mode
`0700`, files created under `umask 077`) at
`~/.cache/sirius-runner/v1/jobs/<run-id>` and the CLI reports the target and run
ID needed to inspect them. Failed bundles are downloaded when available.

Use `--dry-run` to perform read-only compatibility, repository, and cache
checks and print the planned actions:

```bash
pixi run runner run tpch-sf1 \
  --remote developer@gpu-host \
  --remote-repo /srv/sirius \
  --dry-run

pixi run runner doctor \
  --remote developer@gpu-host \
  --remote-repo /srv/sirius \
  --engine both
```

For a CPU-only host, use `doctor --engine duckdb`. Remote dry-run and doctor
disable Git optional locks and do not create jobs, packs, datasets, or builds.

The runner does not provision hosts, configure SSH trust, cross CPU
architectures, or integrate with a scheduler.

## Embedded benchmark definitions

The binary embeds:

- `datasets/<name>.toml`: supported dataset generator and format
- `suites/<name>/suite.toml`: SQL inventory and validation policy
- `benches/<name>.toml`: dataset scale, engines, and execution defaults

Only implemented fields are accepted. Unsupported generators, reference
engines, compression overrides, and encoding overrides fail during resolution
instead of being silently ignored.
