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

Run commands through `pixi run <cmd>` (don't drop into the interactive `pixi shell`) so each
command runs in the activated environment:

```bash
pixi run make                              # library, C++ tests, and DuckDB extension (uses all cores)
pixi run make clean                        # wipe the library and wrapper build dirs

pixi run make test                         # build + run the C++ unit tests (scripts/run_unit_tests.py); make test_debug for debug

pixi run pre-commit run -a                 # all formatting/lint hooks
```

Running tests directly (non-obvious invocations):
```bash
pixi run build/release/test/cpp/sirius_unittest "[path_utils]"  # by Catch2 tag/test name
```

`scripts/run_unit_tests.py` runs the unit tests in parallel shards (2 per GPU), then the
`[multi_gpu]` and late-materialization tests. Arguments after `--` go to every Catch2 process.
Logs go to `build/release/test/cpp/log/<process>/`.

**Python API**: the default Pixi environment includes DuckDB's Python package. Load the built
Sirius extension from Python as shown in [docs/README.md](docs/README.md#python-api).

**Worktrees**: submodules are not auto-initialized. Run `git submodule update --init --recursive`
when the work requires them, such as for a build or to verify documentation against dependency
source. Pure wording or navigation edits usually do not need them.

**Agent-assisted SQL fuzzing**: use the repository's
[sirius-fuzz skill](.agents/skills/sirius-fuzz/SKILL.md) for requests to run the fuzzer or check
fuzzing readiness. It selects from the developer's available environments and checks GPU
interception before a bounded run. CLI details live in [test/fuzz/README.md](test/fuzz/README.md).

## Architecture

**Super Sirius** is the live engine: namespace `sirius`, source under `src/op/` (operators),
`src/planner/` (plan builders + `sirius_physical_plan_generator.cpp`), `src/pipeline/`,
`src/cuda/` (GPU kernels). Before modifying Super Sirius code, use the
[documentation index](docs/super-sirius/README.md) to read the sections relevant to the task;
check the current implementation and tests for the behavior being changed.

I/O implementation lives in cuCascade; `src/io/` contains Sirius glue. See
[Scan](docs/super-sirius/scan.md) and the headers under `cucascade/include/cucascade/io/`.
Shared `src/exec/` primitives mostly re-export `cucascade::exec`; Sirius retains its own thread
pool and scoped dispatcher.

All new work targets Super Sirius. Memory spilling / CPU fallback is handled by the downgrade executor
(`src/downgrade/`, `src/creator/`); see `docs/super-sirius/memory-management.md`.

For dependency APIs, the checked-out headers and implementation are authoritative. The
[module-context skill](.claude/skills/module-context/SKILL.md) can help navigate cached cuDF,
RMM, DuckDB, and cuCascade documentation; verify relevant signatures against the checkout.
