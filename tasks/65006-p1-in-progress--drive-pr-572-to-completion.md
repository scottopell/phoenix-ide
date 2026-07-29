# Drive PR 572 to completion

Take ownership of PR 572 on branch `task-19007-first-class-model-effort` and carry it through review completion.

## Scope

- Inspect the PR, current branch diff, CI status, and every unresolved or newly added review thread.
- Reconcile review feedback with the model-effort requirements, existing specifications, repository correctness constraints, and the PR's stated behavior.
- Address actionable comments across persistence, model capability typing, provider serialization, runtime/API behavior, telemetry, generated wire types, and UI controls as applicable.
- Reply to or resolve review threads only after the concern is demonstrably addressed; explain clearly when feedback should not result in a code change.
- Add or strengthen focused regression coverage for each corrected behavior.
- Run focused checks while iterating, then run the appropriate `./dev.py check` validation before completion.
- Commit logical units of work, push updates to `task-19007-first-class-model-effort`, monitor CI/review state, and fix any resulting failures or valid follow-up feedback.
- Update the PR title/body if the final implementation or validation differs materially from its current summary.

## Completion criteria

- All review threads have been investigated and either addressed or answered with evidence.
- Required checks pass locally and on the PR, or any external blocker is documented precisely.
- The branch is pushed with clean, reviewable commits and PR 572 is in a merge-ready state.
