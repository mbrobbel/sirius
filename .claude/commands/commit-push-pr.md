---
description: Commit and push changes; optionally open a draft PR following Sirius contribution rules.
disable-model-invocation: true
---

# Commit, push, and open a PR

1. Read `AGENTS.md` and the PR branching strategy in `CONTRIBUTING.md`. For a self-contained
   change, branch from `main` if needed and identify the personal fork by its remote URL; if it is
   missing or ambiguous, stop and ask. Follow the documented exceptions for stacked or critical changes.
2. Run `pixi run pre-commit run -a`, relevant tests, and the applicable pre-commit review.
   Hooks run on all files; preserve unrelated work and keep unrelated hook edits out of the commit.
   Resolve failures and repeat affected checks. If a required check cannot run or pass, stop and
   report it.
3. Review the final diff and commit the intended files with a Conventional Commit title and a
   concise one- or two-line body. Push to the selected remote, never directly to `main` or by
   force-pushing an existing PR branch.
4. Create a draft PR against the appropriate base only with explicit user permission; invoking
   this command for its full workflow counts as permission. Use a Conventional Commit title and
   `.github/pull_request_template.md`. Check the `CONTRIBUTING.md` reviewability checklist before
   marking it ready. Report the branch, checks and results, and PR URL if created.
