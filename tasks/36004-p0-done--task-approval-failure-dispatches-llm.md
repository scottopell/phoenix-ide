# Stop task approval failures from dispatching LLM turns

## Severity

P0. A user can approve a task, receive an immediate successful HTTP response, then watch the UI appear to hang while Phoenix performs an invalid post-failure LLM request. The conversation remains stuck in task approval and retries deterministically fail when the target branch already exists.

## Ground truth from live prod repro

Read-only live signals were captured from local prod logs and local Jaeger after the user clicked Approve for:

- Public URL: `https://pp.bigpartydipper.party/c/propose-task-review-7f5e`
- Conversation ID: `9cfac9d9-2a9b-4d8e-a135-1ca7b361d0ff`
- Slug: `propose-task-review-7f5e`
- Task file: `tasks/07004-p1-ready--about-disk-drilldown-actions.md`
- Target branch: `task-07004-about-disk-drilldown-actions`
- Temp branch: `task-pending-9cfac9d9`

### Prod log timeline

All timestamps are UTC from `~/.phoenix-ide/prod.log`.

1. SSE stream was connected before approval:

```text
2026-07-07T02:54:49.568684Z DEBUG SSE client subscribing conv_id=9cfac9d9-2a9b-4d8e-a135-1ca7b361d0ff route=/api/conversations/:id/stream
```

2. The approval HTTP request started and returned success immediately:

```text
2026-07-07T02:54:52.111265Z DEBUG started processing request route=/api/conversations/:id/approve-task method=POST
2026-07-07T02:54:52.119217Z INFO status=200 latency_ms=7 route=/api/conversations/:id/approve-task
```

3. The state machine entered `LlmRequesting` before approval git work completed:

```text
2026-07-07T02:54:52.119286Z DEBUG State transition conv_id=9cfac9d9-2a9b-4d8e-a135-1ca7b361d0ff from=AwaitingTaskApproval to=LlmRequesting
```

4. The approval effect attempted to rename the early Explore temp branch:

```text
2026-07-07T02:54:52.936316Z INFO REQ-PROJ-028: renaming temp branch temp_branch=task-pending-9cfac9d9 task_branch=task-07004-about-disk-drilldown-actions
```

5. Git failed because the target branch already exists:

```text
2026-07-07T02:54:52.952330Z ERROR Task approval git operations failed error="Failed to rename branch 'task-pending-9cfac9d9' to 'task-07004-about-disk-drilldown-actions': git branch -m task-pending-9cfac9d9 task-07004-about-disk-drilldown-actions failed: fatal: a branch named 'task-07004-about-disk-drilldown-actions' already exists"
```

6. The executor broadcast a reverted approval state to the connected SSE receiver:

```text
2026-07-07T02:54:52.952980Z DEBUG broadcasting conversation state_change conv_id=9cfac9d9-2a9b-4d8e-a135-1ca7b361d0ff sequence_id=32 state=AwaitingTaskApproval receiver_count=1
2026-07-07T02:54:52.952991Z DEBUG broadcasted conversation state_change conv_id=9cfac9d9-2a9b-4d8e-a135-1ca7b361d0ff state=AwaitingTaskApproval receiver_count=1
```

7. **Despite the approval failure and state revert, Phoenix dispatched an LLM request:**

```text
2026-07-07T02:54:52.953060Z INFO Making LLM request conv_id=9cfac9d9-2a9b-4d8e-a135-1ca7b361d0ff request_id=30d881af-f48e-4e7a-bf55-8eae19dd3893 span=conversation.turn model=gpt-5.5
```

8. The LLM response arrived, but the state was already `AwaitingTaskApproval`, so the response was invalid and rejected:

```text
2026-07-07T02:54:54.468445Z INFO LLM response token usage input=11507 output=9 cache_write=0 cache_read=5632
2026-07-07T02:54:54.468545Z WARN Rejected invalid outcome — state unchanged reason="Invalid transition: no arm for state=AwaitingTaskApproval event=LlmResponse" state=AwaitingTaskApproval
2026-07-07T02:54:54.468555Z WARN Outcome rejected by state machine error="Invalid transition: no arm for state=AwaitingTaskApproval event=LlmResponse"
```

## Jaeger ground truth

Jaeger was reachable locally at:

```text
http://127.0.0.1:16686
```

The `phoenix-ide` service was present.

Captured traces around the click window:

### Approve HTTP trace

- Trace ID: `41d4d991733ac1b674d9fdba17559eea`
- Span: `http`
- Route: `/api/conversations/:id/approve-task`
- Method: `POST`
- Status: `200`
- Duration: `7992us`
- Start: `2026-07-07T02:54:52.111247Z`

This trace only contains the short HTTP acknowledgement. It does not include the later approval git failure.

### Post-failure LLM trace

- Trace ID: `2e8eac9c79a3c42f4153c6ad081c91e4`
- Span: `gpt-5.5`
- Start: `2026-07-07T02:54:52.961366Z`
- Duration: `1507063us`

This trace begins shortly after the approval git failure and confirms that Phoenix made an LLM request after reverting to `AwaitingTaskApproval`.

### Observability gap

The approval HTTP span and the asynchronous executor/LLM work are separate traces. Jaeger confirms the HTTP 200 and the later LLM request, but the branch-rename failure is only visible in logs, not as a connected child span of the approve request.

## Original cause: task-creation branch collides with task-execution branch

The branch collision was not created by the first failed approval attempt. The target branch already existed before this conversation was approved.

Ground truth from git:

```text
$ git branch --list 'task-07004-about-disk-drilldown-actions' -vv
  task-07004-about-disk-drilldown-actions d926afd4 [origin/task-07004-about-disk-drilldown-actions] tasks: add about disk drilldown actions

$ git branch -r --list 'origin/task-07004-about-disk-drilldown-actions' -vv
  origin/task-07004-about-disk-drilldown-actions d926afd4 tasks: add about disk drilldown actions

$ git show --no-patch --format=... task-07004-about-disk-drilldown-actions
commit=d926afd4ff77a08c9c423e520c6cb73f4a672a78
author_date=2026-07-05T14:19:57-04:00
subject=tasks: add about disk drilldown actions

$ git ls-tree -r --name-only task-07004-about-disk-drilldown-actions | rg '07004|about-disk-drilldown'
tasks/07004-p1-ready--about-disk-drilldown-actions.md

$ git reflog show --date=iso task-07004-about-disk-drilldown-actions
d926afd4 ... commit: tasks: add about disk drilldown actions
37f6b535 ... branch: Created from origin/main
```

That means the same deterministic branch name is being used for two distinct lifecycle concepts:

1. **Task-creation branch**: a branch/PR that adds the ready task file to the repository.
2. **Task-execution branch**: the Work-mode branch Phoenix creates when the user approves that task.

Both currently resolve to:

```text
task-07004-about-disk-drilldown-actions
```

So the first approval attempt was already doomed if the task-creation branch still existed locally or remotely. The branch-exists error is therefore a product/workflow naming collision between task proposal/intake and task execution, not merely a retry artifact.

The fix must decide the intended namespace boundary. Options include:

- reserve `task-{id}-{slug}` exclusively for execution branches and ensure task-intake/proposal branches use another prefix;
- make approval choose a distinct execution branch when the task-intake branch already exists;
- or prove that the existing branch is the intended execution branch and safely reuse it.

Do not treat this as only a generic git failure. The root bug is that Phoenix can create/select a ready task whose existing source branch has the exact branch name that approval later requires.

## Root cause

There are two linked bugs.

### 1. Branch collision is not a first-class approval outcome

Taskmd approval deterministically derives:

```text
branch_name = task-{task_id}-{slug}
```

For this task:

```text
task-07004-about-disk-drilldown-actions
```

`open_early_worktree_and_rename_branch` then runs a blind branch rename:

```text
git branch -m task-pending-9cfac9d9 task-07004-about-disk-drilldown-actions
```

If that target branch already exists, git fails. Phoenix logs the failure and tries to keep the conversation retryable, but the failure is not classified as a typed branch-collision outcome and the initiating HTTP request has already returned `200`.

### 2. Approval failure does not abort the remaining approval effects

The state-machine transition for `Approved / ContinueInCurrentConversation` emits an effect sequence shaped like:

```text
new_state = LlmRequesting
Effect::ApproveTask
Effect::PersistState
Effect::NotifyStateChange
Effect::RequestLlm
```

Relevant code anchor:

- `crates/phoenix-state-machine/src/transition.rs` around the `AwaitingTaskApproval + TaskApprovalDecided(Approved ContinueInCurrentConversation)` arm.

`execute_approve_task` catches approval git failure, manually restores in-memory state to `AwaitingTaskApproval`, sends an SSE error/state change, then returns `Ok(())`.

Because the effect returns `Ok(())`, the executor continues processing the remaining effects and reaches `Effect::RequestLlm`. That dispatches an LLM request from a conversation whose state has already been reverted to `AwaitingTaskApproval`. The later `LlmResponse` is therefore rejected as an invalid state-machine transition.

This is the live-observed cause of the bad post-click behavior.

## Required fix

Fix both the deterministic branch collision and the effect-chain continuation bug.

### Backend approval effect semantics

- An approval git failure must stop the remaining approval effect chain.
- In particular, `Effect::RequestLlm` must not run after `Effect::ApproveTask` fails or reverts state.
- Avoid manual state mutation that makes the effect runner's planned sequence stale unless the remaining effects are explicitly cancelled/skipped.
- Preserve the retryable `AwaitingTaskApproval` state and emit a user-facing error.
- Add a regression test that proves `ApproveTask` failure does not dispatch an LLM request and does not later reject an `LlmResponse` from `AwaitingTaskApproval`.

### Branch collision handling

- Detect that the target branch exists before trying to rename, or classify the git failure into a typed branch-collision approval error.
- Surface a clear user-facing message naming the existing branch.
- Do not force-move or overwrite an existing branch.
- If same-conversation idempotent recovery is possible, implement it only with structural proof that the existing branch/worktree belongs to the same approval. Otherwise fail fast and keep approval retryable.
- Add a backend regression test for the branch-exists path.

### UI recovery

- The task approval UI must not remain in an indefinite approving/busy state after an asynchronous approval failure.
- The user should see a clear error and be able to retry, reject, or otherwise recover.
- Add a frontend regression test for the async-failure path after the POST returned `200`.

### Observability

- Ensure approval git failure is visible in traces or otherwise correlated with the approve action.
- At minimum, preserve structured log fields for `conv_id`, `temp_branch`, `target_branch`, and error class.
- Prefer adding a span around the async approval effect so Jaeger can connect or at least identify approval work by `conv_id` and task branch.

## Acceptance criteria

- Reproducing this exact scenario with an existing `task-07004-about-disk-drilldown-actions` branch does not dispatch an LLM request after approval git failure.
- No `Invalid transition: no arm for state=AwaitingTaskApproval event=LlmResponse` warning occurs as a consequence of failed approval.
- The conversation remains `AwaitingTaskApproval` and retryable after failed approval.
- The UI settles and surfaces a clear branch-collision failure instead of appearing to hang forever.
- The existing branch is never overwritten, force-renamed, deleted, or moved unless a same-conversation idempotency proof is implemented and tested.
- Backend tests cover approval-effect failure stopping later effects and branch-collision classification.
- Frontend tests cover async approval failure after HTTP `200`.
- Trace/log output is sufficient to correlate approval failures with `conv_id` and branch names.

## Useful reproduction/query commands

```bash
rg -n "approve-task|Task approval|task-pending|branch named|propose-task-review-7f5e|9cfac9d9|07004|Invalid transition" ~/.phoenix-ide/prod.log

sqlite3 ~/.phoenix-ide/prod.db \
  "SELECT id, slug, state, cm_kind, cm_worktree_path FROM conversations WHERE slug='propose-task-review-7f5e';"

curl -fsS http://127.0.0.1:16686/api/services
```

Relevant code anchors:

- `crates/phoenix-ide/src/api/lifecycle_handlers.rs::approve_task`
- `crates/phoenix-ide/src/runtime/executor.rs::execute_approve_task`
- `crates/phoenix-ide/src/runtime/executor.rs::execute_approve_task_blocking`
- `crates/phoenix-ide/src/runtime/executor.rs::open_early_worktree_and_rename_branch`
- `crates/phoenix-state-machine/src/transition.rs` approval transition arm
- `ui/src/pages/ConversationPage.tsx::handleApproveTask`
- `ui/src/components/TaskApprovalReader.tsx`
