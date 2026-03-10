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
AND create a branch named `task-{NNNN}-{slug}` from main HEAD
AND checkout that branch in the project checkout
AND transition the conversation to Work mode
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

### REQ-PROJ-009: Complete a Task and Propose Merging to Main

**NOTE:** The merge trigger mechanism will be defined in M4. `update_task` has been
removed; the trigger may be a user-initiated action or a dedicated tool.

WHEN the agent signals ready-for-review (mechanism TBD in M4)
THE SYSTEM SHALL generate a summary of changes between the worktree branch and main
AND present the diff and task file to the user for final review
AND pause agent execution

WHEN user approves the merge
THE SYSTEM SHALL merge the worktree branch into main
AND update the task file status to `done` on main
AND delete the worktree directory
AND delete the branch
AND transition the conversation to Explore mode pinned to the new main HEAD

WHEN user requests further changes before merge
THE SYSTEM SHALL return the feedback to the agent as a message
AND resume agent execution in Work mode

**Rationale:** Human review before merging is the final safety gate. The merge approval
is the moment code moves from isolated experiment to part of the project. Returning to
Explore mode after a successful merge preserves the conversation for follow-up
questions and naturally closes the task.

---

### REQ-PROJ-010: Abandon a Task Without Merging

WHEN user discards an in-progress task
THE SYSTEM SHALL delete the worktree directory
AND delete the branch
AND update the task file status to `abandoned` on main
AND transition the conversation to Explore mode

WHEN a task is abandoned
THE SYSTEM SHALL preserve the task file on main as a historical record
AND NOT merge any code changes to main

**Rationale:** Users must be able to stop work with no lasting consequence to the
codebase. The abandoned task file is a lightweight record of what was attempted —
useful as context for future work, but carries no code changes forward.

---

### REQ-PROJ-011: Ambient Awareness of Main Branch Advancement

WHEN the main branch of a project receives new commits
THE SYSTEM SHALL display an ambient indicator on any Explore conversation showing
how many commits behind the conversation's pinned snapshot is
AND NOT interrupt the active conversation or force re-pinning

WHEN a Work conversation's branch diverges from an advancing main
THE SYSTEM SHALL notify the agent that main has advanced
AND offer a rebase opportunity before the merge step

WHEN user starts a new conversation on a project
THE SYSTEM SHALL pin the conversation to the current HEAD of main at creation time

**Rationale:** Explore conversations are pinned snapshots — they remain coherent and
usable even as main advances, but users should have ambient awareness that their view
may be stale. Work conversations may need to rebase before merging to avoid conflicts.
Neither case warrants an interruption; ambient indicators respect the current focus.

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
