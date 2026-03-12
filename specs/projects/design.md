# Projects — Technical Design

## Architecture Overview

The Projects feature introduces a first-class `Project` concept that sits above
conversations. A project maps to a git repository root. Conversations belong to a
project and have a `ConvMode` that determines what they can do.

The isolation model has two layers:

1. **Physical isolation (all platforms):** Work conversations operate in a
   conversation-scoped git worktree at `.phoenix/worktrees/{conv-id}/`. Two Work
   conversations on the same project occupy different directories and cannot touch
   each other's files by construction.

2. **Enforcement (Linux with Landlock):** Explore conversations can have their
   read-only constraint enforced at kernel level via Landlock (see `specs/bash/`
   REQ-BASH-008). On platforms without Landlock, read-only is an application-level
   constraint enforced by tool configuration.

The state machine knows about `ConvMode` as a field on conversations. It does not
know about git, worktrees, or projects — those are executor-layer concerns triggered
by state machine effects.

## Data Models

### REQ-PROJ-001, REQ-PROJ-002 — Project and Conversation Mode

```
Project {
  canonical_path: PathBuf,     // git repository root
  main_ref: String,            // e.g. "main" or "master"
}

ConvMode {
  Explore {                    // read-only, pinned to a commit
    pinned_commit: String,     // SHA of main HEAD at conversation creation
  }
  Work {                       // read-write in an isolated worktree
    worktree_path: PathBuf,    // .phoenix/worktrees/{conv-id}/
    branch: String,            // task-{NNNN}-{slug}
    task_id: String,
    base_branch: String,       // branch checked out at approval time (e.g. "main")
  }
}
```

`ConvMode` is stored as a field on the conversation record in SQLite alongside the
existing `state` column. It is NOT embedded inside every `ConvState` variant — mode
is conversation-level identity, not per-state ephemeral data.

### REQ-PROJ-005 — Worktree Path

Worktree paths are derived deterministically:

```
{repo_root}/.phoenix/worktrees/{conversation_id}/
```

`.phoenix/worktrees/` is added to `.gitignore` at project creation if not already
present. Worktrees are ephemeral; they are never committed or pushed.

### REQ-PROJ-006 — Task File Format

Task files live at `{repo_root}/tasks/` and are committed to main.

Filename convention: `{NNNN}-{priority}-{status}--{slug}.md`
- `NNNN`: 4-digit zero-padded sequential integer, globally unique within the project
- `priority`: p0 (critical) through p3 (low)
- `status`: awaiting-approval | in-progress | ready-for-review | done | wont-do
- `slug`: kebab-case title derived by Phoenix at creation time

Frontmatter:
```
id: "001"
priority: p1
status: in-progress
branch: phoenix/001-refactor-auth
conversation: {conv-id}
started: 2025-01-15
```

Body sections:
- `## Plan` — the agent's proposed approach as reviewed and approved by the user
- `## Progress` — checklist the agent updates as work proceeds (optional)

Task files are written to `tasks/` and committed to main at the moment the user
approves a plan. During the propose/feedback loop, the plan exists only in memory
(AwaitingTaskApproval state). During Work mode, agents update task files directly
via the patch tool like any other file.

## State Machine Integration

### REQ-PROJ-003, REQ-PROJ-004 — AwaitingTaskApproval State

A new state is added to the bedrock state machine (see `specs/bedrock/`
REQ-BED-028):

```
AwaitingTaskApproval {
  title: String,
  priority: Priority,   // p0-p3
  plan: String,          // the full plan text
}
```

`propose_plan` is intercepted at the LlmResponse handler (same pattern as
`submit_result`). It never enters `ToolExecuting`. The assistant message and a
synthetic tool result are persisted as a `CheckpointData::ToolRound` before the
state transitions. No oneshot channels — all data is serializable.

The prose reader opens with the plan content from the state (not from a file on
disk). SSE event: `task_approval_requested` with the plan data.

- **Approved:** Write task file to main, create branch, checkout, transition to
  Work mode. All git operations happen here and only here.
- **FeedbackProvided:** Close prose reader, deliver annotations as user message,
  return to Explore/Idle. Agent may revise and call `propose_plan` again.
- **Rejected:** Return to Explore/Idle. No git operations, nothing to clean up.

On server restart: reconstruct from DB (title, priority, plan are all serialized
in the ConvState column). Re-emit SSE event on UI reconnect.

### REQ-PROJ-009, REQ-PROJ-010 — Task Completion and Abandon

There is no `AwaitingMergeApproval` state. Completion and abandonment are user-initiated
actions on idle Work conversations, handled entirely by the executor as effect sequences.
The conversation transitions to Terminal (an existing state) after cleanup completes.

**Complete flow (REQ-PROJ-009):**

1. User clicks Complete on an idle Work conversation
2. Executor runs pre-checks:
   - `git -C {worktree} status --porcelain` — must be empty (no dirty files)
   - `git -C {worktree} merge-tree $(git merge-base base_branch HEAD) base_branch HEAD` — check for conflicts
3. If pre-checks fail: return actionable error, conversation stays in Work/Idle
4. If task file status is not `done`: emit non-blocking nudge to UI
5. Executor sends `git diff base_branch...HEAD` to LLM for commit message generation
6. UI shows editable commit message in confirmation dialog
7. On confirm: executor runs squash merge sequence:
   - `git checkout base_branch`
   - `git merge --squash {branch}`
   - `git commit -m "{user-confirmed message}"`
   - `git worktree remove {path} --force`
   - `git branch -D {branch}`
8. Conversation transitions to Terminal

**Abandon flow (REQ-PROJ-010):**

1. User clicks Abandon on an idle Work conversation
2. UI shows confirmation dialog (destructive action warning)
3. On confirm: executor runs cleanup:
   - `git worktree remove {path} --force`
   - `git branch -D {branch}`
   - `git checkout base_branch`
   - Update task file status to `wont-do`: rename file, `git add tasks/`, `git commit`
4. Conversation transitions to Terminal

## Tool Implementation

### REQ-PROJ-012 — propose_plan Tool

`propose_plan` is a pure data carrier, intercepted at the LlmResponse handler
(same pattern as `submit_result`). It never enters `ToolExecuting` or the tool
executor. Only available in Explore mode, rejected from sub-agents.

**Interception flow (in the LlmResponse transition arm):**

1. Detect `propose_plan` tool_use in the LLM response
2. Validate: must be the only tool in the response (error otherwise)
3. Extract title, priority, plan from the tool input
4. Build synthetic `ToolResult::success` with "Plan submitted for review"
5. Persist `CheckpointData::ToolRound(assistant_message, [tool_result])`
6. Transition to `AwaitingTaskApproval { title, priority, plan }`

**On Approved (executor handles git):**

1. Assign next sequential task ID (scan `tasks/` for highest existing NNNN)
2. Derive filename slug from title
3. Record current checked-out branch as `base_branch`
4. Write task file to `{repo_root}/tasks/{NNNN}-{priority}-in-progress--{slug}.md`
5. `git add tasks/{file} && git commit --only tasks/{file} -m "task {NNNN}: {title}"`
6. `git worktree add .phoenix/worktrees/{conv-id} -b task-{NNNN}-{slug}`
7. Update `conv_mode` to `Work { worktree_path, branch, task_id, base_branch }`
8. Resume agent with "Task approved. You are on branch task-{NNNN}-{slug}."

**On Rejected:**

1. Transition to Explore/Idle
2. No git operations needed

**On FeedbackProvided:**

1. Close prose reader
2. Deliver annotations as user message
3. Transition to Explore/Idle
4. Agent may call `propose_plan` again (re-enters AwaitingTaskApproval)

### Task File Updates During Work Mode

During Work mode, the agent updates task files directly using the `patch` tool,
just like any other file in the worktree. No dedicated tool is needed — task files
are regular markdown files. Updates to the task file live on the task branch and
merge to main with the rest of the code changes (M4).

## Main Checkout Mutex

The Complete and Abandon flows require git operations on the main checkout
(base_branch). Since other Explore conversations may be using the main checkout,
the executor must acquire a mutex lock before operating on it. Use a per-project
mutex (keyed by project root path) to serialize Complete/Abandon operations. The
lock is held only for the duration of the git sequence (checkout + merge +
commit), not during the LLM commit message generation.

If the main checkout has uncommitted changes when the lock is acquired, the
operation fails with an actionable error (same pattern as the dirty-worktree
pre-check).

## Executor-Layer Git Operations

All git operations are side effects dispatched by the executor, not SM transitions:

| Effect | Git operation |
|--------|---------------|
| `CreateWorktree` | `git worktree add {path} -b {branch}` |
| `DeleteWorktree` | `git worktree remove {path} --force` + `git branch -D {branch}` |
| `SquashMergeWorktree` | `git checkout {base_branch} && git merge --squash {branch} && git commit -m {msg}` |
| `CommitTaskFile` | `git add tasks/{file} && git commit --only tasks/{file} -m {msg}` |
| `CommitTaskStatusOnBase` | `git checkout {base_branch} && taskmd rename {file} --status wont-do && git add tasks/ && git commit -m {msg}` |

These effects are typed -- the state machine emits the intent; the executor performs
the git operation and feeds back `WorktreeCreated`, `WorktreeMerged`,
`WorktreeDeleted`, or `GitOperationFailed`.

## Tool Registry Configuration by Mode

### REQ-PROJ-007 — Tool capabilities by mode

| Tool | Explore mode | Work mode |
|------|-------------|----------|
| `bash` | Allowed (read-only enforced per REQ-BASH-008) | Allowed (write enabled in worktree) |
| `patch` | Disabled (per REQ-PATCH-009) | Enabled (scoped to worktree) |
| `think` | Allowed | Allowed |
| `keyword_search` | Allowed | Allowed |
| `read_image` | Allowed | Allowed |
| `browser_*` | Allowed | Allowed |
| `propose_plan` | Allowed (intercepted, not executed) | Disabled |
| `spawn_agents` | Allowed | Allowed |
| `submit_result` | Sub-agents only | Sub-agents only |

## Work Sub-Agent Mode Inheritance

### REQ-PROJ-008 — Sub-agent working directory and mode

When a Work conversation spawns sub-agents via `spawn_agents`, each sub-agent spec
includes a `mode` field:

```
SubAgentMode {
  Explore,  // read-only, cwd = parent's worktree_path (reads current state)
  Work,     // read-write, cwd = parent's worktree_path (only one allowed at a time)
}
```

The executor validates at spawn time:
- `mode: Work` is only valid if parent is in Work mode
- `mode: Work` is rejected if parent already has a pending Work sub-agent
- `mode: Work` sub-agent inherits `worktree_path` and `branch` from parent context

Explore sub-agents from any conversation type always read their assigned directory;
they cannot write regardless of which directory they operate in.

## Persistence

### New database columns

The `conversations` table gains:

```sql
ALTER TABLE conversations ADD COLUMN conv_mode TEXT NOT NULL DEFAULT 'explore';
-- Stored as JSON: {"Explore":{"pinned_commit":"abc123"}} or
-- {"Work":{"worktree_path":"/...","branch":"phoenix/001-...","task_id":"001"}}
```

The `tasks` table (new):

```sql
CREATE TABLE tasks (
  id TEXT PRIMARY KEY,               -- "001"
  project_path TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  status TEXT NOT NULL,
  file_path TEXT NOT NULL,           -- tasks/001-p1-in-progress-....md
  branch TEXT NOT NULL,
  created_at TEXT NOT NULL,
  FOREIGN KEY (conversation_id) REFERENCES conversations(id)
);
```

### Crash recovery

On startup, the executor scans conversations with `conv_mode = Work`:
- If worktree directory exists: restore normally, resume from `Idle` in Work mode
- If worktree directory is missing: transition to Terminal state, mark task `wont-do`
  on base_branch, log warning

## Commits-Behind Indicator

### REQ-PROJ-011 — Passive polling for base branch advancement

No filesystem watcher. The system polls on two triggers:

1. **SSE connect:** When a client connects to a Work conversation, compute
   `git rev-list HEAD..base_branch --count` and include in the initial state payload.
2. **Periodic poll:** Every ~60 seconds while clients are connected, re-run the count
   and emit `SSE::CommitsBehind { count: N }` if the value changed.

The UI shows an "N behind" badge in the StateBar next to the branch name when count > 0.
No badge when count is 0.

No rebase automation. No agent notification. The agent has bash access to run
`git rebase` when the user asks.

`.gitignore` management: the system checks for `.phoenix/worktrees/` in `.gitignore`
at project creation and appends it if missing.
