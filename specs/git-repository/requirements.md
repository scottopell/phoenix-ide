# Git Repository: Hidden Repository Identity and Worktree Provenance

## User Story

As a Phoenix user, I need Git-backed conversations to start from the right local repository state and recover safely after restarts, without Phoenix turning the repository itself into a user-facing product object with ownership or workflow controls.

## Scope

This spec governs Phoenix's hidden `GitRepository` domain record: Phoenix-local repository identity, mutable locator observation, default-branch observation and provenance, singular repository attachment to work-bearing scopes, restart repair evidence, repository-backed worktree registry, and repository-observation surfaces that read branch facts without making repository lifecycle user-facing.

It does **not** own:

- conversation creation acceptance, retries, and worker claims — `specs/conversation-creation/`;
- conversation lifecycle, task approval state, or tool execution authority — `specs/bedrock/`;
- user-facing Close conversation retirement — `specs/work-lifecycle/`;
- product surfaces that group, switch, title, or manage repositories as first-class user-owned objects.

## Requirements

### REQ-GITREP-001: GitRepository Identity Is Hidden, Opaque, and Phoenix-Local

WHEN Phoenix recognizes local Git state that should participate in Git-backed provisioning, observation, recovery, or locking
THE SYSTEM SHALL model that repository as one hidden `GitRepository` with a Phoenix-local opaque identity
AND SHALL keep that opaque identity distinct from every filesystem path, Git remote URL, branch name, slug, inferred display name, or user-facing label

THE SYSTEM SHALL treat separate clones as distinct hidden `GitRepository` identities even when they point at the same remotes, contain the same commit graph, or share a directory name

THE SYSTEM SHALL preserve distinct deterministic Project-seeded `GitRepository` identities before and through repository-authority activation
AND SHALL NOT merge existing opaque identities unless an owning normative requirement defines one atomic operation that selects a surviving identity, rewrites every reference losslessly, and retires each losing identity

THE SYSTEM SHALL NOT derive hidden `GitRepository` identity from a canonical path string, remote URL, repository slug, host/name pair, or guessed continuity across restarts
AND SHALL NOT infer that two observations are "the same repository" solely because a path reappeared, a slug matched, or a remote looked similar

WHEN an operator restores or replaces the Phoenix database
THE SYSTEM SHALL require Phoenix to be stopped before the database files change
AND SHALL NOT claim support for replacing the database beneath a running Phoenix process

**Rationale:** Phoenix needs durable local repository identity for provisioning and repair, but users do not need Phoenix to elevate that hidden identity into a user-facing repository object or to guess sameness from mutable environmental strings.

---

### REQ-GITREP-002: GitRepository Locators Are Mutable Observations with Explicit Status

FOR each hidden `GitRepository`
THE SYSTEM SHALL observe at least these mutable locators independently:
- the Git common directory locator
- the management-root locator Phoenix uses for local repository management

EACH locator observation SHALL carry one explicit status of `present`, `missing`, or `inaccessible`
AND SHALL distinguish a missing path from a path that exists but cannot currently be read or traversed

WHEN a locator path changes while retained repository identity still proves the same hidden `GitRepository`
THE SYSTEM SHALL update the locator observation without minting a replacement hidden identity

WHEN locator evidence is incomplete or conflicting
THE SYSTEM SHALL surface that uncertainty as explicit observation state
AND SHALL NOT guess continuity from the latest path string alone

**Rationale:** Repository-adjacent paths move, disappear, or become unreadable. Phoenix needs to preserve hidden identity while telling the truth about what it can currently observe.

---

### REQ-GITREP-003: Default Branch Observation Is Optional, Provenanced, and Never Fabricated

WHEN Phoenix has evidence for a hidden `GitRepository`'s canonical default branch
THE SYSTEM SHALL represent that observation as an optional value carrying:
- the observed branch identity
- whether origin-backed canonical evidence is present
- the provenance, one of `remote_head_cache` or `user_selected`
- the exact `observed_at` time

THE SYSTEM SHALL persist new locator and default-branch observation times as non-negative Unix microseconds

WHEN Phoenix lacks authoritative evidence for a canonical default branch
THE SYSTEM SHALL represent that state as unresolved rather than fabricating a branch name

THE SYSTEM SHALL treat provenance `remote_head_cache` as admissible canonical evidence of the remote's canonical default branch
AND SHALL treat that case as origin-backed evidence for creation-pin selection

THE SYSTEM SHALL treat provenance `user_selected` as observation only
AND SHALL NOT treat it by itself as proof of the remote's canonical default branch
AND SHALL require separate canonical corroboration before any consumer that promises canonical-default behavior may rely on that branch identity as canonical

THE SYSTEM SHALL NEVER fabricate `main` or any other branch name as a synthetic default

**Rationale:** Default-branch knowledge is an observation with provenance, not a permanent fact Phoenix can safely guess.

---

### REQ-GITREP-004: WorkScope Repository Attachment Is Singular, Nullable, and ProductConversation-Derived

WHEN Phoenix attaches repository-backed execution context to active work
THE SYSTEM SHALL model a singular nullable `WorkScope.repository` attachment
AND SHALL allow each `WorkScope` to reference at most one hidden `GitRepository`
AND SHALL allow chat-only or otherwise non-repository work to carry no repository attachment

THE SYSTEM SHALL derive ProductConversation repository context from its attached `WorkScope`
AND SHALL NOT create a second independent writable ProductConversation-to-repository ownership authority

WHEN provisioning, retry, restart repair, or unresolved canonical-default evidence needs to name a hidden `GitRepository` before any `WorkScope` exists
THE SYSTEM SHALL allow typed pre-scope provisioning evidence to reference that repository directly
AND SHALL NOT fabricate a `WorkScope` solely to carry repository identity

WHEN Phoenix backfills hidden `GitRepository` records from retained legacy `Project` repository state
THE SYSTEM SHALL map one retained legacy Project identity to exactly one hidden `GitRepository` identity by a deterministic replay-stable rule
AND MAY seed the hidden `GitRepository` identity from the corresponding retained legacy Project identity only as that backfill rule
AND SHALL NOT treat that migration convenience as runtime authority to substitute between the two identity domains

WHEN one `WorkScope` resolves to conflicting retained legacy Project assignments
THE SYSTEM SHALL fail that migration rather than selecting one assignment heuristically

FOR each repository fact that may be carried by both a legacy `Project`-named representation and the hidden `GitRepository` model
THE SYSTEM SHALL preserve exactly one writable authority at a time
AND SHALL treat every other representation as read-only compatibility output or read-only retained data

WHEN hidden `GitRepository` facts become authoritative
THE SYSTEM SHALL activate them only through the offline authority transition in REQ-GITREP-009

**Rationale:** Repository attachment belongs to the work-bearing scope, while aggregate context derives from that scope. Pre-scope evidence still needs to name the repository truthfully without inventing work ownership. A deterministic single-authority transition prevents migration convenience from becoming permanent parallel authority.

---

### REQ-GITREP-005: Continuations Retain Repository Attachment; Follow-Up Work Starts Fresh but May Reuse the Same Repository

WHEN context continuation creates a new execution row inside the same ProductConversation
THE SYSTEM SHALL retain the same attached `WorkScope`
AND SHALL retain that same `WorkScope`'s hidden `GitRepository` attachment and history
AND SHALL NOT describe continuation as transferring repository ownership between transcript rows

WHEN follow-up work starts a fresh ProductConversation
THE SYSTEM SHALL attach a fresh `WorkScope` appropriate to that follow-up
AND MAY attach that fresh scope to the same hidden `GitRepository` when the new work targets the same local repository
AND SHALL treat that as a fresh scope binding rather than as scope reuse or path-continuity guessing

**Rationale:** Continuation preserves one work context; follow-up creates a new one. Reusing hidden repository identity across fresh work is legitimate, but reusing the old scope is not.

---

### REQ-GITREP-006: Restart Repair Evidence Is Immutable, Typed, and Identity-Bound

WHEN Phoenix restarts and retained repository-backed ownership evidence no longer matches the currently observable filesystem state
THE SYSTEM SHALL record immutable typed restart repair evidence bound to:
- the owning `ProductConversation`
- the attached `WorkScope`
- the hidden `GitRepository`
- the retained worktree identity
- the observed worktree path
- the retained worktree fingerprint
- one exact observation kind: `missing` or `inaccessible`
- one monotonically increasing repair-observation generation within the owning ProductConversation and WorkScope
- the exact observation time

FOR one `ProductConversation`, attached `WorkScope`, and repair-observation generation
THE SYSTEM SHALL retain at most one restart repair evidence record

WHEN later repair, Close, or retry flows select retained restart repair evidence
THE SYSTEM SHALL select the one complete row with the highest observation generation for the same `ProductConversation` and attached `WorkScope`
AND SHALL require its hidden `GitRepository`, retained worktree identity/fingerprint, observation kind, and observation time to be complete
AND SHALL fail closed rather than guessing when the highest generation is duplicated, conflicting, incomplete, or bound to a different scope

THE SYSTEM SHALL treat that repair evidence as an observation record rather than as proof that a replacement path or replacement worktree is equivalent
AND SHALL NOT guess continuity from path reuse, directory name reuse, or remote similarity

WHEN later repair, Close, or retry flows consume restart repair evidence
THE SYSTEM SHALL preserve the original evidence immutably and record any later adoption or conflict as additional typed facts rather than rewriting the source observation

**Rationale:** Restart repair needs durable evidence that later flows can adopt or reject exactly. Rewriting the source observation would erase the boundary between what Phoenix saw and what later logic decided.

---

### REQ-PROJ-015: GitRepository Worktree Registry

WHEN a Phoenix-owned disposable worktree exists for a ProductConversation
THE SYSTEM SHALL register enough data to report that stable ProductConversation identity, its topology-derived latest parent transcript row, worktree path, attached `WorkScope`, and singular attached `GitRepository`

WHEN the server starts
THE SYSTEM SHALL reconcile the registry against worktrees on disk
AND clean up orphaned registry entries
AND report worktrees that exist on disk but have no registry entry

WHEN a parent transcript row is in context-exhausted state and has no continuation successor through `continued_in_conv_id`
THE SYSTEM SHALL NOT treat its worktree as orphaned during reconciliation

WHEN a transcript row has transferred execution through `continued_in_conv_id`
THE SYSTEM SHALL treat that row as a historical transcript segment rather than as an independent `WorkScope` authority
AND SHALL derive the latest execution row from continuation topology rather than storing a second ownership authority
AND SHALL keep the same stable ProductConversation-scoped `WorkScope` identity, the same singular `GitRepository` association, and the same filesystem worktree path across that continuation
AND SHALL preserve the worktree only while an ordinary Open ProductConversation still has that same `WorkScope` attached

WHEN teardown or startup reconciliation evaluates a Phoenix-owned worktree
AND a distinct ordinary Open ProductConversation still resolves to the same `WorkScope`, or persisted transcript membership and attachment records cannot prove whether such a distinct Open aggregate exists
THE SYSTEM SHALL preserve the worktree

WHEN teardown or startup reconciliation evaluates a Phoenix-owned worktree
AND no distinct ordinary Open ProductConversation resolves to its `WorkScope`
AND every persisted transcript row and attachment record for that `WorkScope` resolves to a known ProductConversation
THE SYSTEM SHALL remove the worktree when safe to do so
AND SHALL NOT create, delete, or rewrite a branch as part of that reclamation

WHEN startup reconciliation finds tracked or untracked changes or cannot determine worktree safety
THE SYSTEM SHALL retain the worktree for manual recovery
AND SHALL report why safe reclamation was skipped

**Rationale:** Worktree ownership belongs to the live `WorkScope`, not to stale mode data or branch ownership. The hidden `GitRepository` record helps Phoenix reason about repository-backed resources, but preserving or reclaiming the disposable workspace must never imply that Phoenix also owns a branch lifecycle or a user-facing repository object.

---

### REQ-GITREP-007: Hidden Repository Identity Survives Conversation Deletion When Phoenix Still Needs Repository Truth

WHEN a ProductConversation is deleted
THE SYSTEM SHALL remove only the conversation-owned aggregate and scope attachments that deletion owns
AND SHALL preserve the hidden `GitRepository` whenever Phoenix still needs it for orphan recovery, default-branch observation history, restart repair evidence, or repository-scoped mutation locking

THE SYSTEM SHALL NOT require a surviving user-visible conversation, task, branch, or pull request association in order to retain hidden repository identity

WHEN no surviving WorkScope attachment, provisioning/repair evidence, orphan resource, default-branch observation needed by a consumer, or repository-scoped mutation lock refers to a hidden `GitRepository`
THE SYSTEM MAY retire that hidden repository and its locator/default-branch observations
AND SHALL NOT retain it solely to populate a user-visible catalog, favorite, recent-location suggestion, or grouping surface

**Rationale:** Repository identity outlives one conversation when Phoenix still needs it to reason safely about remaining local repository facts.

---

### REQ-GITREP-008: Hidden GitRepository Owns No User-Facing Lifecycle or Workflow Surface

THE hidden `GitRepository` SHALL NOT own a user-facing title, grouping surface, lifecycle, task inventory, branch inventory, pull-request ownership, or conversation list surface
AND SHALL NOT become the user-facing owner of task approval, Close, Delete, branch mutation, or PR workflow semantics

Repository-observation surfaces MAY read branch or remote facts keyed by hidden `GitRepository`
BUT those surfaces SHALL present repository state as observed local context for conversations rather than as a Phoenix-managed repository object

**Rationale:** Phoenix needs repository facts, not a new repository product with its own lifecycle and workflow contract.

Repository locator and default-branch observations are current per repository and observation kind: a newer observation supersedes the older current observation rather than creating an unbounded user history. Immutable restart-repair evidence is separate because Close adoption requires its exact source observation.

---

### REQ-GITREP-009: Repository Authority Activation Is Consumer-Triggered and Offline

THE SYSTEM SHALL keep legacy `Project` as the sole writable repository authority while no product capability requires hidden `GitRepository` authority
AND SHALL NOT activate hidden `GitRepository` authority merely because its dormant Foundation data is available

WHEN an owning normative requirement for a ProductConversation or destructive Close capability explicitly requires hidden `GitRepository` authority generation `2` for its correctness contract
THE SYSTEM SHALL require an activation mandate that identifies that exact consumer capability and owning normative requirement
AND SHALL make that mandate an exact input to the one activation operation
AND SHALL NOT treat a generic reference to `GitRepository`, `WorkScope`, ProductConversation, Close, or a broad consumer category as activation authority

THE SYSTEM SHALL perform repository-authority activation only as an offline maintenance operation
AND SHALL require Phoenix to be stopped before the operation begins
AND SHALL acquire exclusive access to the target SQLite database before capturing activation recovery state
AND SHALL capture and verify a recoverable snapshot of the exact database state to be activated while exclusive access is held and before any activation mutation
AND SHALL pair that exact pre-activation snapshot with the Project-authority binary that can restore its operation
AND SHALL validate the dormant GitRepository Foundation before changing authority

THE SYSTEM SHALL preserve each deterministic Project-seeded `GitRepository` identity and its existing WorkScope attachments during activation
AND SHALL NOT merge identities or perform linked-worktree convergence as part of activation

THE SYSTEM SHALL change repository authority and its persisted authority generation from Project generation `1` to GitRepository generation `2` in one SQLite transaction
AND SHALL migrate or structurally quarantine every repository-sensitive reader and writer before generation `2` becomes authoritative
AND SHALL leave every surviving Project-shaped repository value as read-only compatibility output or retained data that cannot feed a correctness-sensitive decision
AND SHALL roll back that transaction wholly if validation, reference updates, reader/writer migration, or authority activation fails before commit

AFTER the authority transaction commits
THE SYSTEM SHALL complete the one activation operation with the exact consumer mandate, exact pre-activation snapshot, exact committed generation-2 database state, and exact GitRepository-authority binary artifact as its bound inputs and outputs
AND SHALL NOT require a second persisted representation of those external recovery artifacts inside the activated database
AND SHALL persist repository authority generation `2` as part of that same transaction
AND SHALL allow only Phoenix binaries that require GitRepository authority generation `2` to open the database for normal operation
AND SHALL make binaries that require Project authority generation `1` fail closed
AND SHALL require recovery to roll forward with generation `2` or restore the exact pre-activation snapshot with its paired Project-authority binary under the offline contract in `specs/compatibility/requirements.md`

THE SYSTEM SHALL NOT support live in-process repository-authority activation, runtime-wide drain as an activation protocol, live Git observation during activation, production authorization derived from a source census, or automatic authority rollback

**Rationale:** Authority activation exists to serve a product capability, not as an independent infrastructure milestone. Stopping Phoenix turns old-writer exclusion into an operational precondition and keeps the authority transition bounded to one database transaction without merging distinct seeded identities.

---

### REQ-PROJ-020: Branch Discovery (Local, No Network)

WHEN the user opens the branch picker from a repository-observation surface
THE SYSTEM SHALL list local branches sorted by most-recent commit date (descending)
AND detect the remote's default branch via cached symbolic ref (no network call)

WHEN a local branch has a remote tracking ref (e.g. `origin/<name>`)
THE SYSTEM SHALL compare the local ref against the remote tracking ref
AND display how many commits the local branch is behind the remote tracking ref
AND this comparison SHALL use only local data (no fetch)

WHEN the remote default branch is detectable
THE SYSTEM SHALL include it in the response even if it is not checked out locally

THE SYSTEM SHALL NOT run `git fetch` or any network operation during the no-query branch listing path

**Rationale:** The no-query path must be instant regardless of repo size or network conditions. Local branches sorted by recency surface the branches the user is actively working on, pushing stale branches down. Behind-remote counts use the local remote-tracking ref (last fetch), which may be stale but provides a useful signal at zero cost.

---

### REQ-PROJ-021: Remote Branch Search (Network, On-Demand)

WHEN the user types a search query in the branch picker
THE SYSTEM SHALL run `git ls-remote --heads --tags origin` to list remote refs
AND filter the results server-side by case-insensitive substring match on the query
AND return matching branches and version-like tags

THE SYSTEM SHALL cache `git ls-remote` results keyed by canonical GitRepository identity
AND the cache TTL SHALL be at least 5 minutes
AND subsequent searches within the TTL SHALL filter the cached result (no network)

WHEN the search returns results
THE SYSTEM SHALL distinguish remote-only branches from branches that also exist locally

THE SYSTEM SHALL NOT download git objects during search (`ls-remote` transfers only ref names and SHAs)

**Rationale:** `git ls-remote` lists refs without downloading pack data, making it fast even on large repositories. Caching the full ref list means rapid successive keystrokes filter locally after the first network call.

---

### REQ-PROJ-024: Existing-Branch Work Is Repository State, Not Conversation-Creation Mode

WHEN a Git-backed conversation begins in its Phoenix-owned detached worktree
THE SYSTEM SHALL allow the agent or user to create, checkout, switch to, or stack branches later as repository operations inside that workspace
AND SHALL NOT require a dedicated branch-specific conversation type or branch-selection step at conversation creation time

WHEN the user wants Phoenix to iterate on an existing branch or PR branch
THE SYSTEM SHALL support that by operating within the conversation's disposable worktree after explicit checkout there
AND SHALL NOT treat the checked-out branch as conversation-owned lifecycle state

**Rationale:** Users still need the “fix my PR” workflow, but Phoenix expresses it as repository activity within one conversation-owned worktree rather than as a separate branch-mode conversation type.

---

### REQ-PROJ-025: Reuse Live Conversation Context Instead of Branch-Creation Ceremony

WHEN repository operations reveal that another live conversation already owns the relevant Phoenix worktree context for continuing the same unit of work
THE SYSTEM SHALL prefer navigating or linking the user to that existing live conversation instead of silently duplicating ownership

WHEN orphaned worktrees exist on disk without a live owning conversation
THE SYSTEM SHALL surface truthful recovery or cleanup guidance before reuse

**Rationale:** The user still benefits from avoiding duplicate live work contexts, but the guard centers on live conversation/worktree ownership rather than on a branch-name ownership rule.
