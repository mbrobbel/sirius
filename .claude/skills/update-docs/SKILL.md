---
name: update-docs
description: Update Super Sirius reference documentation to match code changes. Use when asked to refresh or sync those docs with a specified change or range.
---

# Update Super Sirius Documentation

1. Determine the scope. Use the user's specified changes or commit range when provided. Otherwise,
   read `docs/super-sirius/README.md` for a `<!-- last-updated-commit: HASH -->` marker. Verify
   that the marker resolves to an ancestor of the intended target commit. If it is missing, invalid,
   or not an ancestor, inspect the docs' history for a plausible baseline and explain the uncertainty;
   ask for a baseline if the intended coverage cannot be determined. A docs commit's date alone does
   not establish that it covered every preceding code change.
2. Inspect the changes in the selected range. Sirius squash-merges PRs, so do not restrict discovery
   to merge commits. Use `git log --first-parent --oneline BASE..TARGET` and
   `git diff --name-only BASE TARGET`, then inspect relevant commits and source files. PR numbers in
   commit subjects can provide context via `gh pr view`; local commits and changes without PR numbers
   still need inspection. Include uncommitted changes when they are part of the requested scope.
   Use [the classification guide](references/change-classification-guide.md) to find affected docs.
3. Update the affected docs to describe the current design. Read the actual source before editing;
   do not infer behavior from PR titles. Document concepts, interfaces, and behavior at the level of
   each page; avoid changelog language and internal fixes that do not help readers. Include PR numbers
   only in `optimizations.md` entries.
4. If a valid marker was used and the full range through the target has been inspected, advance it to
   the target commit. Do not advance or add a marker for a partial review or an uncertain baseline,
   since that would imply coverage that was not established.
5. Report the range, files updated, changes inspected, and any gaps or uncertainty. If no doc changes
   are needed, report that along with the scope checked.
