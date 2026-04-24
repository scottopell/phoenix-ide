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
THE SYSTEM SHALL create the conversation in Direct mode (REQ-PROJ-018)
AND NOT associate it with any project

**Rationale:** Users think in terms of projects (codebases, repositories), not raw
directories. Git is the structural foundation of the isolation model — without it the
system cannot create worktrees or maintain task history in a versioned, shareable form.
However, Phoenix must remain useful for non-git directories (ad-hoc scripts, /tmp,
miscellaneous files). Direct mode provides full tool access without git-backed
safety features, letting users choose their level of structure.

---

### REQ-PROJ-002: Default Conversation Mode Selection

WHEN a conversation is created for any directory
THE SYSTEM SHALL initialize the conversation in Direct mode by default
AND provide full tool access (bash, patch, all tools)

WHEN a conversation is created for a git repository AND the user selects "Managed" mode
THE SYSTEM SHALL initialize the conversation in Explore mode
AND record the HEAD commit of the main branch as the conversation's pinned snapshot
AND configure all tools in read-only mode

WHILE a conversation is in Explore mode (Managed workflow)
THE SYSTEM SHALL prevent file writes to the project via any tool
AND SHALL allow unrestricted file reading, directory listing, and read-only command execution

WHEN the user selects "Managed" mode for a non-git directory
THE SYSTEM SHALL reject the request (Managed mode requires a git repository)

**Rationale:** Direct mode is the natural, zero-friction starting point for most work.
The Managed (Explore/Work) lifecycle adds value for non-trivial changes that benefit
from plan review and worktree isolation, but should be opt-in rather than mandatory.

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
AND commit the task file on the task branch in the existing worktree (REQ-PROJ-028)
AND transition the conversation from Explore to Work mode within the same worktree
  (storing base_branch, worktree_path, branch, task_id)
AND resume agent execution with "Task approved. You are on branch task-{NNNN}-{slug}."

WHEN user discards the task
THE SYSTEM SHALL return the conversation to Explore mode
AND return a rejection result to the agent
AND NOT perform any git operations (no file was written, nothing to clean up)

**Rationale:** The Explore -> Work transition is a permission upgrade within an
existing worktree (REQ-PROJ-028). The task file is committed on the task branch
(never main -- REQ-PROJ-027) at approval time. Discarding a task is cheap: the
worktree and branch already exist but no task file was written. The prose reader
renders from the plan content carried in the state, not from a file on disk.

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
THE SYSTEM SHALL allocate a task ID via `taskmd_core::ids::next_id` (a
  5-digit `DDNNN` value — per-directory prefix + monotonic counter)
AND write a task file to `tasks/` with filename
  `{ID}-{priority}-{status}--{slug}.md`
AND include frontmatter with `created`, `priority`, `status`, and `artifact`
  fields (matching what `taskmd new` synthesizes, so the file round-trips
  through `taskmd validate`/`fix`)
AND include a Plan section containing the agent's proposed approach as approved
AND include a Progress section (initially empty, updated by the agent via patch tool)
AND commit the file on the task branch (never on main or the base branch)

Task files are only created on approval. During the propose/feedback loop, the plan
exists only in the AwaitingTaskApproval state -- no file on disk, no git commit.
Branch mode conversations (REQ-PROJ-024) do not create task files.

WHEN the agent updates a task file during Work mode (via patch tool)
THE SYSTEM SHALL allow edits to the task file on the task branch like any other file
AND the updates SHALL be pushed with the rest of the code changes (REQ-PROJ-027)

**Rationale:** Task files live on the task branch alongside the code changes, keeping
the branch self-contained. This avoids committing to main (which may be protected)
and eliminates the two-path commit logic. The task file merges to main when the PR
is merged through the user's normal workflow.

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

**DEPRECATED:** Superseded by REQ-PROJ-027 (push branch, user merges via PR).
Squash-merge bypasses code review and branch protection rules. The push-branch
model aligns with how teams actually ship code. Retained for historical context.

---

### REQ-PROJ-010: Abandon a Conversation

WHEN the user initiates the Abandon action on an idle Work conversation
THE SYSTEM SHALL present a confirmation dialog warning that the worktree will be deleted

WHEN the user confirms abandonment of a Managed mode conversation
THE SYSTEM SHALL delete the worktree AND delete the task branch
AND transition the conversation to Terminal state

WHEN the user confirms abandonment of a Branch mode conversation
THE SYSTEM SHALL delete the worktree AND keep the branch
AND transition the conversation to Terminal state

WHEN the user cancels the confirmation dialog
THE SYSTEM SHALL take no action
AND the conversation SHALL remain in Work mode

**Rationale:** Abandon deletes the worktree to free disk space. For Managed mode,
the task branch is also deleted because Phoenix created it -- it's a Phoenix artifact.
For Branch mode, the branch is kept because it belongs to the user's PR, not to
Phoenix. The confirmation dialog prevents accidental worktree deletion.

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

WHEN a conversation is in context-exhausted state (REQ-BED-021)
OR a conversation has `continued_in_conv_id` set (REQ-BED-030)
THE SYSTEM SHALL NOT treat its worktree as orphaned during reconciliation
AND SHALL NOT demote the conversation's mode
  (the worktree is preserved pending explicit user action per REQ-BED-031)

**Rationale:** The registry enables the UI to show all active worktrees and detect
orphans. Reconciliation on startup handles worktrees deleted externally or
conversations that ended without cleanup. Context-exhausted conversations and
their continuations are an explicit exception: their worktrees are held
intentionally and must survive restart unchanged.

---

### REQ-PROJ-016: Standalone Conversation Mode (Superseded)

**SUPERSEDED BY REQ-PROJ-018.** Standalone mode was a distinct mode for
non-git directories providing the full tool suite without git-backed
features. It was folded into `ConvMode::Direct` — which now serves both
git-backed and non-git working directories with identical semantics. See
REQ-PROJ-018 for the canonical historical note and the current behavior.
Retaining this REQ ID for traceability; content below describes the
original pre-supersession design.

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

WHEN a conversation transitions to Work or Branch mode
THE SYSTEM SHALL record the base branch in the conversation's mode data

THE shared worktree fields (present in Explore, Work, and Branch modes) SHALL be:
- `worktree_path: PathBuf` -- path to the conversation's worktree
- `branch_name: String` -- the task branch (Work) or existing branch (Branch)
- `base_branch: String` -- the branch the worktree was created from

THE Work mode SHALL additionally contain:
- `task_id: String` -- always present, the structural discriminator vs Branch mode

THE Branch mode SHALL NOT contain `task_id`

WHEN the "Mark as merged" action runs (REQ-PROJ-026/027)
THE SYSTEM SHALL delete the worktree (and delete the branch for Managed mode)

WHEN the Abandon action runs (REQ-PROJ-010)
THE SYSTEM SHALL delete the worktree (and delete the branch for Managed mode)

**Rationale:** Not all projects use `main` as their integration branch. A user may be
working on a shared feature branch. Recording the base branch at worktree creation
time supports this workflow. Branch mode uses the branch itself as both the branch
name and the base branch.

---

### REQ-PROJ-018: Direct Mode (Implemented)

Direct mode is the default for all new conversations, git-backed and non-git alike.

**Historical note — Standalone → Direct migration.** An earlier design
split non-git directory conversations into a separate `Standalone` mode
(see superseded REQ-PROJ-016 and the rationale in REQ-BED-027). In
practice the two modes had identical runtime semantics (full tool suite,
no `propose_plan`, no worktree, no task file, no branch, no project
association beyond `cwd`), so the split produced no behavioral difference
— only type-level ceremony. `Standalone` was folded into `Direct` via DB
migration 001 (`UPDATE conversations SET conv_mode = REPLACE(conv_mode,
'"Standalone"', '"Direct"')`), and the `ConvMode::Standalone` enum
variant was removed from the code. All references to Standalone in the
spec corpus have been updated to Direct; this REQ-PROJ-018 is the
canonical landing for the history. If you encounter Standalone in old
code comments, task files, or git history, treat it as an alias for
Direct.

WHEN a conversation is created in Direct mode
THE SYSTEM SHALL provide full tool access (bash, patch, all tools)
AND set the working directory to the target directory (not a worktree)
AND NOT include `propose_task` in the tool registry
AND NOT create worktrees, branches, or task files

THE SYSTEM SHALL visually distinguish Direct mode from Explore mode in the UI
AND present the mode choice (Direct vs Managed) on the new conversation page
with descriptions explaining the trade-offs

WHEN a Direct-mode conversation targets a git repository
THE SYSTEM SHALL associate it with the project (for MCP config, filtering, etc.)
AND SHALL NOT restrict any tools based on git state

**Rationale:** The Explore/Work ceremony adds value for non-trivial changes that
benefit from plan review and worktree isolation, but creates friction disproportionate
to simple fixes. Direct mode is the zero-friction default; the Managed workflow is
opt-in for users who want structured project management.

---

### REQ-PROJ-019: Conversation List Filtering and Auto-Archive

WHEN the conversation list contains more than 20 conversations
THE SYSTEM SHALL provide filtering by conversation mode (Explore, Work, Branch, Direct)
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

---

### REQ-PROJ-020: Branch Discovery (Local, No Network)

WHEN the user opens the branch picker in Managed mode
THE SYSTEM SHALL list local branches sorted by most-recent commit date (descending)
AND detect the remote's default branch via cached symbolic ref (no network call)

WHEN a local branch has a remote tracking ref (e.g. `origin/<name>`)
THE SYSTEM SHALL compare the local ref against the remote tracking ref
AND display how many commits the local branch is behind the remote tracking ref
AND this comparison SHALL use only local data (no fetch)

WHEN the remote default branch is detectable
THE SYSTEM SHALL include it in the response even if it is not checked out locally

THE SYSTEM SHALL NOT run `git fetch` or any network operation during the no-query
  branch listing path

**Rationale:** The no-query path must be instant regardless of repo size or network
conditions. Local branches sorted by recency surface the branches the user is
actively working on, pushing stale branches down. Behind-remote counts use the
local remote-tracking ref (last fetch), which may be stale but provides a useful
signal at zero cost. The staleness is resolved at materialization time
(REQ-PROJ-022), not at listing time. The user can also search (REQ-PROJ-021)
to get fresh remote data.

---

### REQ-PROJ-021: Remote Branch Search (Network, On-Demand)

WHEN the user types a search query in the branch picker
THE SYSTEM SHALL run `git ls-remote --heads --tags origin` to list remote refs
AND filter the results server-side by case-insensitive substring match on the query
AND return matching branches and version-like tags

THE SYSTEM SHALL cache `git ls-remote` results keyed by repository path
AND the cache TTL SHALL be at least 5 minutes
AND subsequent searches within the TTL SHALL filter the cached result (no network)

WHEN the search returns results
THE SYSTEM SHALL distinguish remote-only branches from branches that also exist locally

THE SYSTEM SHALL NOT download git objects during search (`ls-remote` transfers only
  ref names and SHAs)

**Rationale:** `git ls-remote` lists refs without downloading pack data, making it
fast even on large repositories. Caching the full ref list means rapid successive
keystrokes (typeahead) filter locally after the first network call. The 5-minute
TTL balances freshness against network cost. Substring matching handles the common
patterns: full branch name paste, prefix search (`sopell/`), and keyword search.

---

### REQ-PROJ-022: Branch Materialization (Single-Branch Fetch)

WHEN a Managed conversation's task is approved (worktree creation begins)
THE SYSTEM SHALL run `git fetch origin <base_branch>` (single-branch) before
  creating the worktree, regardless of whether the branch is local or remote-only
AND this fetch SHALL be best-effort (network failure is non-fatal, logged at debug)

WHEN the fetch succeeds AND the branch exists locally
THE SYSTEM SHALL fast-forward the local ref to match the remote tip
  (if fast-forward is not possible, the local ref is left as-is and a warning is logged)
AND use the updated local ref as the worktree base

WHEN the fetch succeeds AND the branch exists only as a remote ref
THE SYSTEM SHALL create a local tracking branch from the fetched remote ref
AND use that local branch as the base for worktree creation

WHEN the fetch fails (network unavailable)
THE SYSTEM SHALL fall back to the local ref if one exists
AND fail with a clear error if no local ref exists

THE SYSTEM SHALL NOT run `git fetch` without a refspec (no blanket fetch)

**Rationale:** Always fetching the selected branch at materialization time ensures
the worktree starts from the latest remote tip. This is a single targeted network
call at a moment where the user already expects a brief wait (worktree creation
involves git operations). It eliminates the "stale local branch" problem without
requiring the user to confirm an update -- the answer is always "yes, give me the
latest." Listing remains instant (REQ-PROJ-020); the network cost moves to the
commit point where it has the highest value.

---

### REQ-PROJ-023: Remote-Aware Commits-Behind Polling

WHEN the commits-behind poller fires for a Work conversation (REQ-PROJ-011)
THE SYSTEM SHALL run `git fetch origin <base_branch>` (single-branch) before
  comparing commit counts
AND this fetch SHALL be best-effort (failures are non-fatal, logged at debug)

THE SYSTEM SHALL compare the task branch against the local base branch ref
  (which is now updated by the single-branch fetch)

**Rationale:** The poller already runs every 60 seconds. Adding a single-branch
fetch before comparison ensures the behind/ahead counts reflect remote state,
not just the local snapshot from the last full fetch. The cost is one lightweight
network call per minute for one branch, not a full repo fetch.

---

### REQ-PROJ-024: Branch Mode -- Work Directly on an Existing Branch

WHEN the user creates a conversation for a git repository AND selects "Branch" mode
AND selects an existing branch
THE SYSTEM SHALL create a worktree checked out to that branch (no new branch created)
AND initialize the conversation directly in Work mode (no Explore phase)
AND give the agent full tool access in the worktree
AND deliver the user's first message to the agent immediately

WHEN the user selects "Branch" mode for a non-git directory
THE SYSTEM SHALL reject the request (Branch mode requires a git repository)

WHEN the user selects "Branch" mode without selecting a branch
THE SYSTEM SHALL reject the request (Branch mode requires an explicit branch selection)

THE SYSTEM SHALL NOT create a task file for Branch mode conversations
THE SYSTEM SHALL NOT create a new branch -- the worktree checks out the existing branch

**Rationale:** Branch mode serves the "fix my PR" workflow: the user has existing
work on a branch and needs to iterate on it. The Explore phase is overhead when
the user already knows the branch and the task. No task file because the branch
pre-exists and the user manages its lifecycle through their normal PR workflow.
No new branch because the point is to commit directly to the existing branch --
the worktree provides isolation from the main checkout without the indirection
of a task branch.

---

### REQ-PROJ-025: One Active Work Conversation Per Branch

WHEN the user selects a branch in Branch mode AND a non-terminal conversation
already has an active worktree on that branch
THE SYSTEM SHALL prompt the user: "This branch is open in another conversation.
Continue there?"
AND offer a link to navigate to the existing conversation

WHEN the user selects a branch AND an orphaned worktree exists for that branch
(worktree on disk but no matching non-terminal conversation)
THE SYSTEM SHALL prompt: "An orphaned worktree exists for this branch.
Delete it and start fresh?"
AND on confirmation, delete the orphaned worktree and create a new one

WHEN the user selects a branch AND a stale conversation exists (conversation
references this branch but no worktree on disk)
THE SYSTEM SHALL redirect the user to the existing conversation
AND the existing conversation SHALL offer the standard Abandon action
  (abandoning frees the branch for a fresh start)

THE SYSTEM SHALL NOT redirect to terminal (abandoned, completed, merged)
conversations -- only to active or idle ones

**Rationale:** Git worktrees hold an exclusive lock on a branch -- two worktrees
cannot check out the same branch. Rather than surfacing this as a git error,
Phoenix makes the constraint visible at branch selection time. The one-per-branch
rule prevents conflicting edits and encourages reusing conversations for
iterative work on the same branch.

---

### REQ-PROJ-026: Branch Mode Lifecycle -- Push, Mark Merged, Abandon

WHILE a conversation is in Branch mode Work state
THE SYSTEM SHALL allow the agent to commit and push to the branch when instructed
AND the conversation SHALL remain in Work mode after pushing (push is not terminal)

WHEN the user initiates "Mark as merged" on a Branch mode conversation
THE SYSTEM SHALL delete the worktree (keep the branch -- it is not ours to delete)
AND transition the conversation to terminal state

WHEN the user initiates "Abandon" on a Branch mode conversation
THE SYSTEM SHALL delete the worktree (keep the branch)
AND transition the conversation to terminal state

THE SYSTEM SHALL NOT offer "Complete (squash-merge)" for Branch mode conversations

**Rationale:** Branch mode conversations track the PR lifecycle, not the task
lifecycle. Push is a milestone, not an endpoint -- the PR may need reviews, CI
fixes, and follow-up pushes before merge. "Mark as merged" is the user-initiated
terminal action when the PR is merged through their normal workflow. Abandon
means "I'm done with this conversation" but doesn't touch the branch. In both
cases the branch survives because it belongs to the user's PR, not to Phoenix.

---

### REQ-PROJ-027: Simplified Managed Mode Completion -- Push Branch

WHEN the user initiates completion of a Managed mode conversation
THE SYSTEM SHALL push the task branch to origin
AND the conversation SHALL remain in Work mode after pushing

WHEN the user initiates "Mark as merged" on a Managed mode conversation
THE SYSTEM SHALL delete the worktree AND delete the task branch
AND transition the conversation to terminal state

WHEN the user initiates "Abandon" on a Managed mode conversation
THE SYSTEM SHALL delete the worktree AND delete the task branch
AND transition the conversation to terminal state

THE SYSTEM SHALL NOT squash-merge to the base branch
THE SYSTEM SHALL commit the task file on the task branch (never on main/base)

**Rationale:** Many repositories protect their main branch and require PR-based
merges. Squash-merging in Phoenix bypasses code review and branch protection
rules. Pushing the task branch and letting the user merge via PR is simpler,
works with protected branches, and aligns with how teams actually ship code.
The task file lives on the task branch alongside the code changes, keeping the
task branch self-contained. On "Mark as merged," Phoenix cleans up both the
worktree and the task branch (since Phoenix created it). On abandon, same
cleanup -- the task branch was a Phoenix artifact that the user is discarding.

---

### REQ-PROJ-028: Managed Mode -- Worktree from First Message

WHEN the user selects Managed mode AND sends their first message
THE SYSTEM SHALL create the worktree and task branch immediately
AND initialize the conversation in Explore mode within the worktree
AND the agent SHALL read from the worktree (not the main checkout)

WHEN the agent calls `propose_plan` in a Managed conversation with a worktree
THE SYSTEM SHALL intercept the call (same as REQ-PROJ-003)
AND on approval, transition the conversation from Explore to Work mode
AND the agent SHALL begin writing in the same worktree (no second worktree created)

WHEN a Managed conversation with a worktree reaches Terminal state without
ever entering Work mode (user never approved a task)
THE SYSTEM SHALL delete the worktree and task branch during cleanup
AND worktree reconciliation on server restart SHALL detect Explore conversations
  with worktrees (not only Work conversations)

**Rationale:** The Explore phase should read from the selected branch's code,
not whatever the main checkout happens to be (which may be dirty, detached, or
on a different branch). Creating the worktree at conversation start ensures the
agent explores the right code. The Explore -> Work transition becomes a permission
change (read-only to read-write) within the same worktree, not a workspace change.
The cleanup clause ensures worktrees from abandoned Explore conversations don't
accumulate on disk.

---

### REQ-PROJ-029: Branch Mode in the Mode Picker

WHEN the directory is a git repository
THE SYSTEM SHALL show three mode options: Direct, Managed, and Branch
AND Branch mode SHALL require selecting an existing branch

WHEN the user selects Branch mode
THE SYSTEM SHALL show the branch picker (same as Managed mode)
AND the branch picker SHALL use the same search and discovery mechanisms
(REQ-PROJ-020 through REQ-PROJ-023)

WHEN the user selects Managed mode
THE SYSTEM SHALL show the branch picker for selecting a base branch
AND the label SHALL indicate "Base branch" (starting point for new work)

WHEN the user selects Branch mode
THE SYSTEM SHALL show the branch picker for selecting an existing branch
AND the label SHALL indicate "Branch" (the branch to work on directly)

**Rationale:** The mode picker is the decision point where the user declares
their intent: "no git" (Direct), "start new work" (Managed), or "work on
existing branch" (Branch). The branch picker is reused across Managed and
Branch modes with different labeling to communicate the different semantics:
"base branch" (starting point) vs "branch" (destination).

