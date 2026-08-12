# Durable Conversation Creation

## User Story

As a developer starting a Phoenix conversation, I need the conversation shell to appear immediately and provisioning to complete exactly once despite retries, process failure, or concurrent workers, so that creation never duplicates Git resources, corrupts conversation state, or leaves me without a recovery action.

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

### REQ-PROJ-001: Open a Git Repository as a Hidden Conversation Repository

WHEN the user creates a new conversation by providing a directory path
THE SYSTEM SHALL detect whether the directory is inside a git repository
AND create the conversation as an Open Git-backed conversation without asking the user to choose Explore, Work, Managed, or Branch lifecycle modes
AND resolve any existing hidden `GitRepository` into typed pre-scope provisioning evidence
AND provision one Phoenix-owned disposable worktree using the repository's canonical default-branch starting point
AND, only after provisioning resolves the starting commit and creates the worktree, create one `WorkScope` attached to that hidden `GitRepository`
AND derive the ProductConversation's repository context through that attached `WorkScope`
AND start that worktree at detached `HEAD`

WHEN the directory is NOT inside a git repository
THE SYSTEM SHALL create the conversation in Direct mode (REQ-PROJ-018)
AND leave any `WorkScope.repository` attachment empty

**Rationale:** Users think in terms of codebases, not raw directories. Phoenix may normalize that codebase to a hidden GitRepository record, but conversation creation remains a user-facing vertical slice that starts from one disposable Phoenix-owned workspace rather than asking the user to pick repository lifecycle variants up front.

---

### REQ-PROJ-002: Git-Backed Creation Has No Mode or Branch Picker

WHEN the user creates a new conversation for a Git repository
THE SYSTEM SHALL NOT ask the user to choose Managed, Work, Branch, Explore, or equivalent lifecycle modes
AND SHALL NOT ask the user to choose a starting branch for ordinary conversation creation
AND SHALL start from the repository's canonical default-branch commit in the detached disposable worktree defined by REQ-PROJ-001

WHEN a conversation is created for a non-Git directory
THE SYSTEM SHALL initialize it in Direct mode by default
AND provide full tool access (bash, patch, all tools)

**Rationale:** Unified creation removes branch- and mode-selection ceremony from ordinary conversation creation. Git-backed conversations begin in the same Phoenix-owned disposable workspace model; chat-only conversations remain Direct because there is no repository-backed worktree to provision.

---

### REQ-PROJ-005: Worktree Paths Are Unique by Construction

WHEN a worktree is created for a Git-backed ProductConversation
THE SYSTEM SHALL place it at `.phoenix/worktrees/{product-conversation-id}/` relative to the
repository root
AND ensure `.phoenix/worktrees/` is listed in the repository's `.gitignore`

WHEN two conversations create worktrees for the same GitRepository simultaneously
THE SYSTEM SHALL create separate directories for each
AND the directories SHALL share no file paths

**Rationale:** Deriving the worktree path from the ProductConversation ID makes collisions
structurally impossible without a lock registry. Each conversation gets a fully isolated
physical directory. Multiple work-capable conversations on the same repository can coexist
because their code changes never share a directory.

---

### REQ-PROJ-005A: Worktree Build Cache Pre-Warm Is Best-Effort

WHEN Phoenix creates a new conversation worktree under `.phoenix/worktrees/{product-conversation-id}/`
THE SYSTEM MAY pre-warm allowlisted repository-local build cache directories from the repository root
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

### REQ-PROJ-017: Record Detached Default-Branch Provenance Without Mode Semantics

WHEN Phoenix provisions a Git-backed disposable worktree
THE SYSTEM SHALL first produce a typed provisioning result that is either resolved(commit, canonical default-branch identity, freshness) or unresolved(error)
AND SHALL record the repository's authoritative canonical default-branch identity and exact commit only in the resolved case
AND SHALL record the worktree path and any approved task metadata needed for truthful UI and provenance only in the resolved case
AND SHALL persist the unresolved failure reason without omitting it or encoding it as missing worktree metadata in the unresolved case
AND SHALL NOT require conversation ownership semantics to include a selected branch name, a dedicated branch-type discriminator, or a Phoenix-owned branch-lifecycle field

THE Direct mode SHALL carry no Git-backed worktree metadata

WHEN a Git-backed conversation later closes
THE SYSTEM SHALL release the same ProductConversation-scoped worktree according to the Close contract
AND SHALL NOT infer any branch-ownership mutation from the recorded starting provenance

**Rationale:** Users still need to know where a conversation started, but the model records that as provenance rather than as an owned lifecycle mode with branch-selection semantics.

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
AND SHALL bind that unresolved provisioning failure directly to the target `ProductConversation` and concrete target transcript conversation
AND, when local Git identity evidence already proves an existing `GitRepository`, SHALL bind the typed pre-scope failure evidence to that repository
AND, when repository identity itself remains unresolved, SHALL preserve that uncertainty without a repository reference
AND SHALL NOT create or fabricate a `GitRepository` merely to persist unresolved canonical-default failure evidence

THE SYSTEM SHALL preserve that one-branch refresh rule for provisioning even when repository-observation surfaces support broader branch discovery elsewhere

**Rationale:** Ordinary conversation provisioning needs one predictable, bounded starting-point rule. A single targeted refresh keeps creation current without turning provisioning into repository-wide synchronization, and the cached fallback preserves availability without silently pretending the starting point is fresh.

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

**Rationale:** Planning and implementation should observe the same isolated filesystem view. Provisioning the detached worktree at conversation creation preserves that continuity without introducing branch-selection semantics.

---

### REQ-PROJ-029: No Branch Picker in Ordinary Conversation Creation

WHEN the directory is a Git repository
THE SYSTEM SHALL NOT show a branch picker or branch-specific mode picker for ordinary conversation creation
AND SHALL NOT require the user to select a base branch or destination branch before starting the conversation

Branch discovery and checkout capabilities MAY still be reused later inside the disposable worktree when the user or agent needs to inspect or switch repository state.

**Rationale:** The unified creation flow optimizes for starting work quickly. Branch choice remains a repository operation available later, not a prerequisite lifecycle decision.

---

### REQ-CCR-001: Durable Acceptance

WHEN a structurally valid creation request is accepted
THE SYSTEM SHALL atomically persist a navigable conversation shell and its complete creation intent before acknowledging acceptance
AND filesystem, Git, reference expansion, attachment finalization, and runtime bootstrap SHALL occur after acceptance

A created ordinary ProductConversation SHALL begin in Open state. The lifecycle-free Coordinator SHALL preserve its Coordinator identity when its singleton aggregate is created on demand and SHALL NOT receive Open/History state. Moving an ordinary ProductConversation to History is the Close action owned by the conversation lifecycle; provisioning, retries, cancellation, and deletion SHALL NOT invent a parallel lifecycle label.

### REQ-CCR-002: Exclusive Provisioning Authority

WHEN a worker begins or resumes provisioning
THE SYSTEM SHALL grant that worker one time-bounded creation claim with a monotonically increasing generation
AND every authoritative provisioning update SHALL require the current claim and generation

WHEN a worker reports a result after losing its claim
THE SYSTEM SHALL reject the result without changing conversation, job, message, or resource ownership state

### REQ-CCR-003: Crash Reconciliation

WHEN a creation claim expires during an external operation
THE SYSTEM SHALL fence that generation from further authoritative updates
AND a replacement worker SHALL inspect durable reservations and observed external state before resuming, adopting, conflicting, or cleaning the operation

THE SYSTEM SHALL NOT infer that an external operation failed merely because its acknowledgement was not persisted

### REQ-CCR-004: Bounded Retry

WHEN provisioning fails for a transient reason
THE SYSTEM SHALL durably schedule no more than four total attempts using delays of 2 seconds, 10 seconds, and 30 seconds between subsequent attempts

WHEN the retry budget is exhausted
THE SYSTEM SHALL preserve a failed conversation record with the original creation intent and final error

WHEN provisioning fails for a permanent reason
THE SYSTEM SHALL preserve the failed conversation without an automatic retry

### REQ-CCR-005: Repository Mutation Serialization

WHEN creation mutates Git refs or worktrees
THE SYSTEM SHALL serialize mutation by canonical repository identity across live Phoenix processes
AND cleanup SHALL remove only resources whose durable ownership still belongs to the cleanup operation

### REQ-CCR-006: First-Class Runtime Bootstrap

WHEN provisioning submits an initial message
THE SYSTEM SHALL transition directly from provisioning into the normal conversation lifecycle through an idempotent bootstrap operation
AND the system SHALL NOT temporarily persist an idle conversation solely to initialize a runtime

### REQ-CCR-007: Cancellation

WHEN a user cancels an accepted, claimed, or retry-scheduled creation
THE SYSTEM SHALL immediately revoke the active generation
AND SHALL preserve a visible cancelled conversation with its original creation intent
AND SHALL reconcile owned resources asynchronously

WHEN reconciliation completes
THE SYSTEM SHALL offer Start over and Delete for the cancelled record

### REQ-CCR-008: Deletion During Creation

WHEN a user deletes a non-ready creation record
THE SYSTEM SHALL immediately omit it from normal user-facing conversation surfaces
AND SHALL retain an internal deletion-pending record until owned resources are safely reconciled
AND SHALL physically delete the record only after reconciliation succeeds

### REQ-CCR-009: Durable Scheduling

WHEN an accepted job, retry deadline, or expired lease becomes eligible
THE SYSTEM SHALL make it discoverable without requiring another conversation request or process restart

### REQ-CCR-010: Deterministic Verification

WHEN the creation protocol is verified
THE SYSTEM SHALL exercise generated operation schedules containing concurrent claims, lease expiry, late results, crashes, retries, cancellation, deletion, and ambiguous external-effect completion
AND SHALL check lifecycle and ownership invariants after every generated operation
AND SHALL retain minimized failing schedules as deterministic regressions
