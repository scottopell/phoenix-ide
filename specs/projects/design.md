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

2. **Enforcement (planned, not yet implemented):** Explore conversations can have
   their read-only constraint enforced at kernel level via Landlock on Linux or
   sandbox-exec on macOS (see `specs/bash/` REQ-BASH-008). On platforms without
   sandboxing, read-only is an application-level constraint enforced by tool
   configuration.

The state machine knows about `ConvMode` as a field on conversations. It does not
know about git, worktrees, or projects — those are executor-layer concerns triggered
by state machine effects.

## Platform Capability Detection

### REQ-PROJ-013 — Sandbox detection and tool registry selection

At startup, the server probes for kernel-level sandboxing:

```
PlatformCapability {
  None,          // no sandbox available
  Landlock,      // Linux 5.13+ with Landlock LSM enabled
  MacOSSandbox,  // macOS with sandbox-exec available
}
```

Detection is automatic (no configuration):
- Linux: checks `/sys/kernel/security/landlock` exists
- macOS: checks `sandbox-exec -n no-network true` succeeds
- Other: `None`

The result gates which tool registry Explore conversations receive:

| `has_sandbox()` | Explore tool set | Bash available? |
|-----------------|-----------------|-----------------|
| `true` | `explore_with_sandbox()` — full tools including bash | Yes |
| `false` | `explore_no_sandbox()` — restricted to ReadFile, Search, Think, keyword_search, browser tools. No bash, no patch. | No |

**Current implementation state:** Sandbox detection works. Tool registry
selection works. Actual bash sandboxing (Landlock wrappers, sandbox-exec
profiles) is **not implemented**. The `explore_with_sandbox()` path gives
bash but does not apply any kernel restrictions to bash processes. This means
Explore mode with `has_sandbox() = true` has bash but the read-only constraint
is enforced only by the system prompt, not by the kernel.

This is an acceptable interim state: the system prompt tells the agent it is
read-only, and the agent respects this in practice. The sandboxing
implementation (REQ-BASH-008, REQ-BASH-009) will add defense-in-depth when
built, without requiring any changes to the detection or registry selection
code. The plumbing is in place — only the bash execution wrapper is missing.

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

Filename convention: `{ID}-{priority}-{status}--{slug}.md`
- `ID`: 5-digit numeric (`DDNNN`) allocated by `taskmd_core::ids::next_id`
  — a per-directory prefix (derived from hostname + tasks-dir path) plus a
  monotonic counter. Not a simple sequence.
- `priority`: p0 (critical) through p4 (nice-to-have)
- `status`: ready | in-progress | blocked | brainstorming | done | wont-do
  (the status set accepted by `taskmd`)
- `slug`: kebab-case title derived by Phoenix at creation time via
  `taskmd_core::filename::derive_slug`

Frontmatter (synthesized by Phoenix at creation; matches what `taskmd new`
produces so the files round-trip through `taskmd validate/fix`):
```
created: YYYY-MM-DD
priority: p1
status: in-progress
artifact: pending
```

`artifact` is a required field — `pending` is an explicit placeholder for
tasks Phoenix itself creates on plan approval, where the concrete artifact
is "whatever this branch ships". Human/agent-authored tasks created via
`taskmd new` must name a real artifact.

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

1. Allocate next task ID via `taskmd_core::ids::next_id(tasks_dir)` (handles
   per-directory prefix + monotonic counter atomically; do not scan by hand)
2. Derive filename slug from title via `taskmd_core::filename::derive_slug`
3. Record current checked-out branch as `base_branch`
4. Write task file to `{repo_root}/tasks/{ID}-{priority}-in-progress--{slug}.md`
   using `taskmd_core::filename::format_filename`
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
| `CommitTaskStatusOnBase` | `git checkout {base_branch}`, then transition the task's status via `taskmd_core` (renames the file + rewrites frontmatter), then `git add tasks/ && git commit -m {msg}` |

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

### REQ-PROJ-008 — Sub-agent working directory, mode, and resource controls

Sub-agents have a mode that determines their tool set, model, MCP access, and
write capabilities. The parent conversation's mode constrains what sub-agent
modes are available.

#### spawn_agents Tool Schema

The `spawn_agents` tool accepts a `tasks` array. Each task spec carries optional
fields for mode, model, and turn budget:

```
SubAgentTaskSpec {
  task: String,              // required — task description for the sub-agent
  cwd: Option<String>,       // optional — working directory override
  mode: SubAgentMode,        // optional — defaults based on parent mode (see below)
  model: Option<String>,     // optional — LLM model override (e.g., "haiku", "sonnet")
  max_turns: Option<u32>,    // optional — maximum LLM turns before forced completion
}

SubAgentMode {
  Explore,   // read-only tools, cheaper model default
  Work,      // full tool suite, inherits parent model
}
```

Default mode resolution:
- Parent in Explore mode: all sub-agents default to `Explore`. `Work` is rejected.
- Parent in Work mode: sub-agents default to `Explore`. `Work` is available on request.
- Parent in Direct mode: all sub-agents default to `Explore` (no worktree context).

#### Tool Registry Per Mode

Each sub-agent mode gets a distinct tool registry:

| Tool | Explore sub-agent | Work sub-agent |
|------|-------------------|----------------|
| `think` | Yes | Yes |
| `bash` | Yes (read-only enforced) | Yes (write enabled in worktree) |
| `patch` | No | Yes (scoped to worktree) |
| `keyword_search` | Yes | Yes |
| `read_image` | Yes | Yes |
| `browser_*` | Yes | Yes |
| `spawn_agents` | No | No |
| `ask_user_question` | No | No |
| `skill` | No | No |
| `propose_plan` | No | No |
| `submit_result` | Yes | Yes |
| `submit_error` | Yes | Yes |
| MCP tools | Yes (deferred, search-oriented) | Yes (full set, deferred) |

Explore sub-agents get read-only bash and no patch — they investigate and report.
Work sub-agents get the full tool suite scoped to the parent's worktree — they
implement changes.

Neither mode gets `spawn_agents` (no recursive spawning), `ask_user_question`
(sub-agents cannot interact with the end user), `skill` (parent handles skill
invocation), or `propose_plan` (parent handles task proposals).

#### Model Selection

Each mode has a default model. The parent can override per-task.

| Mode | Default model | Rationale |
|------|--------------|-----------|
| Explore | `claude-haiku-4-5` | Read-only research is latency-sensitive and cost-sensitive. Haiku is 5-10x cheaper than Opus for tasks that don't require deep reasoning. |
| Work | Parent's model (inherited) | Implementation work benefits from the same model quality the parent uses. |

The optional `model` field on `SubAgentTaskSpec` overrides the default. Valid
values are model IDs known to the LLM registry (e.g., `"claude-sonnet-4-6"`,
`"claude-haiku-4-5"`). Invalid model IDs produce a tool error at spawn time.

#### One-Writer Constraint

A worktree has at most one writer at any time. Multiple readers are safe.

The executor enforces this at spawn time by tracking active Work sub-agents per
parent conversation. The tracking state is a counter on the parent's runtime
handle (not persisted — sub-agents don't survive restarts).

- Spawning a Work sub-agent when another Work sub-agent is active for the same
  parent: rejected with a tool error explaining the constraint.
- Spawning multiple Explore sub-agents: always allowed, no limit beyond system
  resources.
- A Work sub-agent completing or failing decrements the counter immediately,
  releasing the slot for the next spawn.

Mixed spawns in a single `spawn_agents` call (e.g., 3 Explore + 1 Work) are
valid as long as at most one task has `mode: Work`.

#### MCP Tool Access

MCP tools use the `defer_loading` mechanism (tool search). The parent's MCP
client manager is shared with sub-agents — no per-agent MCP server connections.

Explore sub-agents receive the full set of MCP tool definitions with
`defer_loading: true`. When the model discovers a tool via tool search, the
MCP client manager handles the call. This gives Explore agents access to
search-oriented MCP tools (Atlassian search, Google Workspace search, etc.)
without loading all tool schemas into the prompt.

Work sub-agents receive the same MCP tool set as Explore sub-agents. The MCP
tools themselves are stateless RPC calls — the one-writer constraint applies
to filesystem writes via bash/patch, not to MCP tool invocations.

#### Max Turns Limit

Each sub-agent has a maximum number of LLM request turns. When the limit is
reached, the sub-agent's current turn completes normally, then the runtime
injects a forced completion as if the agent had called `submit_error` with
"Reached maximum turn limit (N)".

| Mode | Default max_turns | Rationale |
|------|-------------------|-----------|
| Explore | 20 | Research tasks that take >20 turns are likely stuck in a loop. |
| Work | 50 | Implementation tasks legitimately require more turns (multi-file edits, test iteration). |

The optional `max_turns` field on `SubAgentTaskSpec` overrides the default.
The existing 5-minute timeout remains as a secondary safety net — whichever
limit fires first terminates the agent.

Turn counting: each transition through `LlmRequesting` increments the counter.
Tool execution turns (where the LLM is not called) do not count. This means
a 20-turn Explore agent can execute up to 20 LLM requests, each of which may
invoke multiple tools.

#### Working Directory Assignment

| Parent mode | Sub-agent mode | Sub-agent cwd |
|-------------|---------------|---------------|
| Explore | Explore | Parent's cwd (main checkout) |
| Work | Explore | Parent's worktree path (reads current work state) |
| Work | Work | Parent's worktree path (writes to worktree) |
| Direct | Explore | Parent's cwd |

The `cwd` field on `SubAgentTaskSpec` overrides this default. The override is
validated: Work sub-agents cannot write outside the parent's worktree even if
`cwd` is overridden to a different directory.

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
