# CI / PR smoke test

Create a deliberately no-op change that can be used to verify the PR and CI pipeline end-to-end.

## Goal

- Exercise the branch/PR workflow without altering product behavior.
- Confirm CI runs successfully on a trivial change.
- Leave the repository functionally unchanged.

## Plan

- Make a minimal, non-functional repository change.
- Keep the diff easy to review and safe to merge or discard.
- Verify the branch can be pushed and a PR can be opened.
- Verify CI starts and reports a result on the PR.

## Constraints

- No user-facing behavior changes.
- No schema, API, or test changes.
- Keep the patch as close to a pure no-op as possible.
