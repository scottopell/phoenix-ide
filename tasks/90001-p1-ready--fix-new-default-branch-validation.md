# Fix /new default-branch wake-up validation

## Problem

On `/new`, the worktree-based “Start fresh from default branch” / “Chat in a fresh worktree” option can appear selected while submission fails with “Pick a Git branch to start from.” In the screenshot, the UI clearly promises the default branch path, so the validation error is contradictory.

Code inspection shows a single-source-of-truth violation: the UI can render a default-branch starting point from local fallback logic while the workflow state still carries `baseBranch: null`. Submit validation then correctly rejects the state, but the user has already been shown a different truth.

## Plan

1. Remove the split between “display fallback branch” and “submitted workflow branch.”
   - The selected Git starting point must live in the workflow state before the UI presents it as selected/ready.
   - `ConversationSettings` should not make a null workflow branch look equivalent to a real selected branch.
2. Centralize default-branch resolution in the create-conversation state owner.
   - When git metadata loads, choose the initial branch once (`default_branch`, then current branch if needed) and write it into the workflow state.
   - If metadata cannot resolve a branch, the UI should honestly remain not-ready / require a branch rather than rendering “default branch” as if selected.
3. Keep backend mode mapping simple and mechanical.
   - `deriveSubmission(workflow)` should continue to consume the workflow state directly, not recompute hidden fallbacks.
   - Display, `canSend`, and submit validation should all observe the same workflow branch value.
4. Add/adjust regression tests for the wake-up case:
   - render a git repo with `default_branch: main`
   - exercise the selected fresh-worktree workflow after metadata load
   - submit and assert `createConversation(..., 'managed', 'main')`
   - assert no “Pick a Git branch to start from” / “Pick a Git starting point” error appears.
5. Run the focused UI tests for `/new` workflow behavior, then the relevant project check if time permits.

## Acceptance criteria

- A selected “start from default branch” workflow can be submitted as soon as git metadata has loaded.
- There is exactly one source of truth for the selected Git starting point: workflow state.
- The UI cannot show a valid default-branch selection while the submit path treats the starting point as missing.
- Regression coverage prevents display-only fallback from drifting away from submit validation again.
