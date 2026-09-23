---
description: Build Sirius with pixi and run tests. Asks the user which tests to run.
---

# Build and Run Tests

Build Sirius using pixi and run the tests the user selects.

## Steps

1. Build Sirius using pixi:
   ```
   pixi run make
   ```
   If build fails, diagnose and fix. If OOM, retry with `CMAKE_BUILD_PARALLEL_LEVEL=8 pixi run make`.

2. After a successful build, present the user with test options:

   ```
   Build succeeded. Which tests would you like to run?

   1. SQL correctness suites
   2. A specific SQL suite or case (e.g. tpch_legacy/q01)
   3. All C++ unit tests
   4. A specific C++ unit test (provide name or [tag])
   5. Auto-detect from changed files
   ```

   Wait for the user to choose before proceeding.

3. Run the selected tests using pixi:
   - **SQL correctness suites**: Follow `test/sqltest/README.md` to prepare the runner and fixtures, then use `pixi run --manifest-path tools/sqltest/pixi.toml sqltest run --run correctness --extension build/release/extension/sirius/sirius.duckdb_extension --output runs/correctness-001` with a new output directory.
   - **Specific SQL test**: Select the appropriate named run, then add `--suite <suite>` or `--select <case-id>` to the SQL runner command.
   - **All C++ unit tests**: `pixi run bash -c "build/release/extension/sirius/test/cpp/sirius_unittest"`
   - **Specific C++ unit test**: `pixi run bash -c "build/release/extension/sirius/test/cpp/sirius_unittest '<name-or-tag>'"`
   - **Auto-detect**: Check `git diff dev --name-only` to identify changed files, then run the most relevant tests. Explain the reasoning to the user before running.

4. If tests fail:
   - Show the failure output clearly.
   - Analyze the root cause.
   - Ask the user if they want you to fix it. If yes, apply the fix and re-run the failing test.

5. Report results: total passed, failed, and skipped.
