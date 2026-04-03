# Projects: Git-Backed Workspaces with Isolated Task Execution

## User Story

As a developer using PhoenixIDE, I need a structured way to explore codebases safely
and execute changes in isolated branches so that I can think through approaches without
risk and commit to changes with clear human oversight.

## Transparency Contract

The user must be able to confidently answer these questions:

**At a glance:**
1. Which projects do I have active?
2. For each project: how many conversations are open? Any active tasks?
3. Is a conversation read-only (Explore) or writing (Work)?

**For any conversation:**
4. What project does this belong to?
5. What mode is it in and what tools are available?
6. If Work mode: what task is it working on? Where is the worktree?

**For project health:**
7. Are there any orphaned worktrees?
8. What branches/worktrees are active across all conversations?

## Requirements

### REQ-PROJ-001: Open a Git Repository as a Project

WHEN user creates a new conversation by providing a directory path
THE SYSTEM SHALL detect whether the directory is inside a git repository
AND if it is, treat the repository root as the project's canonical path
AND associate the conversation with that project
AND initialize the conversation in Explore mode

WHEN the directory is NOT inside a git repository
THE SYSTEM SHALL create the conversation in Standalone mode (REQ-PROJ-016)
AND NOT associate it with any project

**Rationale:** Users think in terms of projects (codebases, repositories), not raw
directories. Git is the structural foundation of the isolation model — without it the
system cannot create worktrees or maintain task history in a versioned, shareable form.
However, Phoenix must remain useful for non-git directories (ad-hoc scripts, /tmp,
miscellaneous files). Standalone mode provides full tool access without git-backed
safety features, letting users choose their level of structure.

---

### REQ-PROJ-002: Start Every Project Conversation in Explore Mode

WHEN a conversation is created for a project (directory is inside a git repository)
THE SYSTEM SHALL initialize the conversation in Explore mode
AND record the HEAD commit of the main branch as the conversation's pinned snapshot
AND configure all tools in read-only mode

WHILE a conversation is in Explore mode
THE SYSTEM SHALL prevent file writes to the project via any tool
AND SHALL allow unrestricted file reading, directory listing, and read-only command execution

WHEN a new conversation is started
THE SYSTEM SHALL pin it to the current HEAD of main at that moment
AND NOT automatically repin if main advances during the conversation

**Rationale:** Exploration is the natural, zero-friction starting point. Users can
freely ask questions, read code, and plan without any risk of accidentally modifying
anything. Multiple Explore conversations on the same project can coexist safely because
none of them write anything.

---

### REQ-PROJ-003: Propose a Task to Initiate Work Mode

WHEN agent calls the `propose_plan` tool with title, priority, and a written plan
THE SYSTEM SHALL intercept it at the LlmResponse handler (like submit_result)
AND NOT execute any side effects (no file writes, no git operations)
AND persist the assistant message and a synthetic tool result atomically
AND transition the conversation to AwaitingTaskApproval state
AND pause agent execution until the user responds

THE AwaitingTaskApproval state SHALL carry the plan content (title, priority, plan text)
as serializable data, NOT as a file path or git reference

WHEN `propose_plan` is called while the conversation is not in Explore mode
THE SYSTEM SHALL reject the call with an error explaining that plans must be proposed
from Explore mode

**Rationale:** `propose_plan` is a pure data carrier with no side effects. The plan
exists only in the state machine and prose reader until the user approves. This keeps
the feedback loop cheap (no git operations, no files to clean up on reject) and defers
all git work to the approval moment when the user has committed to the approach.

---

### REQ-PROJ-004: Review and Iterate on Task Plan Before Starting Work

WHEN conversation enters AwaitingTaskApproval state
THE SYSTEM SHALL open the prose reader with the plan content from the state
AND present Approve and Discard actions alongside the standard annotation feedback

WHEN user sends annotation feedback
THE SYSTEM SHALL close the prose reader
AND deliver the annotations to the agent as a user message
AND transition the conversation to Explore mode (Idle)
AND the agent MAY revise the plan and call `propose_plan` again
  (which re-enters AwaitingTaskApproval and reopens the prose reader)

WHEN user approves the task
THE SYSTEM SHALL assign the next sequential task ID for the project
AND write a task file to `tasks/` in taskmd format
AND commit the task file to main using `git add <task-file> && git commit --only <task-file>`
AND record the current checked-out branch as `base_branch` in the conversation mode
AND create a worktree at `.phoenix/worktrees/{conv-id}` with a new branch `task-{NNNN}-{slug}`
  using `git worktree add .phoenix/worktrees/{conv-id} -b task-{NNNN}-{slug}`
AND transition the conversation to Work mode (storing base_branch, worktree_path, branch, task_id)
AND resume agent execution with "Task approved. You are on branch task-{NNNN}-{slug}."

WHEN user discards the task
THE SYSTEM SHALL return the conversation to Explore mode
AND return a rejection result to the agent
AND NOT perform any git operations (no file was written, nothing to clean up)

**Rationale:** All git operations are deferred to the approval moment. The feedback
loop is entirely in-memory: no files written, no commits, no branches until the user
explicitly approves. Discarding a task is free — there is nothing to undo. The prose
reader renders from the plan content carried in the state, not from a file on disk.

---

### REQ-PROJ-005: Worktree Paths Are Unique by Construction

WHEN a worktree is created for a conversation
THE SYSTEM SHALL place it at `.phoenix/worktrees/{conversation-id}/` relative to the
repository root
AND ensure `.phoenix/worktrees/` is listed in the repository's `.gitignore`

WHEN two conversations create worktrees for the same project simultaneously
THE SYSTEM SHALL create separate directories for each
AND the directories SHALL share no file paths

**Rationale:** Deriving the worktree path from the conversation ID makes collisions
structurally impossible without a lock registry. Each conversation gets a fully isolated
physical directory. Multiple Work-mode conversations on the same project can coexist
because their code changes never share a directory.

---

### REQ-PROJ-006: Task Files as Versioned Living Contracts

WHEN the user approves a task (REQ-PROJ-004)
THE SYSTEM SHALL write a task file to `tasks/` with filename
  `{NNNN}-{priority}-{status}--{slug}.md`
AND include frontmatter with: task ID, priority, status, branch name, conversation ID,
  and creation date
AND include a Plan section containing the agent's proposed approach as approved
AND include a Progress section (initially empty, updated by the agent via patch tool)
AND commit the file to the main branch

Task files are only created on approval. During the propose/feedback loop, the plan
exists only in the AwaitingTaskApproval state — no file on disk, no git commit.

WHEN the agent updates a task file during Work mode (via patch tool)
THE SYSTEM SHALL allow edits to the task file on the task branch like any other file
AND the updates SHALL merge to main with the rest of the code changes (M4)

WHEN any conversation in Explore mode reads the `tasks/` directory
THE SYSTEM SHALL show all task files including those for in-progress work conversations

**Rationale:** Task files on main give every conversation — whether Explore or Work —
project-wide situational awareness. An agent in Explore mode can see what tasks are
in-progress, planned, or done without any special API. The git history of task files
is a human-readable audit trail of all work attempted on the project.

---

### REQ-PROJ-007: Work Mode Enables Writes Within the Worktree

WHILE a conversation is in Work mode
THE SYSTEM SHALL configure tools to operate within the conversation's worktree directory
AND enable file write tools within the worktree
AND allow bash commands that read and write files within the worktree

WHEN a Work-mode tool attempts to write outside the worktree directory
THE SYSTEM SHALL block the write
AND return a descriptive error

**Rationale:** Work mode's write access is scoped to the worktree, not the whole
filesystem. This preserves the isolation guarantee: a Work conversation cannot modify
main directly, and cannot modify another conversation's worktree.

---

### REQ-PROJ-008: Work Sub-Agents Inherit the Worktree

WHEN a Work conversation spawns a sub-agent with Work mode requested
THE SYSTEM SHALL configure the sub-agent's working directory as the parent's worktree
AND configure the sub-agent in Work mode with write access to that worktree
AND allow only one Work sub-agent per parent conversation at a time
AND place the parent conversation in AwaitingSubAgentResult state for the duration

WHEN a Work conversation spawns a sub-agent with Explore mode
THE SYSTEM SHALL configure the sub-agent's working directory as the parent's worktree
AND configure the sub-agent in Explore mode (read-only, no writes)
AND allow multiple Explore sub-agents in parallel

WHEN an Explore conversation spawns sub-agents
THE SYSTEM SHALL configure all sub-agents in Explore mode
AND configure their working directory as the main branch checkout

**Rationale:** Work sub-agents do implementation work inside the same isolated context
as the parent — they must operate in the worktree, not on stale main. Explore
sub-agents do read-only analysis of whatever directory they're given; from a Work
conversation they analyze the current worktree state, which is what matters. The
one-Work-sub-agent constraint maintains a single writer per worktree at all times.

---

### REQ-PROJ-009: Complete a Task (Squash Merge)

WHEN the user initiates the Complete action on an idle Work conversation
THE SYSTEM SHALL run pre-checks before proceeding:
- Verify no uncommitted changes exist in the worktree (fail with actionable error if dirty)
- Verify the main checkout has no uncommitted changes (fail with actionable error if dirty)
- Verify no merge conflicts exist between the task branch and base_branch (fail with actionable error if conflicts)

WHEN pre-checks pass
THE SYSTEM SHALL generate a semantic commit message by sending `git diff base_branch...HEAD`
  to the LLM with instructions to produce a concise, concept-focused commit message
AND present the generated commit message in an editable confirmation dialog

WHILE the commit message confirmation dialog is open
THE SYSTEM SHALL register a browser navigation guard to warn the user before leaving the page

WHEN the user confirms the commit message (possibly after editing)
THE SYSTEM SHALL squash merge the task branch into base_branch:
  `git checkout base_branch && git merge --squash task_branch && git commit -m "{message}"`
AND delete the worktree: `git worktree remove {path} --force`
AND delete the task branch: `git branch -D {branch}`
AND transition the conversation to Terminal state

WHEN the task file status is not `done` at Complete time
THE SYSTEM SHALL display a non-blocking nudge suggesting the user ask the agent to
  update the task file before completing
AND SHALL NOT block the Complete action

WHEN pre-checks fail
THE SYSTEM SHALL display a specific, actionable error message
AND SHALL NOT proceed with the merge
AND the conversation SHALL remain in Work mode (user can ask the agent to fix the issue)

**Rationale:** Completion is user-initiated, not agent-initiated. The user has been
reviewing work live during the conversation and does not need a separate diff review
gate. Squash merge produces a clean single commit on the base branch. The conversation
goes to Terminal (not back to Explore) because the task is done -- the user creates a
new Explore conversation if they need to continue working on the project. Pre-checks
prevent silent data loss (dirty tree) and broken merges (conflicts).

---

### REQ-PROJ-010: Abandon a Task (Destructive Discard)

WHEN the user initiates the Abandon action on an idle Work conversation
THE SYSTEM SHALL present a confirmation dialog warning that all work will be
  permanently deleted (worktree and branch)

WHEN the user confirms abandonment
THE SYSTEM SHALL verify the main checkout has no uncommitted changes (fail with actionable error if dirty)
AND delete the worktree: `git worktree remove {path} --force`
AND delete the task branch: `git branch -D {branch}`
AND update the task file status to `wont-do` on base_branch:
  `git checkout base_branch && taskmd rename {task_file} --status wont-do && git add tasks/ && git commit -m "task {NNNN}: mark wont-do"`
AND transition the conversation to Terminal state

WHEN the user cancels the confirmation dialog
THE SYSTEM SHALL take no action
AND the conversation SHALL remain in Work mode

**Rationale:** Abandon is a destructive, irreversible operation -- the worktree and
branch are deleted, discarding all code changes. The task file is updated to `wont-do`
(a valid taskmd status) on the base branch as a historical record of what was attempted.
The conversation goes to Terminal because the task is over. Confirmation dialog is
mandatory to prevent accidental data loss.

---

### REQ-PROJ-011: Passive Commits-Behind Indicator

WHEN a client connects to a Work conversation via SSE
THE SYSTEM SHALL check how many commits base_branch is ahead of the worktree's branch point
AND emit the count as part of the initial state payload

WHILE a Work conversation has connected clients
THE SYSTEM SHALL poll for base_branch advancement approximately every 60 seconds
AND emit updated counts via SSE when the value changes

WHEN the commits-behind count is greater than zero
THE SYSTEM SHALL display an "N behind" badge in the StateBar next to the branch name

WHEN the commits-behind count is zero
THE SYSTEM SHALL NOT display any badge

THE SYSTEM SHALL NOT automatically rebase, notify the agent, or take any action
  based on the commits-behind count

**Rationale:** The commits-behind indicator gives the user ambient awareness that their
base branch has advanced, which may affect the merge at completion time. It is passive
and informational only -- no filesystem watcher, no rebase automation. The agent already
has bash access to run `git rebase` when the user asks. Polling on SSE connect plus a
periodic interval is simple and sufficient; real-time filesystem watching adds complexity
without meaningful benefit for a metric that changes infrequently.

---

### REQ-PROJ-012: Provide propose_plan Tool to Agents

WHEN agent is in Explore mode
THE SYSTEM SHALL provide the `propose_plan` tool
WHICH accepts: title (required string), priority (required: p0-p3),
  and plan (required string describing the proposed approach)

WHEN `propose_plan` is called outside Explore mode
THE SYSTEM SHALL reject the call with "propose_plan is only available in Explore mode"

WHEN `propose_plan` is called by a sub-agent
THE SYSTEM SHALL reject the call
AND explain that task management is the parent conversation's responsibility

`propose_plan` is a pure data carrier — it has no side effects. It is intercepted
at the LlmResponse handler (like submit_result for sub-agents) and never enters the
tool executor. The plan data flows into the AwaitingTaskApproval state.

During Work mode, the agent updates task files directly using the patch tool like
any other file. No dedicated `update_task` tool is needed.

**Rationale:** `propose_plan` is the agent's way of saying "I have a plan, please
review it." The name signals human review is required. Keeping it as a pure data
carrier with no side effects means the feedback loop is free (no git work to undo)
and the implementation follows the established submit_result interception pattern.

---

### REQ-PROJ-013: Platform Capability Detection

WHEN the server starts
THE SYSTEM SHALL probe for available sandboxing capabilities:
- Linux: check for Landlock support (kernel >= 5.13, LSM enabled)
- macOS: check for sandbox-exec availability
- Other: no sandbox available

THE SYSTEM SHALL re-check capabilities on every startup

WHILE sandbox is not available
THE SYSTEM SHALL provide Explore mode with ReadFile, Search, and Think tools only
AND SHALL NOT provide bash or any tool that can execute arbitrary commands

WHILE sandbox is available (Landlock or macOS sandbox)
THE SYSTEM SHALL provide Explore mode with bash (sandboxed read-only) and all standard tools
AND ReadFile and Search tools SHALL NOT be provided (bash subsumes them)

**Rationale:** Capabilities are a property of the running environment, not the
application. On systems with kernel-level sandboxing, bash is safe in Explore mode
and more capable than ReadFile. On systems without sandboxing, the restricted tool
set prevents writes structurally. Re-checking on startup ensures the tool set
matches the current host.

---

### REQ-PROJ-014: Project UI

WHEN displaying the conversation sidebar
THE SYSTEM SHALL show a project switcher (tabs) at the top of the sidebar
AND group conversations under their project

WHEN a project has active Work conversations
THE SYSTEM SHALL indicate the active task count next to the project name

WHEN the user selects a project tab
THE SYSTEM SHALL show only that project's conversations

WHEN displaying a conversation
THE SYSTEM SHALL indicate whether it is in Explore or Work mode

**Rationale:** Users manage multiple projects. A project switcher reduces cognitive
load compared to a flat list mixing conversations from different codebases. Mode
visibility prevents confusion about what a conversation can do.

---

### REQ-PROJ-015: Project Worktree Registry

**DESCOPED:** The dedicated worktree registry table is not needed. `ConvMode::Work` on each conversation serves as the de facto registry -- querying all Work conversations for a project yields the active worktree list. Startup reconciliation (M3) handles orphan detection by checking worktree paths on disk. This requirement is retained for historical context but will not be implemented.

WHEN a worktree is created for a task
THE SYSTEM SHALL register it in the project record with task ID, worktree path,
branch name, conversation ID, and timestamp

WHEN a worktree is deleted (merge or abandon)
THE SYSTEM SHALL remove it from the registry

WHEN the server starts
THE SYSTEM SHALL reconcile the registry against worktrees on disk
AND clean up orphaned registry entries
AND report worktrees that exist on disk but have no registry entry

**Rationale:** The registry enables the UI to show all active worktrees and detect
orphans. Reconciliation on startup handles worktrees deleted externally or
conversations that ended without cleanup.

---

### REQ-PROJ-016: Standalone Conversation Mode

WHEN a conversation is created for a directory that is not inside a git repository
THE SYSTEM SHALL initialize the conversation in Standalone mode
AND provide the full tool suite (bash, patch, and all other tools)
AND NOT associate it with any project

WHILE a conversation is in Standalone mode
THE SYSTEM SHALL NOT provide the `propose_plan` tool
AND SHALL NOT allow transition to Explore or Work modes

WHEN displaying a Standalone conversation
THE SYSTEM SHALL NOT show Explore/Work mode indicators
AND SHALL indicate that it is a standalone conversation (no project association)

WHEN a Standalone conversation is created
THE SYSTEM SHALL inform the user that the directory is not a git repository
AND that project features (tasks, worktrees, branch isolation) are not available
AND that file writes have no git safety net

**Rationale:** Phoenix must be useful beyond git repositories. A user editing a script
in `/tmp` or exploring a downloaded archive should not be forced to `git init` first.
Standalone mode provides the full tool suite at the cost of git-backed safety features:
no worktree isolation, no task tracking, no branch-based undo. This is an explicit
trade-off the user accepts by working in a non-git directory. Making Standalone a
distinct mode (rather than overloading Explore or Work) allows the UI to communicate
the capability difference clearly and prevents accidental mixing of project and
non-project behaviors.

---

### REQ-PROJ-017: Base Branch Tracking in Work Mode

WHEN a conversation transitions from Explore to Work mode (task approval)
THE SYSTEM SHALL record the currently checked-out branch as `base_branch` in the
  `ConvMode::Work` data

THE `ConvMode::Work` struct SHALL contain:
- `worktree_path: PathBuf` — path to the conversation's worktree
- `branch: String` — the task branch name
- `task_id: String` — the task identifier
- `base_branch: String` — the branch that was checked out at approval time

WHEN the Complete action runs (REQ-PROJ-009)
THE SYSTEM SHALL merge into `base_branch` (not hardcoded to main)

WHEN the Abandon action runs (REQ-PROJ-010)
THE SYSTEM SHALL commit the task file status update on `base_branch`

**Rationale:** Not all projects use `main` as their integration branch. A user may be
working on a shared feature branch and want task work merged there. Recording the
base branch at approval time supports this workflow without requiring the user to
specify a merge target at completion time.

---

### REQ-PROJ-018: Direct Mode for Git Repositories

WHEN the user creates a conversation targeting a git repository
THE SYSTEM SHALL offer the option to start in Direct mode instead of Explore mode

WHEN the user selects Direct mode for a git repository
THE SYSTEM SHALL create the conversation with full tool access (bash, patch, all tools)
AND set the working directory to the repository root (not a worktree)
AND NOT associate the conversation with the Explore/Work lifecycle
AND NOT offer propose_plan (Direct mode bypasses the plan/approve workflow)

WHEN a Direct-mode conversation operates on a git repository
THE SYSTEM SHALL NOT create worktrees, branches, or task files
AND SHALL NOT restrict any tools based on git state

THE SYSTEM SHALL visually distinguish Direct mode from Explore mode
AND communicate that Direct mode bypasses safety features (no isolation, no review)

**Rationale:** The Explore -> propose_plan -> approve -> Work workflow adds value for
non-trivial changes but creates friction disproportionate to simple fixes. A one-line
config change or quick experiment does not warrant the full ceremony. Without an escape
hatch, users are forced to either endure the workflow overhead or create conversations
in non-git directories and manually copy results. Direct mode gives the user an explicit
opt-out with clear trade-off communication: full power, no safety net.

---

### REQ-PROJ-019: Conversation List Filtering and Auto-Archive

WHEN the conversation list contains more than 20 conversations
THE SYSTEM SHALL provide filtering by conversation mode (Explore, Work, Direct, Standalone)
AND provide filtering by project

WHEN a conversation has been in Terminal state for more than 7 days
THE SYSTEM SHALL automatically archive it
AND the conversation SHALL still be accessible via the archive view

WHEN the user applies a mode filter
THE SYSTEM SHALL show only conversations matching the selected mode
AND persist the filter selection across page navigation

**Rationale:** Active daily use produces dozens of conversations per week. Without
filtering, the list becomes a flat chronological dump where active Work tasks are
indistinguishable from three-day-old quick questions. Auto-archiving Terminal
conversations prevents indefinite list growth from completed or abandoned tasks.
Mode and project filters let the user focus on what matters: "show me my active
Work conversations for this project."

