# Polish the `/new` Start from task workflow

## Problem

The `/new` page's **Start from a task** flow is currently rough in several user-visible ways:

- It exposes **Base branch for planning** for task-start, even though task-start should use the same default-branch behavior as the normal fresh-worktree flow.
- The task picker is paginated only; a user who knows they want task `07004` has to page through instead of searching by task number or name.
- The Start from task option appears only after the full task list is loaded, so the UI snaps from absent/disabled to enabled instead of showing an explicit loading state.
- The project task list can be stale after a task was just merged/renamed remotely; `/api/tasks` reads the local checkout without first refreshing remote refs / default branch state, so a just-merged task may not be visible.
- The full task list is fetched eagerly alongside branch metadata even when the user never opens the task flow.

## Goal

Make Start from task feel like a first-class, predictable workflow:

1. The card is visible quickly for repos that support repo tasks.
2. It shows disabled/loading state while task availability/list details are being determined.
3. Opening it lazy-loads the full task list.
4. Task-start uses the existing default-branch behavior without a task-specific base branch UI.
5. The picker supports fuzzy search by task ID and slug/title.
6. The backend refreshes enough git state before enumerating tasks so recently merged task-file changes are visible.

## Proposed implementation

### Backend/API

- Add a lightweight project task availability endpoint for `/new`, e.g. `GET /api/tasks/availability?cwd=...` or equivalent.
  - It should validate the directory and determine whether the repo has a discoverable tasks directory / task support cheaply.
  - It should not build conversation slug mappings or return the full task list.
  - Shape should be explicit enough for UI states, e.g. `{ enabled: boolean, loading_reason? }` or `{ available: boolean }`.
- Update the full project task list endpoint (`GET /api/tasks?cwd=...`) to refresh remote/default-branch state before listing when used by `/new`.
  - At minimum, ensure the default branch ref is fresh enough that task files merged to origin are visible before enumeration.
  - Preserve the app constraint that Phoenix must not move a local branch checked out in any worktree.
  - Prefer fetching remote refs / reading `origin/<default>` over mutating a checked-out local branch.
- Consider adding query support to `/api/tasks` if server-side filtering is simpler or needed for large task sets; otherwise keep filtering client-side after lazy load.

### UI/data flow

- Split task state in `useCreateConversation` from branch metadata state:
  - Branch/default branch metadata still loads for default workflow behavior.
  - Task availability loads via the lightweight endpoint.
  - The full task list is not fetched until the user selects/opens Start from task.
- Show the Start from task card based on availability/loading state:
  - Loading: visible but disabled with copy like `Loading tasks...`.
  - Available: enabled, with count once known.
  - Unavailable/no active tasks: do not snap unexpectedly; show a stable disabled/empty state or hide only after availability settles, whichever matches final UX decision.
- Remove the task-start base branch combobox from `ConversationSettings`.
  - `planFromTask` should not expose or preserve a task-specific `baseBranch` in UI.
  - Submission should still use `effectiveWorkflow`/default branch behavior, the same as `planFromBranch`.
  - Validation/error copy should not say `Pick a Git branch` for task-start unless no default branch can be resolved.
- Add a task search box inside the task picker.
  - Match task ID (`07004`, partial IDs), slug words, and rendered display text (`id · slug`).
  - Searching should reset pagination and show matched results first.
  - A direct ID query should find the task without paging.
- Keep the selected-task detail preview behavior, but ensure loading/error states are explicit and do not block selection/search unnecessarily.

### Tests

Add or update tests in `ui/src/pages/NewConversationPage.workflow.test.tsx` (or adjacent component tests) for:

- Start from task card renders in a loading/disabled state before full tasks load.
- Selecting Start from task triggers lazy full-list loading rather than loading tasks during initial branch metadata fetch.
- Task-start does not render `Base branch for planning` / task base-branch controls.
- Sending a selected task uses the default branch path via existing workflow submission.
- Searching `07004` or a slug fragment surfaces the matching task without paging.
- Stale task prevention: backend unit/integration coverage for the refreshed-ref behavior before `GET /api/tasks` enumerates files, with no checked-out branch ref movement.

## Notes

Relevant current code:

- `ui/src/hooks/useCreateConversation.ts` currently fetches `api.listGitBranches(cwd)` and `api.listProjectTasks(cwd)` together in one `Promise.allSettled`.
- `ui/src/components/ConversationSettings.tsx` currently renders the Start from task card only when `hasActiveTasks`, exposes `Base branch for planning`, paginates active tasks, and has no task search input.
- `crates/phoenix-ide/src/api/handlers.rs::list_project_tasks` currently validates `cwd` then returns `task_entries_for_cwd` directly.
- `crates/phoenix-ide/src/api/handlers.rs::task_entries_for_cwd` scans task files and maps them to conversations.

## Acceptance criteria

- A repo with tasks shows a stable Start from task affordance with loading feedback instead of snapping into existence.
- A user can type a known task number such as `07004` and select it directly.
- Task-start no longer exposes base branch selection.
- Task-start still creates a managed planning conversation from the repo default branch.
- Recently merged task-file changes are visible after opening the task picker without requiring the user to manually fetch or refresh repeatedly.
- Full task list work is paid only when the task picker is opened, not during initial `/new` page setup.
