# sirius-runner

Benchmark runner CLI for Sirius — for developers, CI, and nightly benchmarks.
It will gradually replace the shell/python benchmark infra under
`test/tpch_performance/`; the CLI ships its own SQL queries and benchmark
definitions.

**Status: skeleton.** The command surface below is defined and stable enough to
build on; commands marked *stub* print `not implemented yet` and exit non-zero.

## Usage

```bash
pixi run runner --help
pixi run runner bench list
pixi run runner bench show tpch-sf1
pixi run runner suite show tpch
pixi run runner results schema
```

Or run the binary directly (`cargo build -p sirius-runner`, or download the
`sirius-runner-{x64,arm64}` artifact from the Runner CI workflow).

## Model

Benchmark definitions are layered so each piece is reusable:

| Layer | Lives in | What it is |
|---|---|---|
| Dataset family | [`datasets/<name>.toml`](datasets) | Generator + supported formats (e.g. tpch via tpchgen). An *instance* adds logical args (scale factor) and storage args (format, compression, encoding). |
| Query suite | [`suites/<name>/suite.toml`](suites) | Queries over a dataset family + validation spec. No instance args, no run params. |
| Benchmark | [`benches/<name>.toml`](benches) | Run configuration: suite + dataset instance args + engine selection + execution params. What CI/nightly reference by name, e.g. `tpch-sf100`. |
| Expected results | [`expected/<suite>/sf<N>/`](expected) | Validation data, keyed by the *logical* instance (independent of format/compression/encoding). |

`datasets/`, `suites/`, and `benches/` are embedded into the binary at compile
time; `--assets <DIR>` loads a directory with the same layout instead.
`expected/` is deliberately **not** embedded — see
[expected/README.md](expected/README.md) for the resolution order (assets dir →
data-root cache → generate via the suite's reference engine).

`bench run <name>` resolves everything the run needs: the dataset instance
under the data root (`<data-root>/<family>/sf<N>/<format>[-<compression>][-<encoding>]/`,
generated when missing, free-disk-aware), expected results, a Sirius build,
and the engine config — then runs, validates, and stores results.

## Commands

| Command | Status | Purpose |
|---|---|---|
| `specs` (alias `doctor`) | stub | System specs: GPU, CPU, RAM, disks + free space |
| `build list` | stub | Discover builds under `build/<preset>/extension/sirius` (honors `SIRIUS_BUILD_DIR`) |
| `build source` | stub | Build from source via `pixi run make <preset>` |
| `build download` | stub | Download recent build artifacts from GitHub |
| `dataset list/show` | **works** | List / inspect dataset families |
| `dataset generate` | stub | Generate an instance (disk-aware) |
| `dataset instances` | stub | List instances under the data root |
| `suite list/show` | **works** | List / inspect query suites |
| `bench list/show` | **works** | List / inspect benchmarks (run configurations) |
| `bench run <name>` | stub | Resolve dataset/validation/build/config, run, validate, store; ad-hoc via `--sql`/`--query` |
| `validate generate` | stub | Expected results for a suite at a scale factor via the reference engine |
| `validate status` | stub | Which expected results are present |
| `validate compare` | stub | Check a stored run against expected results |
| `results schema` | **works** | Print the results-store DDL |
| `results list/show/export/push` | stub | Inspect, export, and publish stored results |
| `compare` | stub | Compare two stored runs |
| `sweep run` | stub | Sweep a benchmark across engine configs / dataset encodings |
| `telemetry serve/view` | stub | Quent telemetry over a run's output |
| `remote add/list/status/pack` | stub | Manage remote machines |

Global flags: `--repo-root` (`SIRIUS_REPO_ROOT`), `--assets`
(`SIRIUS_RUNNER_ASSETS`), `--data-root` (`SIRIUS_RUNNER_DATA_ROOT`), `--remote`
(`SIRIUS_RUNNER_REMOTE`), `--json`, `-q/--quiet`, `--no-input`, `--no-color`,
`-v`.

Manifests are TOML (the crate's native config format); Sirius *engine* configs
stay YAML and are referenced by path from benchmark manifests.

## CLI conventions

The CLI follows [clig.dev](https://clig.dev/). The contracts implementations
must keep:

- **stdout is for results, stderr is for everything else** (progress, logs,
  hints, errors). `--json` makes stdout machine-readable; human tables are for
  TTYs. `-q/--quiet` suppresses the non-essential stderr chatter.
- **Exit codes are meaningful**: 0 success, 1 runtime error, 2 usage error
  (clap). Reserved for CI gating: 3 = validation mismatch, 4 = comparison
  regression beyond threshold.
- **Prompts are optional**: only when stdin is a TTY, always bypassable —
  `--no-input` fails instead of asking. Expensive/destructive actions (large
  dataset generation, overwrites) confirm first; `-n/--dry-run` on
  `bench run`, `dataset generate`, and `sweep run` shows the plan without
  acting.
- **Color** only when stdout is a TTY, and never when `NO_COLOR` is set,
  `TERM=dumb`, or `--no-color` is passed.
- **Secrets never travel via flags or env vars**: `results push` credentials
  come from a token file or the system keychain.
- **Runner config** (named remotes etc.) lives in the XDG base directory
  (`~/.config/sirius-runner/`).
- **Operations are recoverable**: dataset generation lands atomically,
  downloads resume, re-running a failed bench continues rather than redoing
  finished work.

## Validation

Each suite declares a reference engine (`duckdb`) that produces expected
results; per-query `validation` settings choose the comparison strategy:
`rows` (tolerance-aware float comparison, the default) or `digest` (exact,
constant-size — for queries whose result size scales with the dataset, e.g.
tpch q16). Expected results for small/common scale factors are committed under
`expected/` so CI and developers never run the reference engine; larger scale
factors are generated once and cached (and may later move to a dedicated
validation-results repository with the same layout).

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
