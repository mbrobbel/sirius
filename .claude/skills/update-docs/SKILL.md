---
name: update-docs
description: Update Super Sirius reference documentation to match code changes. Use when asked to refresh or sync those docs with a specified change or range.
---

# Update Super Sirius Documentation

Use the user's specified changes or commit range when provided. Otherwise, read
`docs/super-sirius/README.md` for a `<!-- last-updated-commit: HASH -->` marker and verify it
resolves to an ancestor of the target. If it is missing, invalid, or not an ancestor, inspect the
docs' history for a plausible baseline and explain the uncertainty; ask for a baseline if the
intended coverage cannot be determined. A docs commit's date alone does not establish coverage.

Sirius squash-merges PRs, so do not restrict discovery to merge commits. Use
`git log --first-parent --oneline BASE..TARGET` and `git diff --name-only BASE TARGET`, then
inspect relevant commits and source files. Include uncommitted changes when in scope. PR numbers in
commit subjects can provide context via `gh pr view`; inspect local changes without PRs as well.
Find affected pages in `docs/super-sirius/`, and check the current source rather than inferring
behavior from titles. Describe the current design at each page's level of abstraction; avoid
changelog language and internal fixes that do not help readers. Include PR numbers only in
`optimizations.md` entries.

Advance an existing valid marker to the target only when the full range through that target has
been inspected. Do not advance or add a marker for a partial review or an uncertain baseline.
Report the scope inspected, pages updated, and any gaps or uncertainty, including when no changes
are needed.
