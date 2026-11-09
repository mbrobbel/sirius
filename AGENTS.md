# AGENTS.md

Sirius is a GPU-native SQL engine that runs as a DuckDB extension, routing supported SQL
operations to the GPU (via cuDF/RMM/cuCascade) and falling back to DuckDB's CPU execution
otherwise. Once the extension is loaded it **transparently intercepts** normal SQL and runs it
on the GPU — no special syntax needed.

## Contributions & PRs

Branch and open PRs against `main`. Before opening a PR, follow [CONTRIBUTING.md](CONTRIBUTING.md)'s
branching strategy: self-contained changes normally go to a personal fork. Open as Draft until the
PR meets its reviewability checklist. For dependent changes, read its stacked PR and merging
sections; merge stacks bottom-up, never with "Enqueue stack" or `gh stack merge`.

## Build & test

Run build and test commands through `pixi run`:

```bash
pixi run make                              # library, C++ tests, and DuckDB extension (uses all cores)
pixi run make test                         # build + run the C++ unit tests (scripts/run_unit_tests.py); make test_debug for debug

pixi run pre-commit run -a                 # all formatting/lint hooks
```

Run a focused C++ test with `pixi run build/release/test/cpp/sirius_unittest "[path_utils]"`.

`scripts/run_unit_tests.py` runs the unit tests in parallel shards (2 per GPU), then the
`[multi_gpu]` and late-materialization tests. Arguments after `--` go to every Catch2 process.
Logs go to `build/release/test/cpp/log/<process>/`.

For SQL fuzzing, use the [sirius-fuzz skill](.agents/skills/sirius-fuzz/SKILL.md); it verifies
GPU interception before a run. CLI details: [test/fuzz/README.md](test/fuzz/README.md).

## Architecture

**Super Sirius** is the live engine: namespace `sirius`, source under `src/op/` (operators),
`src/planner/` (plan builders + `sirius_physical_plan_generator.cpp`), `src/pipeline/`,
`src/cuda/` (GPU kernels). The [documentation index](docs/super-sirius/README.md) maps tasks
to architecture and execution contracts.

I/O implementation lives in cuCascade; `src/io/` contains Sirius glue. See
[Scan](docs/super-sirius/scan.md) and the headers under `cucascade/include/cucascade/io/`.
Shared `src/exec/` primitives mostly re-export `cucascade::exec`; Sirius retains its own thread
pool and scoped dispatcher.

All new work targets Super Sirius. Memory spilling / CPU fallback is handled by the downgrade executor
(`src/downgrade/`, `src/creator/`); see `docs/super-sirius/memory-management.md`.

For dependency APIs, the checked-out headers and implementation are authoritative. The
[module-context skill](.claude/skills/module-context/SKILL.md) can help navigate cached cuDF,
RMM, DuckDB, and cuCascade documentation; verify relevant signatures against the checkout.
