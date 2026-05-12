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
  Direct                       // default — full tools, no worktree, no git ceremony
  Explore {                    // Managed phase 1 — read-only
    worktree_path: Option<NonEmptyString>,  // Some for a top-level managed Explore
                               // conversation (REQ-PROJ-028); None for an Explore
                               // sub-agent that shares the parent's cwd
  }
  Work {                       // Managed phase 2 — read-write in the same worktree
    worktree_path: NonEmptyString,
    branch_name:   NonEmptyString,   // task-{NNNN}-{slug} (renamed from the temp branch)
    base_branch:   NonEmptyString,   // branch the worktree was created from
    task_id:       NonEmptyString,   // structural discriminator vs Branch mode
    task_title:    NonEmptyString,   // human-readable title, for UI
  }
  Branch {                     // work directly on an existing branch (REQ-PROJ-024)
    worktree_path: NonEmptyString,
    branch_name:   NonEmptyString,   // the user-chosen existing branch
    base_branch:   NonEmptyString,   // == branch_name for Branch mode
  }
}
```

The presence/absence of `task_id` is the structural Work-vs-Branch discriminator
(REQ-PROJ-017). A top-level managed Explore conversation carries its temp branch
(`task-pending-{conv-id}`) only as the worktree's checked-out branch on disk — it is
not stored in the `Explore` variant; on approval the temp branch is renamed in place
to `task-{NNNN}-{slug}` and the mode becomes `Work`.

`ConvMode` is stored as a JSON field on the conversation record in SQLite alongside
the existing `state` column. It is NOT embedded inside every `ConvState` variant —
mode is conversation-level identity, not per-state ephemeral data. There is no
`Standalone` variant — non-git directories use `Direct` (REQ-PROJ-016/018).

### REQ-PROJ-005 — Worktree Path

Worktree paths are derived deterministically:

```
{repo_root}/.phoenix/worktrees/{conversation_id}/
```

`.phoenix/worktrees/` is added to `.gitignore` at project creation if not already
present. Worktrees are ephemeral; they are never committed or pushed.

### REQ-PROJ-006 — Task File Format

Task files live at `{repo_root}/{tasks_dir}/`. `{tasks_dir}` is the project's tasks
directory, discovered at conversation startup as the first immediate child of the repo
root containing `_TEMPLATE.md`. It falls back to the literal `tasks/` when no marker is
found, so existing repos behave as before (task 13008).

The task file is authored by the agent in Explore mode with the `patch` tool (whose
allowlist in Explore mode is scoped to `{tasks_dir}/`) and referenced by path in the
`propose_task` call. On approval it is committed on the task branch — never on `main`
or the base branch (REQ-PROJ-027). taskmd 1.0: the filename is the sole source of task
metadata; there is no frontmatter.

Filename convention: `{ID}-{priority}-{status}--{slug}.md`
- `ID`: 5-digit (`DDNNN`) value — a per-directory prefix (hostname + tasks-dir path)
  plus a monotonic counter. The agent chooses the filename when it drafts the file;
  Phoenix does not allocate the ID, it parses it back out of the filename at approval
  time (`taskmd_core::filename::parse_filename`).
- `priority`: p0 (critical) through p4 (nice-to-have)
- `status`: ready | in-progress | brainstorming at proposal time (`propose_task`
  rejects any other status); the executor promotes it to `in-progress` on approval if
  it isn't already. The agent moves it to `done` (or `wont-do`) itself via `patch`
  before the work closes — nothing renames it automatically.
- `slug`: kebab-case, part of the filename the agent chose.

Body: free-form markdown. Conventionally an `# H1` title, a `## Plan` section (the
approach the user reviewed), and a `## Progress` section the agent updates as work
proceeds. The whole body is what the prose reader shows the user at approval time.

During the propose/feedback loop the task file already exists on disk under
`{tasks_dir}/` (the agent wrote it); `AwaitingTaskApproval` carries the file path plus
a display copy of the title/priority/body. During Work mode, agents update the task
file directly via `patch` like any other file in the worktree; those commits ride
along on the task branch.

## State Machine Integration

### REQ-PROJ-003, REQ-PROJ-004 — AwaitingTaskApproval State

`AwaitingTaskApproval` is a parent-conversation state in the bedrock state machine
(see `specs/bedrock/` REQ-BED-028):

```
AwaitingTaskApproval {
  task_file: String,    // path under {tasks_dir}/ to the file the agent wrote
  title: String,        // display copy (parsed from the file's H1 / filename)
  priority: String,     // display copy ("p0".."p4", from the filename)
  plan: String,         // display copy of the file body
}
```

The display fields are populated once at interception time so the prose reader and
SSE state payload don't have to re-read the file; on approval the executor re-reads
`task_file` from disk (it is the source of truth). `task_file` carries
`#[serde(default)]` as a rollout shim — rows persisted before the file-based flow
deserialise with an empty `task_file`, which the executor surfaces as a clear
"reject and re-propose" error rather than silently resetting to `Idle`.

`propose_task` is intercepted at the `LlmResponse` handler (same pattern as
`submit_result`). It never enters `ToolExecuting`. It must be the only tool call in
the response. The assistant message and a synthetic tool result are persisted as a
`CheckpointData::ToolRound` before the state transitions. No oneshot channels — all
data is serializable. The conversation entering `AwaitingTaskApproval` is what drives
the UI to open the prose reader on the task file's body (see `specs/prose-feedback/`).

- **Approved:** the executor runs the git sequence below (`Effect::ApproveTask`),
  then resumes the agent in Work mode in the same worktree.
- **FeedbackProvided:** close prose reader, deliver annotations as a user message,
  return to Explore/Idle. The agent may edit the task file (via `patch`) and call
  `propose_task` again, re-entering `AwaitingTaskApproval`.
- **Rejected:** return to Explore/Idle. No git operations — the task file stays on
  disk where the agent left it; nothing to clean up.

On server restart: reconstruct from DB (`task_file` + display fields are serialized in
the `ConvState` column). The UI re-opens the prose reader on reconnect.

### REQ-PROJ-010, REQ-PROJ-026, REQ-PROJ-027 — Terminal Actions (Mark as Merged, Abandon)

There is no `AwaitingMergeApproval` state and no in-Phoenix squash-merge (REQ-PROJ-009
was removed — see `requirements.md`). The two terminal actions are user-initiated HTTP
calls (`POST /api/conversations/:id/mark-merged`, `POST /api/conversations/:id/abandon-task`)
on a Work or Branch conversation that is `Idle` or `ContextExhausted`. Both reject with
409 if the conversation has been continued (`continued_in_conv_id` set — REQ-BED-031);
the live conversation is the continuation, so terminal actions belong there. The handler
performs the git cleanup in a `spawn_blocking` task, then routes through the state
machine via `Effect::ResolveTask`/`TaskResolved`, which moves the conversation to
`Terminal`.

**Mark as merged:**

1. Validate mode (Work or Branch), state (`Idle`/`ContextExhausted`), not continued,
   project-scoped.
2. `git worktree remove {worktree_path} --force` (filesystem-rm + `worktree prune`
   fallback on failure).
3. Work (Managed) mode: `git branch -D {branch_name}` — the task branch is a Phoenix
   artifact. Branch mode: keep the branch — it is the user's PR branch.
4. Emit a system message ("Marked as merged. Worktree removed[, task branch deleted].")
   and transition to `Terminal`.

The UI (`WorkActions`) makes this action PR-aware via `GET /api/conversations/:id/pr-status`
(REQ-PROJ-011/026/027): a `gh`-confirmed merged PR gets the "Clean up merged PR" happy
path; an open/draft/failing/closed-unmerged PR disables the button with an explanatory
note; when `gh` is unavailable the user can opt into an explicit manual fallback. Phoenix
never pushes or merges — `git push` is the agent's job via the bash tool.

**Abandon:**

1. Same validation as mark-merged.
2. Capture a best-effort diff snapshot (committed + uncommitted, capped at 100 KiB) from
   the worktree *before* deleting it, and persist it as a system message so the work
   isn't silently lost.
3. `git worktree remove {worktree_path} --force` (same fallback as above).
4. Work mode: `git branch -D {branch_name}`. Branch mode: keep the branch.
5. No task-file edit — for Work mode the branch (and the task file on it) is deleted;
   for Branch mode there is no task file. Transition to `Terminal`.

## Tool Implementation

### REQ-PROJ-012 — propose_task Tool

`propose_task` is a pure data carrier — its `run()` is an unreachable fallback. It is
intercepted at the `LlmResponse` handler (same pattern as `submit_result`) and never
enters `ToolExecuting`. Only available in Explore mode, rejected from sub-agents.

Input: `{ task_file: string }` — a path (relative to the agent's cwd) to a file under
`{tasks_dir}/` whose filename follows the taskmd 1.0 convention
(`NNNNN-pX-status--slug.md`, status one of `ready`/`in-progress`/`brainstorming`). The
agent either points at an existing task file or drafts one first with `patch` (the
Explore-mode `patch` allowlist is scoped to `{tasks_dir}/`). The body is free-form
markdown shown to the user as the plan.

**Interception flow (in the `LlmResponse` transition arm):**

1. Detect a `propose_task` tool_use in the response.
2. Validate: it must be the only tool call (error otherwise).
3. Read the file, parse its filename, build the display fields (title/priority/body).
4. Build a synthetic `ToolResult::success` ("Task submitted for review").
5. Persist `CheckpointData::ToolRound(assistant_message, [tool_result])`.
6. Transition to `AwaitingTaskApproval { task_file, title, priority, plan }`.

**On Approved — `Effect::ApproveTask` (executor, in `spawn_blocking`, serialized by a
process-global approval mutex):**

1. Resolve `base_branch` (the conversation's `desired_base_branch` if set, else the
   current `HEAD` of the conversation's cwd) and `git fetch origin {base_branch}`
   single-branch, best-effort (REQ-PROJ-022).
2. Parse the on-disk task filename → `task_id`, `priority`, `status`, `slug`. No ID
   allocation here — taskmd 1.0 puts the metadata in the filename. The task branch name
   is `task-{task_id}-{slug}`.
3. Locate `.phoenix/worktrees/{conv-id}/`. **Early-worktree path (REQ-PROJ-028, the
   normal Managed flow):** the conversation's cwd already *is* that worktree on a temp
   branch. Rename the temp branch in place to `task-{task_id}-{slug}`, rename the task
   file to `...-in-progress--{slug}.md` if it isn't already in-progress, `git add` +
   `git commit -m "task {task_id}: {title}"` on the task branch. **Fallback path** (no
   pre-created worktree, e.g. a conversation that referenced an existing task file from
   the main checkout): transplant the file into a freshly created worktree on the new
   branch off `base_branch` and commit it there; only then remove the cwd copy.
4. Ensure `.phoenix/worktrees/` is in the worktree's `.gitignore`.
5. Update `conv_mode` to `Work { worktree_path, branch_name, base_branch, task_id,
   task_title }`.
6. Resume the agent with "Task approved. You are on branch task-{task_id}-{slug}."

The task file is committed on the task branch — never on `main`/the base branch
(REQ-PROJ-027). `Effect::ResolveTask`/`TaskResolved` is the symmetric terminal effect
used by mark-merged and abandon (see above).

**On Rejected / FeedbackProvided:** no git operations. The task file stays on disk; the
conversation returns to Explore/Idle (FeedbackProvided also delivers the annotations and
lets the agent re-propose).

### Task File Updates During Work Mode

During Work mode the agent updates the task file with `patch`, like any other file in
the worktree — no dedicated tool. Those commits live on the task branch and reach `main`
when the user merges the PR (REQ-PROJ-027). The agent renames the file to `...-done--...`
(or `...-wont-do--...`) itself when the work closes; Phoenix does not.

## Executor-Layer Git Operations

The git choreography is not modelled as fine-grained typed effects — the state machine
emits one of two coarse effects (`Effect::ApproveTask`, `Effect::ResolveTask`) and the
corresponding handler runs the `git` sequence directly in a `spawn_blocking` task,
feeding back a single completion (or a `GitOperationFailed`-style error message). A
process-global `TASK_APPROVAL_MUTEX` serialises concurrent approvals so two of them
can't race on the same branch/worktree name; there is no per-project main-checkout
mutex because no terminal action touches the main checkout — mark-merged and abandon
operate only on the conversation's own worktree (and, for Managed mode, delete its task
branch). The concrete `git` commands are listed inline in the flows above.

`git worktree remove --force` is used unconditionally on cleanup (the worktree may have
uncommitted files), with a `std::fs::remove_dir_all` + `git worktree prune` fallback if
the porcelain command fails.

## Tool Registry Configuration by Mode

### REQ-PROJ-007 — Tool capabilities by mode

| Tool | Explore mode | Work mode |
|------|-------------|----------|
| `bash` | Allowed (read-only enforced per REQ-BASH-008) | Allowed (write enabled in worktree) |
| `patch` | Scoped to project's tasks dir (typically `tasks/`, discovered per project — task 13008) so the agent can draft a task file before `propose_task`; writes elsewhere rejected | Enabled (scoped to worktree) |
| `think` | Allowed | Allowed |
| `keyword_search` | Allowed | Allowed |
| `read_image` | Allowed | Allowed |
| `browser_*` | Allowed | Allowed |
| `propose_task` | Allowed (intercepted, not executed) | Disabled |
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
| `propose_task` | No | No |
| `submit_result` | Yes | Yes |
| `submit_error` | Yes | Yes |
| MCP tools | Yes (deferred, search-oriented) | Yes (full set, deferred) |

Explore sub-agents get read-only bash and no patch — they investigate and report.
Work sub-agents get the full tool suite scoped to the parent's worktree — they
implement changes.

Neither mode gets `spawn_agents` (no recursive spawning), `ask_user_question`
(sub-agents cannot interact with the end user), `skill` (parent handles skill
invocation), or `propose_task` (parent handles task proposals).

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

### Conversation columns

`ConvMode` is stored as a JSON `conv_mode` column on `conversations` (added by the
projects migration; `Direct` is the default for new rows). Examples:
`"Direct"`, `{"Explore":{"worktree_path":"/repo/.phoenix/worktrees/<id>"}}`,
`{"Work":{"worktree_path":"...","branch_name":"task-YF042-fix-bug","base_branch":"main","task_id":"YF042","task_title":"Fix bug"}}`,
`{"Branch":{"worktree_path":"...","branch_name":"my-pr","base_branch":"my-pr"}}`.
`desired_base_branch` is a separate nullable column used between Managed-mode worktree
creation and approval. There is **no `tasks` table** — querying conversations whose
`conv_mode` is `Work` (or `Branch`) is the de-facto worktree registry (REQ-PROJ-015);
task metadata lives in the task file's name on the task branch.

### Crash recovery (worktree reconciliation)

On startup `reconcile_worktrees` scans conversations whose `conv_mode` is `Work`,
`Branch`, or `Explore`-with-a-worktree:
- Worktree directory present on disk → leave the conversation as-is.
- Worktree directory missing → the conversation is a genuine orphan; mark it `Terminal`
  and log a warning. (No task-file edit — the task file went away with the worktree.)
- **Exception (REQ-BED-031 / REQ-PROJ-015):** a conversation in `ContextExhausted`, or
  one with `continued_in_conv_id` set, is *not* treated as orphaned even if its worktree
  is gone — the worktree is held intentionally pending explicit user action; its mode is
  not demoted.

## Branch Health Indicator

### REQ-PROJ-011 — PR status, not local commit divergence

The StateBar uses PR status as the branch health signal for Work and Branch
conversations. It shows the PR number plus merge/check state when `gh` can resolve
a pull request for the branch. It does not render local commits-ahead or
commits-behind badges; those counts are easy to inspect with git commands when
needed, but they are not the primary decision point for PR cleanup.

`GET /api/conversations/:id/pr-status` is the data source. It runs `gh pr list --head
{branch} --state all --json ...` (and `gh pr checks` where needed) from the
conversation's worktree, in a `spawn_blocking` task, and normalises the result to
`{ found, display_state: open|draft|merged|closed, check_state: passing|pending|failing|unknown,
number/title/url/... }` or `{ found: false, unavailable_reason: gh_missing|not_authenticated|not_git_repo|command_failed }`.
`gh` failures are logged at `debug` and surfaced to the UI as a compact non-blocking
hint, never as a hard error — the conversation page stays usable without `gh`.

`.gitignore` management: the system checks for `.phoenix/worktrees/` in `.gitignore`
at project creation and appends it if missing.
