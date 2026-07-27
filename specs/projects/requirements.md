# Projects: Git-Backed Workspaces with Disposable Worktrees and Guided Task Handoffs

## User Story

As a developer using PhoenixIDE, I need Git-backed conversations to start in a predictable disposable workspace, preserve clear task handoffs, and let Phoenix coordinate write authority safely without making lifecycle or repository ownership depend on mode-specific branch ceremony.

## Transparency Contract

The user must be able to confidently answer these questions:

**At a glance:**
1. Which projects do I have active?
2. For each project: how many conversations are open? Any active tasks?
3. Is a conversation currently read-only planning, actively writing, or closed to History?

**For any conversation:**
4. What project does this belong to?
5. What capabilities are currently available?
6. If it owns a disposable worktree: what task is it pursuing, and where is that worktree?

**For project health:**
7. Are there any orphaned worktrees?
8. What branches/worktrees are active across all conversations?

## Requirements

### REQ-PROJ-000: Conversation Working Directory Root Floor

WHEN a conversation is created or resumed with a working directory
THE SYSTEM SHALL resolve symlinks and parent-directory traversal before accepting the path
AND reject the working directory if it resolves to the filesystem root
AND reject the working directory if the system cannot prove it is an existing directory

WHEN a sub-agent working directory is inherited from its parent or supplied as an override
THE SYSTEM SHALL apply the same validation before the sub-agent conversation is persisted or run

**Rationale:** A system-root working directory makes relative tool paths resolve from the
entire filesystem. Even read-only tools can consume unbounded resources when search or
listing operations start at the filesystem root, so the root floor is independent of
conversation mode and write permissions.

---

### REQ-PROJ-001: Open a Git Repository as a Project

WHEN the user creates a new conversation by providing a directory path
THE SYSTEM SHALL detect whether the directory is inside a git repository
AND, if it is, treat the repository root as the project's canonical path
AND associate the conversation with that project
AND create the conversation as an Open Git-backed conversation without asking the user to choose Explore, Work, Managed, or Branch lifecycle modes
AND provision one Phoenix-owned disposable worktree for that conversation using the repository's canonical default-branch starting point
AND start that worktree at detached `HEAD`

WHEN the directory is NOT inside a git repository
THE SYSTEM SHALL create the conversation in Direct mode (REQ-PROJ-018)
AND NOT associate it with any project

**Rationale:** Users think in terms of projects (codebases, repositories), not raw directories. In the current product model, a Git-backed conversation always starts from one disposable Phoenix-owned workspace rather than making the user choose among branch/lifecycle variants up front.

---

### REQ-PROJ-001A: Suggest Known Projects for New Conversations

WHEN the user opens the new-conversation page
THE SYSTEM SHALL obtain project suggestions from the server's known project records
AND rank projects with more active conversations ahead of projects with fewer active conversations
AND use project recency to order projects with equal active-conversation counts
AND allow the user to select a suggested project's canonical path as the conversation working directory

---

### REQ-PROJ-002: Git-Backed Creation Has No Mode or Branch Picker

WHEN the user creates a new conversation for a Git repository
THE SYSTEM SHALL NOT ask the user to choose Managed, Work, Branch, Explore, or equivalent lifecycle modes
AND SHALL NOT ask the user to choose a starting branch for ordinary conversation creation
AND SHALL start from the repository's canonical default-branch commit in the detached disposable worktree defined by REQ-PROJ-001

WHEN a conversation is created for a non-Git directory
THE SYSTEM SHALL initialize it in Direct mode by default
AND provide full tool access (bash, patch, all tools)

**Rationale:** The unified lifecycle removes branch- and mode-selection ceremony from ordinary conversation creation. Git-backed conversations begin in the same Phoenix-owned disposable workspace model; chat-only conversations remain Direct because there is no repository-backed worktree to provision.

---

### REQ-PROJ-003: Propose a Task for Blocking Review

WHILE a Git-backed conversation is in a planning/read-only phase
THE SYSTEM SHALL allow the agent to draft a markdown task file using the `patch` tool, scoped to the discovered tasks directory when task drafting rules require it

WHEN the agent calls the `propose_task` tool with a `task_file` path to a markdown file inside the conversation's allowed workspace
THE SYSTEM SHALL intercept it at the LlmResponse handler (like submit_result)
AND require it to be the only tool call in the response
AND NOT execute any immediate Git side effects
AND read the file and persist the assistant message and a synthetic tool result atomically
AND transition the conversation to AwaitingTaskApproval state
AND pause ordinary agent execution until the user responds

WHEN the task file's name parses as a taskmd filename but the path is **not** under the project's discovered tasks directory
THE SYSTEM SHALL reject the call

WHEN the task file's name parses as taskmd but its status is not `ready` / `in-progress` / `brainstorming`
THE SYSTEM SHALL reject the call

THE AwaitingTaskApproval state SHALL carry the `task_file` path plus a display copy of the title, priority, and body; on approval the executor SHALL re-read the file from disk as the source of truth

WHEN `propose_task` is called in a non-Git Direct conversation
THE SYSTEM SHALL NOT provide the tool at all

WHEN `propose_task` is called by a sub-agent
THE SYSTEM SHALL reject the call because task-management authority belongs to the parent conversation

**Rationale:** The task file is a real file the agent edits with `patch`, so revisions are normal file edits rather than plan text hidden in tool arguments. Blocking review preserves user approval without making task approval synonymous with lifecycle mode changes.

---

### REQ-PROJ-004: Review and Place an Approved Task

WHEN a conversation enters AwaitingTaskApproval state
THE SYSTEM SHALL open the prose reader with the plan content from the state
AND present **Continue here**, **Start in new conversation**, and **Request changes / discard** actions alongside the standard annotation feedback

WHEN the user sends annotation feedback
THE SYSTEM SHALL close the prose reader
AND deliver the annotations to the agent as a user message
AND return the conversation to its prior Open planning state
AND the agent MAY revise the plan and call `propose_task` again

WHEN the user approves the task and chooses **Continue here**
THE SYSTEM SHALL commit the approved task artifact according to REQ-PROJ-006
AND keep the same Open conversation and the same `WorkScope`
AND resume execution in that conversation
AND SHALL NOT create, rename, select, or delete a branch as an approval side effect

WHEN the user approves the task and chooses **Start in new conversation**
THE SYSTEM SHALL create a separate Open conversation derived from the source conversation
AND provision a fresh detached-default-branch disposable worktree for the spawned conversation
AND seed only the exact approved task as the spawned conversation's starting context
AND preserve the approved task artifact independently of the source worktree's eventual closure by storing one normalized approved-task source record and by materializing the approved artifact in the spawned worktree
AND record exactly one source relation of kind `approved_task` on the spawned conversation that points to the source conversation
AND resume execution in the spawned conversation
AND leave the source conversation Open
AND SHALL NOT create, rename, select, or delete a branch as an approval side effect
AND SHALL NOT copy, summarize, or inject the source conversation transcript into the spawned conversation as part of approval placement

WHEN the user rejects or discards the task
THE SYSTEM SHALL return a rejection result to the agent
AND SHALL NOT perform any Git side effects

**Rationale:** Approval is a user decision about placement and authority, not a hidden transition into branch-backed lifecycle modes. The current product offers two deliberate placements — continue in the same conversation or start fresh in a new one — while keeping branch ownership out of the approval contract.

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

### REQ-PROJ-005A: Worktree Build Cache Pre-Warm Is Best-Effort

WHEN Phoenix creates a new conversation worktree under `.phoenix/worktrees/{conversation-id}/`
THE SYSTEM MAY pre-warm allowlisted project-local build cache directories from the repository root
AND SHALL keep the destination paths inside the new worktree
AND SHALL skip missing sources and existing destination paths
AND SHALL use copy-on-write clone semantics only when supported by the platform/filesystem
AND SHALL NOT fall back to a large physical copy
AND SHALL NOT fail worktree creation when pre-warm cloning is unsupported or fails

The allowlist is intentionally narrow: `node_modules/.cache/`, `.next/cache/`, `.turbo/`,
and project-local `.vite/`. Phoenix does not pre-warm Cargo `target/` artifacts, `.git/`,
`.phoenix*`, lock files, sockets, PID files, arbitrary ignored directories, or full dependency
trees such as `node_modules/`.

**Rationale:** Pre-warming improves time-to-first-build for isolated Phoenix worktrees on
filesystems with cheap block cloning, while preserving the isolation guarantee. The cloned
files occupy independent worktree paths and diverge normally when rebuilt; failure to clone
only loses an optimization, never correctness.

---

### REQ-PROJ-006: Task Files as Versioned Living Contracts

WHEN the agent drafts a task file for `propose_task`
THE SYSTEM SHALL place taskmd-named drafts in the project's discovered tasks directory: Phoenix scans immediate children of the repo root for taskmd sentinel files and prefers `tasks/`, otherwise the lexically-first discovered taskmd directory, otherwise literal `tasks/`
AND the filename SHALL follow the taskmd 1.0 convention `{ID}-{priority}-{status}--{slug}.md` when the project uses taskmd naming
AND the **filename** SHALL remain the sole authoritative source of taskmd metadata
AND the body SHALL be free-form markdown

WHEN the user approves a taskmd task
THE SYSTEM SHALL parse the task ID, priority, status, and slug from the filename
AND rename the file to `...-in-progress--{slug}.md` if its status is not already `in-progress`
AND persist the approved content so the chosen conversation placement can continue to use the exact approved task

WHEN the agent later updates the task file during active work
THE SYSTEM SHALL allow edits to it like any other workspace file
AND the agent MAY rename it to `...-done--{slug}.md` or `...-wont-do--{slug}.md` when the work is complete

WHEN the task file is not taskmd-named
THE SYSTEM SHALL treat it as a plain task brief: the display title is the body's first `# H1` (falling back to a title-cased file stem), the display priority defaults to `p2`, and there is no structured id/status/slug contract

**Rationale:** The task artifact is the durable handoff, regardless of whether the user continues in the same conversation or starts a new one. taskmd metadata remains filename-based, but task approval no longer depends on creating or owning a dedicated Phoenix branch lifecycle.

---

### REQ-PROJ-007: Git-Backed Write Authority Is Scoped to the Conversation Worktree

WHILE a Git-backed conversation has write authority
THE SYSTEM SHALL configure tools to operate within the conversation's worktree directory
AND enable file-write tools within that worktree
AND allow bash commands that read and write files within that worktree

WHEN a tool with write authority attempts to write outside the worktree directory
THE SYSTEM SHALL block the write
AND return a descriptive error

**Rationale:** Write authority is scoped to the disposable worktree, not to the whole filesystem and not to a lifecycle mode name. This preserves isolation without requiring a separate writing lifecycle label as a product concept.

---

### REQ-PROJ-008: Sub-Agent Capabilities Inherit the Parent Workspace Authority

WHEN a Git-backed parent conversation spawns a sub-agent with write authority requested
THE SYSTEM SHALL configure the sub-agent's working directory as the parent's worktree
AND grant write access to that same worktree
AND allow only one write-authority sub-agent per parent conversation at a time
AND place the parent conversation in AwaitingSubAgentResult state for the duration

WHEN a Git-backed parent conversation spawns a sub-agent with read-only authority requested
THE SYSTEM SHALL configure the sub-agent's working directory as the parent's worktree
AND grant read-only authority there
AND allow multiple read-only sub-agents in parallel

WHEN a planning/read-only conversation spawns sub-agents
THE SYSTEM SHALL configure those sub-agents with read-only authority

**Rationale:** The important distinction is execution authority, not lifecycle naming. Phoenix must preserve the single-writer guarantee for one worktree while still allowing parallel read-only analysis.

---

### REQ-PROJ-009: Complete a Task (Squash Merge)

**DEPRECATED:** Superseded by `specs/work-lifecycle/requirements.md` REQ-WL-001 through REQ-WL-003.
Squash-merge bypasses code review and branch protection rules. The current model
uses one explicit Close flow that leaves branches and pull requests untouched.
Retained for historical context only.

---

### Work lifecycle and PR association (specified in dedicated specs)

The explicit **Close conversation** lifecycle and its PR-aware guidance are specified by the **work-lifecycle** spec (REQ-WL-001 through REQ-WL-003). PR association, status, and feedback freshness are specified by the **pr-association** spec (REQ-PRA-001 through REQ-PRA-004). This spec covers project setup, disposable worktrees, task authoring, and branch/repository provenance.

---

### REQ-PROJ-012: Provide propose_task Tool to Parent Conversations

WHEN a parent conversation is allowed to request task review or derived follow-up work
THE SYSTEM SHALL provide the `propose_task` tool
WHICH accepts `task_file` (required string): a path, relative to the agent's working directory, to an existing markdown (`.md`) file inside the allowed workspace

WHEN `propose_task` is called from a planning/read-only phase
THE SYSTEM SHALL treat it as the blocking review path defined by REQ-PROJ-003 and REQ-PROJ-004

WHEN `propose_task` is called from a Git-backed conversation that already has write authority
THE SYSTEM SHALL treat it as the same blocking review path used for planning/read-only conversations
AND SHALL NOT reinterpret that call as a nonblocking derived-conversation proposal

WHEN `propose_task` is called in a chat-only Direct conversation
THE SYSTEM SHALL NOT provide the tool, even if its working directory happens to be inside a Git repository

WHEN `propose_task` is called by a sub-agent
THE SYSTEM SHALL reject the call
AND explain that task-management authority belongs to the parent conversation

WHEN `propose_task` is not the only tool call in the response
THE SYSTEM SHALL reject it

`propose_task` is a pure data carrier intercepted at the LlmResponse handler. It never performs Git side effects directly.

**Rationale:** `propose_task` is the agent's way of saying “here's a task artifact for human review.” The key distinction is whether the parent is asking for blocking approval in place or proposing a separate derived conversation — not whether the product is in one named lifecycle mode or another.

---

### REQ-PROJ-013: Platform Capability Detection

WHEN the server starts
THE SYSTEM SHALL ask `nono::Sandbox::support_info()` whether the host has an enforceable OS sandbox backend capable of applying the read-only planning bash network-block and unrelated-process signal-isolation policy

THE SYSTEM SHALL re-check capabilities on every startup

WHILE sandbox is not available
THE SYSTEM SHALL provide top-level planning/read-only conversations with read-only/planning tools, browser tools, scoped task-proposal `patch`, `propose_task`, and parent coordination tools including `spawn_agents`
AND SHALL NOT provide `bash`, `tmux`, `tmux_run`, or any tool that can execute arbitrary unsandboxed commands

WHILE sandbox is available
THE SYSTEM SHALL provide top-level planning/read-only conversations with read-only/planning tools, scoped task-proposal `patch`, `propose_task`, browser tools, parent coordination tools including `spawn_agents`, and sandboxed `bash`
AND SHALL NOT provide `tmux` or `tmux_run`

**Rationale:** Capabilities are a property of the running environment, not the application. Sandbox capability gates bash, not delegation. Planning/read-only conversations and their sub-agents can still investigate locally without write authority when the host can enforce network blocking and unrelated-process signal isolation; otherwise withholding bash preserves that promise structurally. Re-checking on startup keeps the tool set aligned with the current host.

---

### REQ-PROJ-014: Project UI

WHEN displaying the conversation sidebar
THE SYSTEM SHALL show a project switcher (tabs) at the top of the sidebar
AND group conversations under their project

WHEN a project has active Git-backed conversations pursuing tasks
THE SYSTEM SHALL indicate the active task count next to the project name

WHEN the user selects a project tab
THE SYSTEM SHALL show only that project's conversations

WHEN displaying a conversation
THE SYSTEM SHALL indicate its current capabilities and whether it is Open or in History

**Rationale:** Users manage multiple projects. A project switcher reduces cognitive load compared to a flat list mixing conversations from different codebases. Capability and lifecycle visibility prevent confusion without reintroducing obsolete mode labels.

---

### REQ-PROJ-015: Project Worktree Registry

WHEN a Phoenix-owned disposable worktree exists for a conversation
THE SYSTEM SHALL register enough data to report its owning conversation, stable owning ProductConversation, worktree path, and `WorkScope`

WHEN the server starts
THE SYSTEM SHALL reconcile the registry against worktrees on disk
AND clean up orphaned registry entries
AND report worktrees that exist on disk but have no registry entry

WHEN a conversation is in context-exhausted state and has not transferred ownership through `continued_in_conv_id`
THE SYSTEM SHALL NOT treat its worktree as orphaned during reconciliation

WHEN a transcript row has transferred execution through `continued_in_conv_id`
THE SYSTEM SHALL treat that row as a historical transcript segment rather than as an independent WorkScope authority
AND SHALL derive the latest execution row from continuation topology rather than storing a second ownership authority
AND SHALL keep the same stable ProductConversation-scoped `WorkScope` identity and the same filesystem worktree path across that continuation
AND SHALL preserve the worktree only while a non-History open product conversation still has that same `WorkScope` attached

WHEN teardown or startup reconciliation evaluates a Phoenix-owned worktree
AND another live conversation still resolves to the same `WorkScope`
THE SYSTEM SHALL preserve the worktree

WHEN teardown or startup reconciliation evaluates a Phoenix-owned worktree
AND no other live conversation resolves to its `WorkScope`
THE SYSTEM SHALL remove the worktree when safe to do so
AND SHALL NOT create, delete, or rewrite a branch as part of that reclamation

WHEN startup reconciliation finds tracked or untracked changes or cannot determine worktree safety
THE SYSTEM SHALL retain the worktree for manual recovery
AND SHALL report why safe reclamation was skipped

**Rationale:** Worktree ownership belongs to the live `WorkScope`, not to stale mode data or branch ownership. Preserving or reclaiming the disposable workspace must never imply that Phoenix also owns a branch lifecycle.

---

### REQ-PROJ-016: Standalone Conversation Mode (Superseded)

**SUPERSEDED BY REQ-PROJ-018.** `Standalone` was the historical name for the chat-only non-Git conversation shape now represented by Direct mode. Retaining this REQ ID for traceability only; the old standalone/work/explore mode split is no longer normative.

---

### REQ-PROJ-017: Record Detached Default-Branch Provenance Without Mode Semantics

WHEN Phoenix provisions a Git-backed disposable worktree
THE SYSTEM SHALL first produce a typed provisioning result that is either resolved(commit, canonical default-branch identity, freshness) or unresolved(error)
AND SHALL record the repository's authoritative canonical default-branch identity and exact commit only in the resolved case
AND SHALL record the worktree path and any approved task metadata needed for truthful UI and provenance only in the resolved case
AND SHALL persist the unresolved failure reason without omitting it or encoding it as missing worktree metadata in the unresolved case
AND SHALL NOT require conversation ownership semantics to include a selected branch name, a dedicated branch-type discriminator, or a Phoenix-owned branch-lifecycle field

THE Direct mode SHALL carry no Git-backed worktree metadata

WHEN a conversation later closes
WHEN a Git-backed conversation later closes
THE SYSTEM SHALL release the same ProductConversation-scoped worktree according to the Close contract
AND SHALL NOT infer any branch-ownership mutation from the recorded starting provenance


**Rationale:** Users still need to know where a conversation started, but the new model records that as provenance rather than as an owned lifecycle mode with branch-selection semantics.

---

### REQ-PROJ-018: Direct Mode

Direct mode is the chat-only / non-worktree conversation shape.

WHEN a conversation is created in Direct mode
THE SYSTEM SHALL provide full tool access (bash, patch, all tools)
AND set the working directory to the target directory (not a Phoenix-owned worktree)
AND SHALL NOT include `propose_task`
AND NOT create worktrees, branches, or task files for the Direct conversation itself

THE SYSTEM SHALL visually distinguish Direct mode from Git-backed worktree conversations in the UI

WHEN a Direct conversation targets a Git repository
THE SYSTEM SHALL associate it with the project for discovery/configuration purposes
AND SHALL NOT treat that association as ownership of a Phoenix worktree lifecycle

**Rationale:** Direct mode remains useful for chat-only and ad hoc workflows, while Git-backed conversations use the disposable-worktree model. The important distinction is worktree ownership, not a branching lifecycle picker.

---

### REQ-PROJ-019: Conversation List Filtering and Explicit Close

WHEN the conversation list contains more than 20 conversations
THE SYSTEM SHALL provide filtering by lifecycle and conversation shape
AND provide filtering by project

WHEN the user applies a filter
THE SYSTEM SHALL show only conversations matching the selected filter
AND persist the filter selection across page navigation

THE SYSTEM SHALL NOT automatically archive or auto-close conversations based on age alone
AND SHALL keep closed conversations accessible through History after explicit Close

**Rationale:** Users need list controls as activity grows, but the unified lifecycle requires an explicit Close decision rather than age-based automatic archival.

---

### REQ-PROJ-020: Branch Discovery (Local, No Network)

WHEN the user opens the branch picker from a repository-operations surface
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

### REQ-PROJ-022: Default-Branch Materialization Uses One Bounded Refresh and Typed Fallback

WHEN Phoenix materializes the starting commit for a new Git-backed conversation, a Start-in-new-conversation spawn, or a follow-up conversation
THE SYSTEM SHALL resolve the repository's authoritative canonical default branch first
AND SHALL run at most one targeted refresh for that branch before provisioning the detached worktree
AND SHALL NOT run a blanket fetch, prune, or multi-branch refresh as part of ordinary provisioning

WHEN the targeted refresh succeeds
THE SYSTEM SHALL provision from the refreshed canonical default-branch tip at detached `HEAD`

WHEN the targeted refresh fails but a previously resolved local or remote-tracking ref for the canonical default branch still exists
THE SYSTEM SHALL return a typed resolved provisioning result carrying the exact commit, canonical default-branch identity, and `stale_cached` freshness
AND SHALL provision from that cached canonical-default ref at detached `HEAD`
AND SHALL surface that the starting point may be stale

WHEN no canonical default-branch commit can be resolved after the bounded refresh-or-fallback attempt
THE SYSTEM SHALL return a typed unresolved provisioning result carrying the failure reason
AND SHALL persist that unresolved provisioning failure on the still-Open conversation
AND SHALL NOT fabricate a `WorkScope`, worktree attachment, detached branch label, or fallback branch selection for that conversation
AND SHALL fail provisioning with a typed error instead of guessing from the repository's currently checked out branch or another arbitrary ref

THE SYSTEM SHALL preserve that one-branch refresh rule for provisioning even when repository-operations surfaces support broader branch discovery elsewhere

**Rationale:** Ordinary conversation provisioning needs one predictable, bounded starting-point rule. A single targeted refresh keeps creation current without turning provisioning into repository-wide synchronization, and the cached fallback preserves availability without silently pretending the starting point is fresh.

---

### REQ-PROJ-023: Reserved

Remote-aware commits-behind polling was removed when PR status became the StateBar's
branch health indicator.

---

### REQ-PROJ-024: Existing-Branch Work Is Repository State, Not Conversation-Creation Mode

WHEN a Git-backed conversation begins in its Phoenix-owned detached worktree
THE SYSTEM SHALL allow the agent or user to create, checkout, switch to, or stack branches later as repository operations inside that workspace
AND SHALL NOT require a dedicated branch-specific conversation type or branch-selection step at conversation creation time

WHEN the user wants Phoenix to iterate on an existing branch or PR branch
THE SYSTEM SHALL support that by operating within the conversation's disposable worktree after explicit checkout there
AND SHALL NOT treat the checked-out branch as conversation-owned lifecycle state

**Rationale:** Users still need the “fix my PR” workflow, but the current product truth expresses it as repository activity within one conversation-owned worktree rather than as a separate Branch-mode conversation type.

---

### REQ-PROJ-025: Reuse Live Conversation Context Instead of Branch-Creation Ceremony

WHEN repository operations reveal that another live conversation already owns the relevant Phoenix worktree context for continuing the same unit of work
THE SYSTEM SHALL prefer navigating or linking the user to that existing live conversation instead of silently duplicating ownership

WHEN orphaned worktrees exist on disk without a live owning conversation
THE SYSTEM SHALL surface truthful recovery or cleanup guidance before reuse

**Rationale:** The user still benefits from avoiding duplicate live work contexts, but the guard now centers on live conversation/worktree ownership rather than on a Branch-mode picker or branch-name ownership rule.

---

### Close lifecycle for Git-backed conversations (specified in work-lifecycle)

The explicit Close flow for Git-backed conversations — worktree/resource release, PR-aware guidance, and the no-ref-mutation rule — is specified by the **work-lifecycle** spec (REQ-WL-001 through REQ-WL-003).

---

### REQ-PROJ-028: Worktree Provisioning Happens at Git-Backed Conversation Creation

WHEN a Git-backed conversation is created
THE SYSTEM SHALL create the disposable worktree immediately
AND SHALL start it from the repository's canonical default-branch commit at detached `HEAD`
AND the agent SHALL read from that worktree rather than from the main checkout

WHEN task approval later grants write authority
THE SYSTEM SHALL continue using the same worktree rather than creating a second workspace

WHEN a Git-backed conversation closes without ever receiving write authority
THE SYSTEM SHALL still clean up the disposable worktree through the ordinary Close/reconciliation path

**Rationale:** Planning and implementation should observe the same isolated filesystem view. Provisioning the detached worktree at conversation creation preserves that continuity without introducing Managed-mode or task-branch semantics.

---

### REQ-PROJ-029: No Branch Picker in Ordinary Conversation Creation

WHEN the directory is a Git repository
THE SYSTEM SHALL NOT show a Branch-mode or Managed-mode picker for ordinary conversation creation
AND SHALL NOT require the user to select a base branch or destination branch before starting the conversation

Branch discovery and checkout capabilities MAY still be reused later inside the disposable worktree when the user or agent needs to inspect or switch repository state.

**Rationale:** The unified creation flow optimizes for starting work quickly. Branch choice remains a repository operation available later, not a prerequisite lifecycle decision.


### REQ-PROJ-033: Propose a Derived Task from an Already-Active Conversation

WHILE a Git-backed conversation already has write authority
THE SYSTEM SHALL provide `propose_task` as the same blocking approval tool used in planning/read-only conversations

WHEN the agent calls `propose_task` with a markdown file inside its allowed workspace
THE SYSTEM SHALL intercept it at the LlmResponse handler
AND require it to be the only tool call in the response
AND validate the file by the same taskmd/plain-brief rules as REQ-PROJ-003
AND read and persist the proposal as the blocking approval artifact defined by REQ-PROJ-003/004
AND transition the conversation to AwaitingTaskApproval rather than leaving it running

WHEN `propose_task` is called from a planning/read-only phase
THE SYSTEM SHALL keep the same blocking review behavior

WHEN `propose_task` is called in Direct mode
THE SYSTEM SHALL NOT provide the tool

WHEN `propose_task` is called by a sub-agent
THE SYSTEM SHALL reject the call

**Rationale:** Approved product behavior uses one blocking human-review checkpoint for all Git-backed `propose_task` calls. Write authority changes what approval can grant or preserve; it does not create a second nonblocking proposal meaning.

---

### REQ-PROJ-034: Reserved

Derived-conversation review and spawn semantics were removed when `propose_task` was unified into one blocking approval flow. This REQ id remains reserved for traceability only.

---

### REQ-PROJ-035: Reserved

Derived-proposal control-plane state and extra decoupling contracts were removed with the nonblocking derived-proposal model. Continuation topology and approved-task source relations are specified elsewhere.

---

### REQ-PROJ-036: propose_task Availability by Conversation Capability

THE `propose_task` tool SHALL be available as follows:

| Conversation capability shape | `propose_task` behavior |
|-------------------------------|-------------------------|
| Git-backed planning/read-only conversation | Blocking review path (REQ-PROJ-003 / REQ-PROJ-004) |
| Git-backed conversation with write authority | Blocking review path (REQ-PROJ-003 / REQ-PROJ-004) |
| Any Direct conversation | Not provided |
| Any sub-agent | Not provided |

**Rationale:** Availability depends on whether the host conversation itself is Git-backed and therefore eligible for the one blocking approval flow. Chat-only/direct conversations and sub-agents remain excluded, even if a chat-only working directory happens to sit inside a repository.

---

### REQ-PROJ-037: Reserved

Fresh refinement-conversation behavior was removed with the nonblocking derived-proposal model. Request changes remains part of the ordinary blocking approval loop in REQ-PROJ-004.

---

### REQ-PROJ-038: Show the Live Worktree Checkout in Diff Review

WHEN the system presents a conversation worktree diff
THE SYSTEM SHALL identify the worktree's live Git checkout at the time the diff is read
AND distinguish a named branch, detached HEAD, unborn branch, and unavailable observation
AND SHALL NOT substitute the branch recorded when the conversation was created for the live checkout

WHEN the live checkout is a named branch
THE SYSTEM SHALL show the branch name
AND, when a configured upstream or locally cached matching remote-tracking branch exists, show that ref and the checkout's ahead and behind counts relative to it
AND distinguish a configured upstream from a matching remote-tracking branch that is not configured as the upstream
AND, when neither is locally known, state that no remote branch is known from the last fetched state

WHEN the live checkout is detached
THE SYSTEM SHALL present detached HEAD as a valid checkout identified by its commit object ID
AND MAY show bounded, deterministic local or remote-tracking refs that resolve exactly to that commit
AND SHALL describe those refs as pointing to the commit rather than as the checkout's provenance

THE SYSTEM SHALL derive remote relationship data only from local Git refs while opening the diff
AND SHALL NOT fetch or otherwise perform network I/O as part of diff review
AND SHALL keep the worktree checkout's remote relationship distinct from the base ref used to calculate the workspace diff

---

## WorkScope Resource Ownership

### REQ-PROJ-WS-001: WorkScope as Resource Owner

Work-affine resources SHOULD be owned by the opaque persisted `work_scope_id`. Resource ownership MUST NOT be derived from conversation ids, transcript-row ids, sub-agent ids, working directories, or worktree paths. Product conversations, transcript rows, and subordinate execution conversations MAY have a `WorkScope` attached, but attachment is not ownership. When continuation creates a new execution row within the same product conversation, the conversation keeps the same attached `WorkScope`; Phoenix SHALL NOT describe that step as transferring WorkScope ownership from one row to another or as electing a latest row owner. Attachments and other work-affine execution resources share that same WorkScope ownership model. Distinct product conversations remain isolated by distinct WorkScope identities even when their environments use the same path.

