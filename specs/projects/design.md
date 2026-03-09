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
    branch: String,            // phoenix/{task-id}-{slug}
    task_id: String,
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
- `status`: awaiting-approval | in-progress | ready-for-review | done | abandoned
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

Phoenix writes and commits task files via `propose_plan` and `update_task` tool
handlers. Agents never write task files directly via the patch tool.

## State Machine Integration

### REQ-PROJ-003, REQ-PROJ-004 — AwaitingTaskApproval State

A new state is added to the bedrock state machine (see `specs/bedrock/`
REQ-BED-028):

```
AwaitingTaskApproval {
  task_id: String,
  task_path: PathBuf,
  reply: oneshot::Sender<TaskApprovalOutcome>,
}

TaskApprovalOutcome {
  Approved,
  Rejected { reason: Option<String> },
  FeedbackProvided { annotations: String },
}
```

The executor opens the prose reader on `task_path` when entering this state (SSE
event: `task_approval_requested` with task file content). The UI presents Approve,
Reject, and annotation feedback actions.

- **Approved:** Create branch, checkout, transition to Work mode.
- **FeedbackProvided:** Close prose reader, deliver annotations as user message,
  return to Explore/Idle. Agent may revise and call `propose_plan` again (reopens
  prose reader).
- **Rejected:** Mark task abandoned on main, return to Explore/Idle.

On server restart: reconstruct from DB (task_id + task_file_path), read task file
from disk, re-emit SSE event on UI reconnect.

### REQ-PROJ-009 — AwaitingMergeApproval State

```
AwaitingMergeApproval {
  task_id: String,
  diff_summary: String,
  reply: oneshot::Sender<MergeApprovalOutcome>,
}

MergeApprovalOutcome {
  Approved,
  ChangesRequested { feedback: String },
}
```

The executor generates the diff when entering this state and includes it in the SSE
event. The UI presents Approve Merge and Request Changes actions.

## Tool Implementation

### REQ-PROJ-012 — propose_plan Tool

The `propose_plan` tool is only available in Explore mode. When called:

**First call (no task_id):**

1. Validate inputs (title, priority, plan)
2. Assign next sequential task ID (query `tasks/` directory for highest existing NNNN)
3. Derive filename slug from title (lowercase, spaces to hyphens, truncate at 40 chars)
4. Write task file to `{repo_root}/tasks/{NNNN}-{priority}-awaiting-approval--{slug}.md`
5. Commit to main via `git commit --only tasks/{file}` (does not touch staging area)
6. Emit `Effect::TransitionToAwaitingTaskApproval { task_id, task_path }`

**Revision call (with task_id, after feedback):**

1. Update existing task file with revised plan
2. Commit to main via `git commit --only tasks/{file}`
3. Re-enter `AwaitingTaskApproval`

The tool does not return until `TaskApprovalOutcome` is resolved. On `Approved`, the
executor:
1. `git branch task-{NNNN}-{slug}` from main HEAD
2. `git checkout task-{NNNN}-{slug}`
3. Updates conversation `conv_mode` to Work
4. Updates task file status to `in-progress` and commits to main
5. Returns success result to agent

On `Rejected`:
1. Updates task file status to `abandoned` and commits to main
2. Returns rejection result to agent

On `FeedbackProvided`:
1. Closes prose reader
2. Delivers annotations to agent as user message
3. Returns to Explore/Idle (agent may call `propose_plan` again with task_id)

### REQ-PROJ-012 — update_task Tool

The `update_task` tool is only available in Work mode for the parent conversation
(not sub-agents). When called:

1. Read current task file from main (not from worktree)
2. Apply status and/or progress updates
3. Present the proposed update to the user for approval
4. On approval: rename file if status slug changes, commit to main via
   `git commit --only tasks/{file}`
5. If status is `ready-for-review`: emit `Effect::TransitionToAwaitingMergeApproval`

## Executor-Layer Git Operations

All git operations are side effects dispatched by the executor, not SM transitions:

| Effect | Git operation |
|--------|---------------|
| `CreateWorktree` | `git worktree add {path} -b {branch}` |
| `DeleteWorktree` | `git worktree remove {path} --force` + `git branch -D {branch}` |
| `MergeWorktree` | `git merge --no-ff {branch}` on main |
| `CommitTaskFile` | `git commit --only tasks/{file} -m {msg}` |
| `RebaseWorktree` | `git rebase main` in worktree (offered, not forced) |

These effects are typed — the state machine emits the intent; the executor performs
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
| `propose_plan` | Allowed | Disabled |
| `update_task` | Disabled | Allowed (parent only, not sub-agents) |
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
- If worktree directory is missing: transition to Explore mode, mark task `abandoned`,
  log warning

## Main Branch Advancement Detection

### REQ-PROJ-011 — Watching for main branch changes

A background watcher monitors `.git/refs/heads/{main_ref}` for changes using
filesystem events (inotify on Linux, FSEvents on macOS). When main advances:

1. For each Explore conversation on the project: emit `SSE::MainAdvanced { commits_ahead: N }`
2. For each Work conversation on the project: emit `SSE::MainAdvanced { commits_ahead: N }`
   and store the advancement notification for the agent to see on next turn

`.gitignore` management: the system checks for `.phoenix/worktrees/` in `.gitignore`
at project creation and appends it if missing.
