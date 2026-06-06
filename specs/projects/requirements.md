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
AND configure all tools in read-only mode
AND (on the user's first message) create a worktree on a temp branch off the chosen base
  branch and run the conversation in that worktree (REQ-PROJ-028)

WHILE a conversation is in Explore mode (Managed workflow)
THE SYSTEM SHALL prevent file writes to the project via any tool
  (except drafting a task file under the project's tasks directory — REQ-PROJ-003)
AND SHALL allow unrestricted file reading, directory listing, and read-only command execution

WHEN the user selects "Managed" mode for a non-git directory
THE SYSTEM SHALL reject the request (Managed mode requires a git repository)

**Rationale:** Direct mode is the natural, zero-friction starting point for most work.
The Managed (Explore/Work) lifecycle adds value for non-trivial changes that benefit
from plan review and worktree isolation, but should be opt-in rather than mandatory.

---

### REQ-PROJ-003: Propose a Task to Initiate Work Mode

WHILE a conversation is in Explore mode
THE SYSTEM SHALL allow the agent to draft a markdown task file using the `patch`
tool (whose Explore-mode allowlist is scoped to the project's tasks directory).
The recommended form is the taskmd 1.0 convention (`NNNNN-pX-status--slug.md`, status
one of `ready` / `in-progress` / `brainstorming`) — taskmd-named files additionally
yield id/priority/status/slug and a `ready` → `in-progress` rename on approval
(REQ-PROJ-006) and **must live under the project's tasks directory**. Any other `.md`
file is also accepted as a plain task brief (task 13009) — no structured metadata, no
status rename — and may live anywhere in the worktree

WHEN agent calls the `propose_task` tool with a `task_file` path to a markdown file
inside the worktree
THE SYSTEM SHALL intercept it at the LlmResponse handler (like submit_result)
AND require it to be the only tool call in the response
AND NOT execute any side effects (no git operations)
AND read the file and persist the assistant message and a synthetic tool result atomically
AND transition the conversation to AwaitingTaskApproval state
AND pause agent execution until the user responds

WHEN the `task_file` name parses as a taskmd filename but the path is **not** under the
project's tasks directory
THE SYSTEM SHALL reject the call (taskmd-named files must live under the tasks dir; a
brief that wants to live elsewhere must not use the taskmd naming)

WHEN the task file's name parses as taskmd but its status is not `ready` /
`in-progress` / `brainstorming` (e.g. `done`)
THE SYSTEM SHALL reject the call (a closed task cannot be proposed for approval).
For a non-taskmd-named `.md` file there is no status segment, so this check does not apply.

THE AwaitingTaskApproval state SHALL carry the `task_file` path plus a display copy of
the title, priority (taken from a taskmd filename; `p2` for a plain-markdown file), and
body (so the prose reader and SSE state payload need not re-read the file); on approval
the executor SHALL re-read the file from disk as the source of truth

WHEN `propose_task` is called in a writing mode (Work, Branch, or Direct whose working
directory is inside a git repository)
THE SYSTEM SHALL treat it as a non-blocking fork proposal (REQ-PROJ-033/036), NOT the
Explore gateway — this requirement governs only the Explore-mode parking behavior

WHEN `propose_task` is called in Direct mode whose working directory is not inside a git
repository
THE SYSTEM SHALL NOT provide the tool at all (REQ-PROJ-036)

WHEN `propose_task` is called by a sub-agent
THE SYSTEM SHALL reject the call (task management is the parent conversation's job)

**Rationale:** The task file is a real file the agent edits with `patch`, so revisions
are file edits rather than full plans round-tripped through tool arguments. taskmd is
the *default* (the filename carries id/priority/status/slug; Phoenix allocates no ID and
reads no frontmatter), but it is not a hard dependency: a project that hasn't adopted
taskmd can point `propose_task` at any markdown file and the Explore → Work cycle still
works (see `crate::task_source::TaskSource` for the seam). `propose_task` itself is a
pure data carrier (its `run()` is an unreachable fallback): no git work happens until
the user approves.

---

### REQ-PROJ-004: Review and Iterate on Task Plan Before Starting Work

WHEN conversation enters AwaitingTaskApproval state
THE SYSTEM SHALL open the prose reader with the plan content from the state
AND present Approve and Discard actions alongside the standard annotation feedback

WHEN user sends annotation feedback
THE SYSTEM SHALL close the prose reader
AND deliver the annotations to the agent as a user message
AND transition the conversation to Explore mode (Idle)
AND the agent MAY revise the plan and call `propose_task` again
  (which re-enters AwaitingTaskApproval and reopens the prose reader)

WHEN user approves the task AND the file's name parses as a taskmd filename
THE SYSTEM SHALL parse the task ID, priority, status, and slug from the on-disk task
  file's name (it allocates no ID; see REQ-PROJ-006)
AND rename the worktree's temp branch in place to `task-{ID}-{slug}` (REQ-PROJ-028)
AND rename the task file to `...-in-progress--{slug}.md` if it isn't already
AND commit the task file on that task branch in the existing worktree
AND transition the conversation from Explore to Work mode within the same worktree
  (storing worktree_path, branch_name, base_branch, task_id, task_title)
AND resume agent execution with "Task approved. You are on branch task-{ID}-{slug}."

WHEN user approves the task AND the file is a plain-markdown brief (not a taskmd filename)
THE SYSTEM SHALL rename the worktree's temp branch in place to
  `task-{sanitized-file-stem}-{conversation-id-prefix}` (the conversation-id prefix is the
  uniquifier — two conversations proposing files with the same stem must not collide on the
  branch name; the approval mutex only serializes, it does not uniquify)
AND NOT rename the file (a plain brief has no status segment) and NOT call `format_filename`
AND commit the file at its own path on the task branch
AND transition the conversation to Work mode (the `task_id` recorded is the sanitized stem,
  kept non-empty for the conversation record; there is no `task_title` parse so the display
  title — the body's `# H1`, falling back to the stem — is used)

WHEN user discards the task
THE SYSTEM SHALL return the conversation to Explore mode
AND return a rejection result to the agent
AND NOT perform any git operations (the task file stays on disk where the agent
left it under the tasks directory — nothing to commit, nothing to clean up)

**Rationale:** The Explore -> Work transition is a permission upgrade within an
existing worktree (REQ-PROJ-028). The task file already exists on disk (the agent
wrote it with `patch` in Explore mode); approval renames the temp branch, promotes the
file's status to `in-progress` if needed, and commits it on the task branch (never
main -- REQ-PROJ-027). Discarding is cheap: the worktree and temp branch already
exist, and an uncommitted task file on the temp branch is harmless. The prose reader
renders the file's body — surfaced via the display copy in the state, re-read from disk
on approval.

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

WHEN the agent drafts a task file in Explore mode (REQ-PROJ-003)
THE SYSTEM SHALL place it in the project's tasks directory (typically `tasks/`,
  discovered per-project as the first immediate child of the repo root containing
  `_TEMPLATE.md`, falling back to literal `tasks/`; see task 13008)
AND the filename SHALL follow the taskmd 1.0 convention `{ID}-{priority}-{status}--{slug}.md`
  where `ID` is a 5-digit `DDNNN` value (per-directory prefix + monotonic counter) and the
  agent picks the whole filename — the **filename is the sole authoritative source of the
  task's metadata** (id, priority, status, slug); Phoenix neither writes nor reads any
  body frontmatter
AND the body SHALL be free-form markdown (conventionally an `# H1` title, a `## Plan`
  section, and a `## Progress` section the agent updates as work proceeds)

Note on frontmatter: taskmd's validator only checks the filename pattern, so a task file
whose body happens to carry a YAML `created/priority/status/...` block is not *rejected* —
but that block is decorative, never consulted, and may diverge from the filename, so it is
not written. The filename always wins.

WHEN the user approves the task (REQ-PROJ-004)
THE SYSTEM SHALL parse the task ID, priority, status, and slug from the filename
  (`taskmd_core::filename::parse_filename`) — it allocates no ID
AND rename the file to `...-in-progress--{slug}.md` if its status is not already `in-progress`
AND commit the file on the task branch (never on main or the base branch)

WHEN the agent updates the task file during Work mode (via the patch tool)
THE SYSTEM SHALL allow edits to it on the task branch like any other file; those commits
  reach main only when the PR is merged through the user's normal workflow
AND the agent SHALL rename the file to `...-done--{slug}.md` (or `...-wont-do--{slug}.md`)
  itself when the work closes — Phoenix does not rename it

Branch mode conversations (REQ-PROJ-024) do not have task files.

WHEN the task file the agent points `propose_task` at is **not** a taskmd-named file
(any other `.md` file inside the worktree — e.g. `docs/plan.md`, or even `README.md`)
THE SYSTEM SHALL treat it as a plain task brief: the display title is the body's first
  `# H1` (falling back to a title-cased file stem), the display priority defaults to `p2`,
  there is no structured id/status/slug, no on-approve status rename, and the task branch
  is named `task-{sanitized-stem}-{conversation-id-prefix}`
AND the file is committed at its own path on the task branch (a plain `.md` file may live
  anywhere in the worktree — only a *new* file drafted via `patch` is forced under the
  tasks directory by the Explore-mode `patch` scope; the Explore prompt steers the agent
  toward taskmd naming so a project that uses `taskmd validate` stays clean)

**Rationale:** Task files live on the task branch alongside the code changes, keeping
the branch self-contained — no commits to main (which may be protected), no two-path
commit logic. taskmd 1.0's filename-is-truth model means there's nothing for Phoenix to
keep in sync — no authoritative frontmatter, no ID allocation step on the Phoenix side;
the agent owns the filename (including the `done` rename) the same way it owns the code.
taskmd is the default but not a hard dependency (task 13009): pointing `propose_task` at
a plain `.md` file works too — the `TaskSource` seam (`crate::task_source`) picks taskmd
when the filename parses, plain-markdown otherwise. A future explicit "task source"
backend (beads, etc.) would slot in behind the same seam; that is out of scope for v1.

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

### REQ-PROJ-011: PR Status Is the Branch Health Indicator

WHEN a Work or Branch conversation has an associated pull request
THE SYSTEM SHALL display the PR status in the StateBar

THE SYSTEM SHALL NOT display local commits-ahead or commits-behind badges in the StateBar

**Rationale:** PR state is the actionable signal in the normal review workflow. Local
commit divergence badges draw attention without telling the user whether the PR is
ready, merged, blocked, or stale. Users and agents can still inspect git history via
normal git commands when they need that detail.

### REQ-PROJ-030: PR Feedback Freshness Indicator

WHEN a Work or Branch conversation has an open associated pull request
AND PR feedback changed since Phoenix last captured agent-facing PR remediation context
THE SYSTEM SHALL show a compact advisory marker near the `Address CI & comments` Work Action
SUCH AS `new comments`, `{N} new`, or `updated`

THE SYSTEM SHALL NOT use PR feedback freshness as the StateBar branch-health signal

THE SYSTEM SHALL NOT block cleanup, abandon, or ordinary conversation use based on PR feedback freshness

**Rationale:** Fresh review activity is useful exactly where the user asks the agent to
address review feedback. It is not branch health, and it is not lifecycle authority.

### REQ-PROJ-031: Agent-Facing PR Context Baseline

WHEN Phoenix successfully captures PR remediation context for an associated pull request
THE SYSTEM SHALL record that successful capture as the baseline for agent-facing PR feedback freshness

THE SYSTEM SHALL store compact baseline data for the work scope and PR number:
- capture timestamp
- pull request `updated_at` timestamp when available
- stable feedback identities or fingerprints when available

WHEN classifying freshness
THE SYSTEM SHALL treat feedback as new when current feedback contains stable identities absent from the latest successful baseline

WHEN no successful baseline exists
THE SYSTEM SHALL NOT show `new comments`

**Rationale:** The baseline is what Phoenix has actually handed to the agent, not what
GitHub happened to contain at some unrelated time.

### REQ-PROJ-032: Bounded PR Feedback Refresh

WHEN refreshing routine PR status
THE SYSTEM SHALL keep the poll lightweight and SHALL NOT fetch all PR feedback surfaces unless gated by evidence that feedback may have changed

Evidence includes the pull request `updated_at` timestamp being newer than the latest successful baseline, or an explicit remediation context capture

WHEN full feedback surfaces are unavailable during freshness classification
THE SYSTEM SHALL degrade to no count or a coarse `updated` advisory
AND SHALL log the failure

**Rationale:** PR status is polled routinely. Full review surfaces are slower and more
rate-limit sensitive, so Phoenix fetches them only when they can change the advisory.

---

### REQ-PROJ-012: Provide propose_task Tool to Agents

WHEN agent is in Explore mode
THE SYSTEM SHALL provide the `propose_task` tool
WHICH accepts: `task_file` (required string) — a path, relative to the agent's working
  directory, to an existing markdown (`.md`) file inside the worktree. A taskmd 1.0
  filename (`NNNNN-pX-status--slug.md`, status one of `ready` / `in-progress` /
  `brainstorming`, **required to be under the project's tasks directory**) additionally
  derives id/priority/status/slug from the name; any other `.md` file is accepted as a
  plain task brief (REQ-PROJ-006, task 13009) and may live anywhere in the worktree

WHEN `propose_task` is called in a writing mode (Work, Branch, or Direct-in-a-git-repo)
THE SYSTEM SHALL treat it as a fork proposal (REQ-PROJ-033/036) rather than the Explore
gateway described by this requirement
AND in Direct mode whose working directory is not a git repository the tool is not
provided at all (REQ-PROJ-036)

WHEN `propose_task` is called by a sub-agent (in any mode)
THE SYSTEM SHALL reject the call
AND explain that task management is the parent conversation's responsibility

WHEN `propose_task` is not the only tool call in the response
THE SYSTEM SHALL reject it

`propose_task` is a pure data carrier — its `run()` is an unreachable fallback. It is
intercepted at the LlmResponse handler (like submit_result for sub-agents) and never
enters the tool executor; the file path and a display copy of its contents flow into
the AwaitingTaskApproval state. The agent drafts the referenced file with the patch
tool beforehand (or points at an existing task file).

During Work mode, the agent updates the task file directly using the patch tool like
any other file. No dedicated `update_task` tool is needed.

**Rationale:** `propose_task` is the agent's way of saying "here's a task file, please
review it." The name signals human review is required. Referencing a file (rather than
passing the plan inline) means revisions are ordinary file edits and the task's metadata
lives where taskmd 1.0 keeps it — in the filename. The interception follows the
established submit_result pattern, so no git work happens until approval.

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

**Partially DESCOPED:** The dedicated worktree registry *table* is not implemented — `ConvMode::Work` on each conversation serves as the de facto registry, and querying all Work conversations for a project yields the active worktree list. The first three clauses below (register/deregister/reconcile on disk) are therefore conceptual descriptions of that de facto registry's behavior, not a separate table's behavior. The fourth clause (context-exhausted / continued-conversation exclusion) is **normative and active** — it constrains how the conceptual-registry reconciliation treats those conversations, regardless of the registry's storage shape.

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
THE SYSTEM SHALL NOT provide the `propose_task` tool
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

THE Work and Branch modes SHALL each carry:
- `worktree_path` -- path to the conversation's worktree
- `branch_name` -- the task branch (Work) or the existing branch (Branch)
- `base_branch` -- the branch the worktree was created from (== `branch_name` for Branch)

THE Work mode SHALL additionally carry:
- `task_id` -- always present, the structural discriminator vs Branch mode
- `task_title` -- human-readable title, for UI

THE Branch mode SHALL NOT carry `task_id` or `task_title`

THE Explore mode SHALL carry only `worktree_path` (an `Option` — `Some` for a top-level
Managed Explore conversation, which runs in its own worktree on a temp branch before
approval; `None` for an Explore sub-agent, which shares the parent's working directory)

THE Direct mode SHALL carry no git metadata

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
no `propose_task`, no worktree, no task file, no branch, no project
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

### REQ-PROJ-023: Reserved

Remote-aware commits-behind polling was removed when PR status became the StateBar's
branch health indicator.

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

### REQ-PROJ-026: Branch Mode Lifecycle -- Mark Merged, Abandon

WHEN the user initiates "Mark as merged" on a Branch mode conversation
THE SYSTEM SHALL delete the worktree (keep the branch -- it is not ours to delete)
AND transition the conversation to terminal state

WHEN the user initiates "Abandon" on a Branch mode conversation
THE SYSTEM SHALL delete the worktree (keep the branch)
AND transition the conversation to terminal state

THE SYSTEM SHALL NOT offer "Complete (squash-merge)" for Branch mode conversations
THE SYSTEM SHALL NOT push to origin on the user's behalf (push is the agent's
responsibility, run through the bash tool when the user requests it)
THE SYSTEM SHALL use the GitHub CLI, when available, to observe PR state for the
branch and guide the cleanup action
THE SYSTEM SHALL treat a user-asserted manual "Mark as merged" action as a
fallback when PR state is unavailable, not as the preferred happy path

**Rationale:** Branch mode conversations track the PR lifecycle, not the task
lifecycle. The agent commits and pushes from bash on the user's instruction;
Phoenix observes no push event and gates no lifecycle on it. "Mark as merged"
is the user-initiated terminal action when the PR is merged through their
normal workflow. Abandon means "I'm done with this conversation" but doesn't
touch the branch. In both cases the branch survives because it belongs to the
user's PR, not to Phoenix.

---

### REQ-PROJ-027: Simplified Managed Mode Completion -- User Merges via PR

WHEN the user initiates "Mark as merged" on a Managed mode conversation
THE SYSTEM SHALL delete the worktree AND delete the task branch
AND transition the conversation to terminal state

WHEN the user initiates "Abandon" on a Managed mode conversation
THE SYSTEM SHALL delete the worktree AND delete the task branch
AND transition the conversation to terminal state

THE SYSTEM SHALL NOT squash-merge to the base branch
THE SYSTEM SHALL NOT push to origin (push is the agent's responsibility, run
through the bash tool when the user requests it)
THE SYSTEM SHALL commit the task file on the task branch (never on main/base)

**Rationale:** Many repositories protect their main branch and require PR-based
merges. Squash-merging in Phoenix bypasses code review and branch protection
rules. Letting the user merge through their normal PR workflow is simpler,
works with protected branches, and aligns with how teams actually ship code.
The task file lives on the task branch alongside the code changes, keeping the
task branch self-contained. Phoenix never pushes on the user's behalf — the
agent runs `git push` from bash if and when instructed; Phoenix observes no
push event and gates no lifecycle on it. When `gh` can observe a PR for the
branch, Phoenix uses that state to make merged PR cleanup the happy path and to
discourage local cleanup while the PR is still open, draft, failing, pending, or
closed-unmerged. On "Mark as merged" / merged-PR cleanup, Phoenix cleans
up both the worktree and the task branch (since Phoenix created it). On
abandon, same cleanup -- the task branch was a Phoenix artifact that the user
is discarding.

---

### REQ-PROJ-028: Managed Mode -- Worktree from First Message

WHEN the user selects Managed mode AND sends their first message
THE SYSTEM SHALL create the worktree and task branch immediately
AND initialize the conversation in Explore mode within the worktree
AND the agent SHALL read from the worktree (not the main checkout)

WHEN the agent calls `propose_task` in a Managed conversation with a worktree
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


### REQ-PROJ-033: Propose a Decoupled Task Fork from a Writing Mode

WHILE a conversation is in a writing mode (Work, Branch, or Direct whose working
directory is inside a git repository)
THE SYSTEM SHALL provide the `propose_task` tool as a **fork proposal**: the agent's
way of handing a self-contained, newly-discovered unit of work to a separate
conversation without carrying it itself.

WHEN the agent calls `propose_task` with a `task_file` path to a markdown file inside
its working tree
THE SYSTEM SHALL intercept it at the LlmResponse handler (like the Explore-mode
propose_task — REQ-PROJ-003)
AND require it to be the only tool call in the response
AND validate the `task_file` by the **same rules as REQ-PROJ-003**: it must be a regular
`.md` file inside the worktree; a taskmd-pattern filename (`NNNNN-pX-status--slug.md`) is
rejected unless it lives under the project's tasks directory and carries an open status
(`ready` / `in-progress` / `brainstorming`) — a closed status such as `done` is rejected;
any other `.md` file is accepted as a plain brief (REQ-PROJ-006). An invalid file is
rejected via a synthetic tool error (the conversation keeps running), never silently
reclassified
AND read the file and capture a **content snapshot** (the file bytes plus the display
title/priority derived exactly as in REQ-PROJ-003, the file path **normalized to
repository-relative** — the agent's path is relative to its working directory, which for a
Direct origin started in a subdirectory is a repo subdirectory, so it is resolved against
the repo root so the fork commits it at the correct location — and a stable `proposal_id`)
into a fork-proposal record
AND persist the assistant message and a synthetic tool result reporting that the
proposal was recorded and is pending the user's review
AND **return the conversation to its running state so the agent continues its own work**
(it SHALL NOT enter AwaitingTaskApproval and SHALL NOT pause)

THE fork proposal is a content snapshot, not a live file reference: the fork runs in a
fresh worktree off the **repository's default branch** (REQ-PROJ-034) that does not
contain the originating worktree's file, so the bytes captured at propose time are
authoritative. The file the agent drafted stays on the originating branch as an ordinary
tracked task file; the fork gets its own committed copy.

WHEN `propose_task` is called in Explore mode
THE SYSTEM SHALL keep the existing parking behavior (REQ-PROJ-003) — Explore's
propose_task is the in-place Explore→Work gateway, not a fork

WHEN `propose_task` is called in Direct mode AND the working directory is **not** inside
a git repository
THE SYSTEM SHALL NOT provide the tool (a fork cuts from the repository's default branch, which requires a git repository)

WHEN `propose_task` is called by a sub-agent
THE SYSTEM SHALL reject the call (task management is the parent conversation's job —
REQ-PROJ-003)

**Rationale:** A conversation refactoring one subsystem often discovers an unrelated,
self-contained problem (a bug in a different module). Carrying that work dilutes the
conversation's focus. A fork proposal lets the agent record "here is a separate task,
fully described" and immediately drop it from its own context. The proposal is
non-blocking precisely because the originating conversation has its own task to finish —
unlike Explore, whose entire purpose is to become the proposed task. This is the same
hand-off pattern as Explore's propose_task; the only new axis is that the originating
conversation keeps going.

---

### REQ-PROJ-034: Approve a Fork Proposal — Spawn an Independent Conversation

WHEN a fork proposal exists on a conversation AND the originating conversation is not
terminal
THE SYSTEM SHALL surface it in that conversation's tool output with a Review affordance
AND the user MAY open the same full-screen task-review interface used for Explore
approvals (REQ-PROJ-004) to read the snapshot and Approve or Dismiss it (addressed by its
`proposal_id`)
AND the user's decision arrives asynchronously — the originating conversation does not
wait on it and is unaffected by it

WHEN the originating conversation reaches a terminal state (abandoned / merged) with a
`pending` proposal still un-reviewed
THE SYSTEM SHALL make that proposal moot: the Review affordance is withdrawn and the
proposal can no longer be approved (REQ-PROJ-035, bound to origin)

WHEN the user approves a `pending` fork proposal
THE SYSTEM SHALL create a fresh top-level conversation in Work mode, in its own new
worktree (REQ-PROJ-005), on a new task branch **cut from the repository's default branch
(the project's `main_ref` — REQ-PROJ-034a), not from the originating conversation's
`base_branch` or HEAD** — uniformly for every origin mode (Work, Branch, Direct-in-git)
AND write the snapshot's bytes into the fork's worktree (a taskmd file under the tasks
directory, a plain brief at its own path — classified as in REQ-PROJ-006); for a taskmd
file, promote its status to `in-progress` before committing exactly as REQ-PROJ-006 does
on Explore approval; then commit the task file on the fork's task branch
AND seed the fork's LLM context with the task brief — the snapshot **body** itself, plus
a line naming the fork's **resolved `branch_name`** (`task-{ID}-{slug}` for a taskmd file,
`task-{stem}-{fork-id-prefix}` for a plain brief — never a fixed taskmd-shaped template,
which would name a branch that was never created) — and nothing else (it inherits none of
the originating conversation's transcript; the brief is in context, not merely a file the
agent must discover)
AND record the resolution of the proposal as **spawned** (referencing the new
conversation), idempotently: a `pending` proposal spawns exactly one fork; approving an
already-`spawned` or `dismissed` proposal is rejected

WHEN the user dismisses a `pending` fork proposal
THE SYSTEM SHALL spawn nothing
AND record the resolution of the proposal as **dismissed**
AND leave the originating conversation unaffected

**REQ-PROJ-034a (fork base):** The fork base is **always the repository's default branch**,
named by the project's `main_ref` — a mandatory, immutable field resolved once at project
creation (the remote's default branch when detectable, else the repository's checked-out
branch at creation time). It is the same value regardless of origin mode; in particular a
Branch-mode origin's `base_branch` equals the branch it is editing (REQ-PROJ-017), which
is *not* an independent base, so the fork never uses it.

THE fork SHALL be independent by construction: branching off the repository default branch
means its eventual diff contains only its own work, reviewable and mergeable on its own
with no entanglement with the originating conversation's in-progress changes. A fork
therefore SHALL NOT be used for work that depends on the originator's uncommitted changes.

**Rationale:** Approval is the human gate, moved off the blocking path. The originating
conversation proposed and moved on; the user picks the proposal up whenever, in the
familiar review surface. Spawning off the repository default branch keeps the fork a clean,
independent unit — the alternatives (off the origin's HEAD, or off a Branch origin's own
PR branch) would stack it on unmerged work and muddy its PR.

---

### REQ-PROJ-035: Fork Provenance and Decoupling Guarantees

WHEN a fork conversation is created (REQ-PROJ-034)
THE SYSTEM SHALL record a provenance breadcrumb `spawned_from_conversation_id` on the
fork, pointing at the originating conversation
AND this field SHALL be distinct from `parent_conversation_id` (sub-agent / fresh-handoff
lifecycle) and from `continued_in_conv_id` (chains — REQ-BED-030), and SHALL carry no
lifecycle or notification semantics

THE SYSTEM SHALL NOT establish any lifecycle relationship between the originating
conversation and the fork:
- the fork SHALL NOT be a sub-agent of the originator (no AwaitingSubAgentResult, no
  SubAgentResult — REQ-PROJ-008)
- the fork SHALL NOT be a chain continuation of the originator (no `continued_in_conv_id`,
  no shared chain root)
- the originating conversation SHALL receive no notification of the fork's creation,
  progress, completion, or failure, through the agent's context or otherwise

WHEN a fork proposal's resolution is recorded (spawned or dismissed — REQ-PROJ-034)
THE SYSTEM SHALL track that resolution as control-plane state bound to the originating
conversation's lifecycle (removed with the conversation)
AND SHALL NOT inject it into the originating conversation's LLM context (a resolution the
agent could read would itself be a lifecycle notification, which is forbidden above)

A fork proposal is **bound to its origin**: it lives with the originating conversation's
transcript and does not survive that conversation's termination. If the originating
conversation is abandoned before the proposal is reviewed, the proposal is moot.

**Rationale:** The whole point is to shed mental load — so the spawner must learn nothing
about what it spawned. The decoupling is structural: the fork is created through the
ordinary top-level-conversation path, never the sub-agent spawn path, so there is no
parent event channel to carry a notification (it cannot leak because it does not exist).
The breadcrumb exists only for the UI and audit; nothing keys behavior off it. Binding the
proposal to its origin keeps the model simple — a proposal is an artifact of a
conversation, not a free-floating queue item — at the cost that an unreviewed proposal
dies with its conversation, which is acceptable because the proposal carries no commitment
until approved.

---

### REQ-PROJ-036: Fork-Eligible Mode Availability

THE `propose_task` tool SHALL be available as follows:

| Origin mode | `propose_task` behavior |
|-------------|-------------------------|
| Explore | In-place Explore→Work gateway, parks in AwaitingTaskApproval (REQ-PROJ-003) |
| Work | Fork proposal, non-blocking (REQ-PROJ-033) |
| Branch | Fork proposal, non-blocking (REQ-PROJ-033) |
| Direct, working dir inside a git repo | Fork proposal, non-blocking (REQ-PROJ-033) |
| Direct, working dir not in a git repo | Not provided (no repository default branch to cut from) |
| Any sub-agent | Not provided (REQ-PROJ-008) |

A Direct origin owns no worktree or task ceremony of its own, yet the fork it proposes is
a managed Work conversation (own worktree, own task branch off the repository's default
branch). The fork is managed even when its origin is not.

**Rationale:** Forking is meaningful from any mode that sits on a git history to branch
from. Explore keeps its distinct, blocking gateway semantics because there the proposal
*is* the conversation's purpose. Direct-not-in-a-repo is the one writing context with no
branch to fork from, so it is excluded structurally rather than failing at spawn time.

---

## WorkScope Resource Ownership

### REQ-PROJ-WS-001: WorkScope as Resource Owner

Work-affine resources SHOULD be owned by `WorkScope`. Managed/Branch work uses the managed worktree path; Direct conversations use their conversation id as the fallback scope.

