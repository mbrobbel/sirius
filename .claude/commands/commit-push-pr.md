---
description: Commit and push changes; optionally open a draft PR following Sirius contribution rules.
disable-model-invocation: true
---

# Commit, push, and open a PR

1. Read `AGENTS.md` and the PR branching strategy and reviewability sections of
   `CONTRIBUTING.md`. Inspect `git status`, staged and unstaged diffs, and untracked files.
   Identify the intended changes; preserve unrelated work and stage explicit paths only.
2. Select the appropriate branching strategy. For a self-contained change, create a branch
   from `main` if needed and identify the personal fork remote by its URL; do not assume
   `origin` is a fork. If no unique personal fork remote can be identified, stop and ask.
   Follow the documented exceptions for stacked or critical changes.
3. Run the formatter and linters with `pixi run pre-commit run -a` and run relevant tests.
   Fix failures and repeat affected checks before committing. If a required check cannot run or
   remains failing, stop before committing and report the checks and results. Preserve unrelated
   work if hooks modify files. Follow the applicable pre-commit review instructions.
4. Review the final diff and commit the intended files using a Conventional Commit title and
   a concise one- or two-line body. Do not commit unrelated changes.
5. Push the branch to the remote selected under the contribution strategy; never push directly
   to `main` or force push an existing PR branch.
6. Create a draft PR against the appropriate base only if the user has explicitly requested or
   permitted a PR; explicitly invoking this command for its full workflow counts as permission.
   Use a Conventional Commit PR title and fill in `.github/pull_request_template.md`, including
   the motivation, change, and validation. Check
   the reviewability checklist before marking it ready; otherwise leave it as a draft. Report
   the branch, checks and results, and PR URL if created.
