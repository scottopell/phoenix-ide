# Refine /new workflows and conversation-mode UX

## Problem

The recent `/new` refactor improved the page, but it still exposes too much internal vocabulary and has fragile state around git/worktree workflows.

Observed issues from exploration:

1. **Redundant text**
   - The selected top-level workflow repeats itself in the detail pane (`Direct` card + `Direct` detail text).
   - The worktree path has both a top-level `Worktree-based` label and nested choices (`Start fresh from default branch`, `Pick a task`, `Work in branch`) that repeat “new worktree” language.
   - Directory status appears both in `SettingsFields` and inside `DirectoryPicker` status icon, adding visual noise.

2. **Conversation modes are implicit and confusing**
   - User-visible labels (`Direct`, `Worktree-based`, `Start fresh…`, `Pick a task`, `Work in branch`) do not clearly map to backend/runtime modes:
     - Direct → `mode=direct` / Direct mode
     - Start fresh from default branch → `mode=managed` / Explore first, then Work after task approval
     - Pick a task → `mode=managed` with an initial prompt to call `propose_task`
     - Work in branch → `mode=branch` / Branch mode, no task file or approval workflow
   - “Worktree-based” is technically accurate but not the user’s actual goal. It hides the important difference between “plan/approve first” and “edit an existing branch now”.
   - “Conversation mode” is not explained where the user makes the choice; the page determines mode, but the user cannot predict downstream behavior.

3. **Likely state bug: stale git detection / stale workflow selection**
   - `DirectoryPicker` validates `cwd` with a 300ms debounce but does not abort or version validation requests. An older validation can resolve after `cwd` changed and call `onGitStatusChange(false)`, which can force `intent` back to `direct` or hide git workflows for a valid git repo.
   - `useCreateConversation` clears `startingPoint` on `cwd` change, but the first branch-loading effect later reselects a default branch only if `intent === 'fromExistingWork'`. If stale git status flips intent/directness at the wrong time, the form can submit an unintended mode or show backend errors like “Managed mode requires a git repository”.
   - `canSend` does not currently require `isGitDir === true` for `fromExistingWork`; `handleSend` relies on UI gating and backend validation instead of structurally preventing invalid sends.

## Plan

### 1. Reframe the UX around user goals, not internal modes

Replace the top-level `Direct` / `Worktree-based` split with clearer choices that explain consequences:

- **Chat in this folder** — works directly in the selected directory/current branch; no isolation.
- **Plan a new task** — creates an isolated worktree from a selected base branch; agent starts read-only/explore and asks for approval before editing.
- **Start from an existing task** — same plan/approve flow, seeded from a task file.
- **Continue an existing branch** — creates an isolated worktree with that branch checked out; agent edits/commits directly on that branch, no task approval.

Keep backend names out of the primary UI, but optionally add compact explanatory copy such as “Plan mode” / “Branch mode” in a tooltip or secondary text if useful.

### 2. Reduce redundant copy and visual weight

- Remove the `Direct` detail panel; when “Chat in this folder” is selected, show only the selected card’s concise description.
- Avoid repeating “new worktree” on every nested option; say it once in a short explanation for isolated workflows.
- Make the branch/base/task choice an inline configuration for the selected goal rather than a second nested mode picker.
- Consider removing either the label status text or the directory input icon so directory validation is visible once, not twice.

### 3. Make mode derivation explicit and testable

Refactor UI state so there is a single selected workflow enum, e.g.:

```ts
type NewConversationWorkflow =
  | { kind: 'direct' }
  | { kind: 'planFromBranch'; baseBranch: string | null }
  | { kind: 'planFromTask'; task: TaskEntry | null; baseBranch: string | null }
  | { kind: 'continueBranch'; branch: string | null };
```

Then derive API params from that enum in one place:

- `direct` → `mode='direct'`, no `base_branch`
- `planFromBranch` → `mode='managed'`, `base_branch=<base>`
- `planFromTask` → `mode='managed'`, `base_branch=<base>`, text built from task prompt
- `continueBranch` → `mode='branch'`, `base_branch=<branch>`

This should replace the current indirect combination of `intent`, `startingPoint`, `baseBranch`, `currentBranch`, and `defaultBranch` where possible.

### 4. Fix git-status race conditions

- In `DirectoryPicker`, protect validation results with a sequence number and/or `AbortController` so only the latest `cwd` validation can update `dirStatus` / `isGitDir`.
- When path is empty/invalid/checking, clear git status to `null` rather than `false` so the UI can distinguish “not checked yet” from “checked and not a git repo”.
- Ensure `fromExistingWork` / isolated workflows cannot be selected or submitted unless the latest validation says `isGitDir === true` for the current `cwd`.
- Disable send while a git workflow is selected and branch/task metadata is still loading.

### 5. Add regression coverage

Add focused tests for:

- Stale directory validation responses cannot overwrite newer git status.
- Selecting a git repo enables isolated workflows and submitting “plan from default branch” sends `mode='managed'` with the expected `base_branch`.
- “Continue existing branch” sends `mode='branch'` with the selected branch.
- Switching `cwd` resets branch/task selections and cannot submit stale branch data from the previous repo.
- Non-git directories show only direct/chat workflow and never submit `managed`/`branch`.
- Task workflow builds the propose-task prompt and still uses managed mode/base branch.

## Acceptance criteria

- `/new` presents workflows in user-facing terms with no redundant detail panel copy.
- The user can tell before sending whether the conversation will edit directly, plan/await approval, or continue an existing branch.
- Backend mode selection is derived from one explicit workflow state shape.
- Directory/git detection is race-safe; stale validation responses cannot affect the current form.
- The reported “not in git” false-negative path is covered by a test.
