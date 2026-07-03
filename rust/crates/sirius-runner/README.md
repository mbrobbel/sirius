# sirius-runner

Benchmark runner CLI for Sirius — for developers, CI, and nightly benchmarks.
It will gradually replace the shell/python benchmark infra under
`test/tpch_performance/`; the CLI ships its own SQL queries and suite
definitions.

**Status: skeleton.** The command surface below is defined and stable enough to
build on; commands marked *stub* print `not implemented yet` and exit non-zero.

## Usage

```bash
pixi run runner                      # --help
pixi run runner "suite list"
pixi run runner "suite show tpch"
pixi run runner "results schema"
```

Or run the binary directly (`cargo build -p sirius-runner`, or download the
`sirius-runner-{x64,arm64}` artifact from the Runner CI workflow).

## Commands

| Command | Status | Purpose |
|---|---|---|
| `specs` (alias `doctor`) | stub | System specs: GPU, CPU, RAM, disks + free space |
| `build list` | stub | Discover builds under `build/<preset>/extension/sirius` (honors `SIRIUS_BUILD_DIR`) |
| `build source` | stub | Build from source via `pixi run make <preset>` |
| `build download` | stub | Download recent build artifacts from GitHub |
| `data generate` | stub | Generate a dataset (disk-aware: checks free space first) |
| `data list` | stub | List datasets under the data root |
| `suite list` / `suite show` | **works** | List / inspect suites |
| `suite run` | stub | Run a suite (resolves or generates its dataset) |
| `bench run` | stub | Ad-hoc single-query benchmark |
| `validate generate` | stub | Expected results from a reference engine |
| `validate compare` | stub | Check a run against expected results |
| `results schema` | **works** | Print the results-store DDL |
| `results list/show/export/push` | stub | Inspect, export, and publish stored results |
| `compare` | stub | Compare two stored runs |
| `sweep run` | stub | Sweep a suite across engine configs / dataset encodings |
| `telemetry serve/view` | stub | Quent telemetry over a run's output |
| `remote add/list/status/pack` | stub | Manage remote machines |

Global flags: `--repo-root` (`SIRIUS_REPO_ROOT`), `--suites`
(`SIRIUS_RUNNER_SUITES`), `--data-root` (`SIRIUS_RUNNER_DATA_ROOT`), `--remote`
(`SIRIUS_RUNNER_REMOTE`), `--json`, `-v`.

## Suites

A suite is a directory with a TOML manifest plus the files it references:

```
suites/tpch/
├── suite.toml
└── queries/q1.sql … q22.sql
```

Suites under [`suites/`](suites) are embedded into the binary at compile time;
`--suites <DIR>` loads from a directory instead (same layout). Manifests are
TOML (the crate's native config format); Sirius *engine* configs stay YAML and
are referenced by path from the manifest.

The `[dataset]` section is a logical spec, not a path — it keys into the data
root as `<data-root>/<benchmark>/sf<N>/<format>[-<compression>]/`. `suite run`
resolves it there and generates the dataset when missing (free-disk-aware;
`--no-generate` to fail instead, `--data-dir` to bypass resolution), so suites
stay machine-portable. See `suite show tpch` for the schema by example.

## Results store

`results schema` prints the DDL ([schema.sql](schema.sql)): `environments`
(system-spec snapshots), `runs`, `results` (per query/iteration), and
`validations`. The store targets a local DuckDB file; runs sync back to the
local store even when executed with `--remote`, and `results push` publishes
to the remote results database.

## Remote execution

Planned as a global `--remote <name|user@host>` flag: the runner ships a
[pixi-pack](https://pixi.prefix.dev/latest/deployment/pixi_pack/) of the
runtime environment plus the matching runner binary and Sirius build to the
target, re-invokes itself there over ssh, and pulls results back. The `remote`
subcommand group only manages targets.
