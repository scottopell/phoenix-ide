# Durable Conversation Creation

## User Story

As a developer starting a Phoenix conversation, I need Phoenix to accept one durable creation job per request, publish the conversation only when its starting state is whole, and recover safely across retries or crashes, so that creation never duplicates Git resources, mutates the wrong repository state, or leaves me with an ambiguous starting workspace.

## Requirements

### REQ-CCR-011: Ordinary Creation Model Default

WHEN ordinary conversation creation omits a model selection
THE SYSTEM SHALL use the available registry product default independently of repository presence or conversation workspace mode
AND SHALL NOT substitute an auxiliary cheap-model choice

WHEN creation supplies an explicit supported model selection
THE SYSTEM SHALL preserve that selection independently of the product default
AND SHALL preserve supported explicit reasoning-effort and request-speed selections

WHEN effort or request speed is omitted
THE SYSTEM SHALL use the selected model's native effort semantics and Standard request speed

**Rationale:** Workspace provisioning is not a model preference. Ordinary coding conversations use the product default, while explicitly cheap auxiliary tasks and specialized worker selections retain their own selection contracts.

---

### REQ-PROJ-000: Conversation Working Directory Root Floor

WHEN a conversation is resumed with a working directory or creation names an existing directory
THE SYSTEM SHALL resolve symlinks and parent-directory traversal before accepting the path
AND reject the working directory if it resolves to the filesystem root
AND reject the working directory if the system cannot prove it is an existing directory

WHEN ordinary ProductConversation creation names a missing directory beneath the server home directory or `/tmp`
THE SYSTEM SHALL accept only an absolute path with no parent traversal whose nearest existing ancestor resolves beneath an allowed root
AND SHALL persist the normalized directory intent without creating it during request validation or durable acceptance
AND the claimed creation worker SHALL revalidate the allowed canonical ancestor and idempotently create and canonicalize each missing path segment after durable acceptance before repository detection or publication
AND a terminal creation outcome SHALL preserve those selected working-directory segments because their existence does not prove exclusive Phoenix ownership
AND automated cleanup SHALL remain limited to staging resources whose Phoenix ownership is proven exactly

WHEN a sub-agent working directory is inherited from its parent or supplied as an override
THE SYSTEM SHALL apply the same validation before the sub-agent conversation is persisted or run

**Rationale:** A system-root working directory makes relative tool paths resolve from the
entire filesystem. Even read-only tools can consume unbounded resources when search or
listing operations start at the filesystem root, so the root floor is independent of
conversation mode and write permissions.

---

### REQ-PROJ-001: Open a Git Repository as a Hidden Conversation Repository

WHEN the user creates a new conversation by providing a directory path
AND the request is not the explicit gateway Direct/no-repository intent defined by [Slack gateway](../slack-gateway/requirements.md) REQ-SG-008
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

WHEN the trusted Slack gateway supplies its explicit Direct/no-repository creation intent
THE SYSTEM SHALL create an ordinary Direct conversation at the server home directory without Git discovery changing the requested shape or attaching a hidden `GitRepository`
AND SHALL retain ordinary durable creation, root-floor validation, model defaults, and permission behavior
AND SHALL NOT expose this exception as a new mode picker for ordinary directory-based creation

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
AND ensure `.phoenix/` is excluded through the repository's local Git common-directory `info/exclude` file without rewriting the user's tracked `.gitignore`

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

### REQ-PROJ-017: Record One Pinned Starting Commit Without Fallback Semantics

WHEN Phoenix provisions a Git-backed disposable worktree
THE SYSTEM SHALL resolve exactly one starting commit before materializing the worktree
AND SHALL record that pinned exact commit together with the authoritative canonical default-branch identity that produced it
AND SHALL record the worktree path only after materialization succeeds
AND SHALL NOT persist a stale/local canonical fallback result, an unresolved provisioning result, or missing-as-implicit worktree metadata as creation success
AND SHALL NOT require conversation ownership semantics to include a selected branch name, a dedicated branch-type discriminator, or a Phoenix-owned branch-lifecycle field

THE Direct mode SHALL carry no Git-backed worktree metadata

WHEN a Git-backed conversation later closes
THE SYSTEM SHALL release the same ProductConversation-scoped worktree according to the Close contract
AND SHALL NOT infer any branch-ownership mutation from the recorded starting provenance

**Rationale:** Users still need to know where a conversation started, but creation records one exact pinned starting fact rather than a fallback ladder, staged unresolved shell, or owned branch lifecycle.

---

### REQ-PROJ-022: Default-Branch Materialization Uses Authoritative Origin or Exact Local Main Only

WHEN Phoenix materializes the starting commit for a new Git-backed conversation, a Start-in-new-conversation spawn, or a follow-up conversation
THE SYSTEM SHALL first detect whether the repository has a remote named `origin`

WHEN remote `origin` exists
THE SYSTEM SHALL hold the `RepositoryMutationLock` while discovering the remote's current authoritative default branch and fetching that branch tip fresh for provisioning
AND SHALL provision from the freshly fetched authoritative default-branch tip at detached `HEAD`
AND SHALL treat any remote-default discovery failure, authentication failure, authorization failure, transport failure, or fetch failure as an explicit retryable creation-job failure
AND SHALL NOT fall back to cached remote state, previously observed canonical-default facts, local branch tips, the currently checked out branch, another arbitrary ref, or a synthetic default

WHEN remote `origin` does NOT exist
THE SYSTEM SHALL resolve only the exact OID of local `refs/heads/main`
AND SHALL provision from that exact commit at detached `HEAD`
AND SHALL treat a missing local `refs/heads/main` or an unresolvable local `refs/heads/main` OID as an explicit retryable creation-job failure
AND SHALL NOT fall back to another local branch, HEAD, a remote-tracking ref, or a synthetic default

THE SYSTEM SHALL pin the exact starting OID once for the accepted creation job
AND SHALL NOT refresh, rediscover, or replace that pinned OID on retry, restart, claim replacement, publication, or runtime bootstrap after it has been recorded

**Rationale:** Ordinary conversation provisioning needs one truthful starting-point rule. Either Phoenix provisions from the current authoritative `origin` default branch after a fresh fetch, or—when no `origin` exists—from the exact local `main` ref. Anything else is guesswork.

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
THE SYSTEM SHALL atomically persist exactly one durable creation job identity for that request, its complete creation intent, and any stable replay handle before acknowledging acceptance
AND SHALL NOT publish a user-visible ready conversation, transcript row, attached `WorkScope`, objective, navigation side effect, or initial message until the creation job reaches one whole publishable outcome
AND filesystem, Git, attachment finalization, runtime bootstrap, and any queued post-publication work SHALL occur after acceptance
AND the acceptance acknowledgement SHALL return the durable request identity and reserved ProductConversation identity without waiting for that after-acceptance work to complete

WHEN acceptance is acknowledged before publication
THE SYSTEM SHALL expose the exact durable request's progress and recovery outcome by request identity
AND the client SHALL remain on a creation or recovery surface until that exact request publishes atomically, fails, or is cancelled
AND SHALL navigate to the conversation route only after observing atomic publication

WHEN public exact-request status observes a retained creation whose published product or original transcript is absent
THE SYSTEM SHALL report the original request identity, reserved product identity, and original transcript identity as a historical publication without a live route or mutating recovery action
AND SHALL distinguish this retained outcome from a request identity that was never accepted
AND the client SHALL stop polling without navigating, reposting, or retrying that request, while allowing explicit local release of the observation without deleting the server record.

WHEN the same request is retried with the same request identifier and the same creation intent
THE SYSTEM SHALL replay the original acceptance result rather than creating a second job

WHEN the same request identifier is reused with different creation intent
THE SYSTEM SHALL reject the reuse as a conflict rather than silently retargeting the accepted job

A created ordinary ProductConversation SHALL begin in Open state only when publication succeeds. The lifecycle-free Coordinator SHALL preserve its Coordinator identity when its singleton aggregate is created on demand and SHALL NOT receive Open/History state. Moving an ordinary ProductConversation to History is the Close action owned by the conversation lifecycle; provisioning, retries, cancellation, and deletion SHALL NOT invent a parallel lifecycle label.

### REQ-CCR-002: Exclusive Provisioning Authority

WHEN a worker begins or resumes provisioning
THE SYSTEM SHALL grant that worker one time-bounded creation claim with a monotonically increasing generation
AND every authoritative provisioning update SHALL require the current claim and generation
AND the durable job identity SHALL remain the request identifier accepted under REQ-CCR-001 rather than a later worker-local token

WHEN a worker reports a result after losing its claim
THE SYSTEM SHALL reject the result without changing job, conversation publication, message publication, pin selection, or resource ownership state

WHEN a worker checkpoints a provisioning stage
THE SYSTEM SHALL require a live matching claim using a current microsecond clock sample taken by the persistence layer after acquiring write authority
AND SHALL allow an unchanged same-stage checkpoint or the exact next stage in this order: `validate_intent`, `resolve_repository`, `resolve_starting_pin`, `materialize_worktree`, `finalize_attachments`, `commit_metadata`, `bootstrap_initial_turn`, `finalize_publication`
AND SHALL reject stage skips, regressions, and transitions whose clock sample precedes the stored stage timestamp
AND SHALL preserve the stored timestamp for an idempotent same-stage checkpoint
AND SHALL advance through no-op stages for direct, non-Git creation rather than skipping them
AND SHALL retain the recorded stage when a replacement worker revisits earlier phases for reconciliation

WHEN creation metadata is published
THE SYSTEM SHALL require `commit_metadata` as the predecessor and atomically advance to `bootstrap_initial_turn`

WHEN objective delivery completes creation
THE SYSTEM SHALL require `bootstrap_initial_turn` as the predecessor and atomically advance to `finalize_publication`

THE SYSTEM SHALL reserve those two transitions for their publication and completion transactions rather than an independent stage checkpoint

### REQ-CCR-003: Crash Reconciliation

WHEN a creation claim expires during an external operation
THE SYSTEM SHALL fence that generation from further authoritative updates
AND a replacement worker SHALL inspect observed external state before resuming, adopting, conflicting, or cleaning the operation

THE SYSTEM SHALL NOT infer that an external operation failed merely because its acknowledgement was not persisted
AND SHALL preserve ambiguity when external ownership or cleanup safety cannot be proven exactly

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
AND deterministic staging paths beneath the repository's `.phoenix/worktrees` namespace SHALL be a Phoenix-private resource namespace whose Phoenix writers serialize through the repository mutation lock
AND cleanup ownership checks SHALL protect against stale Phoenix operations and observed identity mismatch within that namespace, while hostile same-user filesystem mutation outside Phoenix writers is not part of the automatic-cleanup threat model
AND SHALL enter an explicit non-destructive ambiguous cleanup outcome when ownership, equivalence, or safety cannot be proven exactly

ADR-043 records the private staging namespace and automatic-cleanup threat boundary for this requirement.

### REQ-CCR-005A: Immutable Starting Pin Selection

WHEN Git-backed creation resolves its starting commit under REQ-PROJ-022
THE SYSTEM SHALL persist exactly one immutable starting pin for that accepted creation job
AND SHALL use the exact OID produced by either the freshly fetched authoritative `origin` default branch or local `refs/heads/main` when no `origin` exists
AND SHALL treat any inability to produce that exact OID as explicit retryable creation-job failure rather than as an unresolved success outcome or fallback selection
AND SHALL treat that pin as immutable across retries, claim replacement, runtime bootstrap, publication, and later conversation lifecycle operations
AND SHALL allow later repository operations to move the worktree independently without rewriting the recorded creation pin

**Rationale:** Creation needs one truthful starting-point fact. The pin records where the conversation began; it is not a branch-ownership claim and it does not get recomputed after acceptance.

---

### REQ-CCR-006: First-Class Runtime Bootstrap

WHEN provisioning needs to queue objective or navigation work after successful publication
THE SYSTEM SHALL transition directly from provisioning into the normal conversation lifecycle through an idempotent bootstrap operation
AND SHALL queue that requested follow-on work only after the ProductConversation and attached usable `WorkScope` are published
AND the system SHALL NOT temporarily publish a partial or idle conversation solely to initialize a runtime

### REQ-CCR-006A: Atomic Publication

WHEN creation reaches a publishable success outcome
THE SYSTEM SHALL publish the ProductConversation, transcript state, attached usable `WorkScope`, and immutable starting pin as one atomic user-visible outcome after worktree materialization succeeds
AND SHALL queue any requested objective or navigation work only after that publication boundary when the user flow says to queue it after creation
AND SHALL NOT expose an earlier staged, partial, unresolved-shell, or early-publication state on normal user-facing surfaces
AND SHALL NOT perform creation-time file expansion or creation-time skill expansion as part of that publication transaction

WHEN publication cannot commit all required creation facts together
THE SYSTEM SHALL leave the job non-ready and retryable or failed according to the ordinary creation policy rather than publishing a partial conversation

**Rationale:** Users can recover from a failed creation, but they should never see a conversation that claims to exist before its starting state is whole.

---

### REQ-CCR-007: Cancellation

WHEN a user cancels an accepted, claimed, retry-scheduled, or `delivery_pending` creation before durable objective acceptance
THE SYSTEM SHALL immediately revoke the active generation
AND SHALL preserve a visible cancelled creation record with its original creation intent
AND SHALL reconcile owned resources asynchronously, including removing the exactly owned hidden staged ProductConversation, transcript, and attached `WorkScope` aggregate without publishing it
AND SHALL preserve selected working-directory segments whose exclusive Phoenix ownership cannot be proven

WHEN cancellation races objective enqueue for a `delivery_pending` creation
THE SYSTEM SHALL linearize cancellation and durable objective acceptance atomically under the same persistence write authority
AND, if cancellation commits first, SHALL reject the revoked delivery claim without persisting an objective acceptance receipt or queued objective
AND, if durable objective acceptance commits first, SHALL reject creation cancellation without revoking the claim or removing the aggregate, even if completion has not yet marked the creation ready or the queue has already drained

THE SYSTEM SHALL NOT allow creation cancellation after durable objective acceptance
AND SHALL leave subsequent lifecycle actions to the ordinary conversation lifecycle

WHEN reconciliation completes successfully after cancellation
THE SYSTEM SHALL offer Start over and Delete for the cancelled record

### REQ-CCR-008: Deletion During Creation

WHEN a user deletes a non-ready creation record
THE SYSTEM SHALL immediately omit it from normal user-facing conversation surfaces
AND SHALL retain an internal deletion-pending record until owned resources are safely reconciled or explicit ambiguity is recorded
AND SHALL physically delete the record only after reconciliation succeeds

### REQ-CCR-009: Durable Scheduling

WHEN an accepted job, retry deadline, or expired lease becomes eligible
THE SYSTEM SHALL make it discoverable without requiring another conversation request or process restart

### REQ-CCR-010: Deterministic Verification

WHEN the creation protocol is verified
THE SYSTEM SHALL exercise generated operation schedules containing concurrent claims, lease expiry, late results, crashes, retries, cancellation, deletion, immutable pin selection, atomic publication, and ambiguous external-effect completion
AND SHALL check lifecycle and ownership invariants after every generated operation
AND SHALL retain minimized failing schedules as deterministic regressions

### REQ-CCR-012: Provisioning-Stage Migration Preserves Durable Evidence

WHEN a forward migration adds provisioning-stage fields to product creation rows that lack them
THE SYSTEM SHALL preserve the rows, request and product identities, accepted intent, status, claims, pins, and staging facts
AND SHALL derive `provisioning_stage` from durable row evidence in this precedence order:

1. `published` status maps to `finalize_publication`.
2. `delivery_pending` or `delivery_failed` status maps to `bootstrap_initial_turn`.
3. A non-null staging path maps to `materialize_worktree`.
4. A non-null exact checkout pin maps to `resolve_starting_pin`.
5. All remaining rows map to `validate_intent`.

THE SYSTEM SHALL copy each row's `updated_at_unix_micros` into `stage_updated_at_unix_micros` without substituting migration time
AND SHALL treat a backfilled stage as the phase justified by that evidence, not as proof that an unacknowledged external effect completed
AND SHALL retain claim-fenced reconciliation and sequential advancement when work resumes
AND SHALL NOT infer additional completed phases from repository state, error text, or elapsed time

This forward transformation grants no downgrade, mixed-version writer, or live database replacement guarantee beyond [the compatibility requirements](../compatibility/requirements.md).

### REQ-CCR-013: Coordinator Creation Preserves Intent and Provenance

WHEN the current Global Coordinator requests an ordinary conversation through its dedicated creation capability
THE SYSTEM SHALL use the shared durable creation acceptance and reconciliation path with one canonical request identity and an explicit submit-objective start intent
AND SHALL accept only nonblank objective text, an existing absolute directory, and validated supported model, effort, and language choices
AND SHALL NOT accept attachments, caller-authored origin metadata, filesystem references, or skill expansion as part of this capability.

WHEN the Coordinator discovers project directory options
THE SYSTEM SHALL return only existing, normalized directories satisfying creation's directory floor
AND SHALL apply its result bound and truncation indicator to usable options rather than stale persisted candidates.

THE SYSTEM SHALL persist the executing product, transcript, assistant message, and tool-use identities as internal-conversation provenance
AND SHALL deliver Coordinator objective text literally as agent-authored input rather than user-authored input
AND SHALL retain the first accepted provenance on an identical retry while allowing a fresh source-call locator from the same authorized source
AND SHALL reject reuse of a request identity for a different intent rather than reserve another conversation.

WHEN the Coordinator reconciles an uncertain creation result
THE SYSTEM SHALL return the original durable request and product identities and observed creation status
AND SHALL distinguish acceptance from publication, objective delivery, runtime execution, and completion
AND SHALL derive the complete native readback, including publication identity, route, and execution evidence, from one persistence snapshot that also validates the current Coordinator's authority
AND SHALL NOT implicitly cancel, restart, or repeat an intentional creation under a new identity.

WHEN the published product or its original transcript is deleted
THE SYSTEM SHALL retain the accepted request identity, immutable intent identity, first source provenance, reserved product identity, and original publication identity
AND SHALL reconcile an identical retry to that historical acceptance rather than create another product or repeat work
AND SHALL reject changed-intent reuse of that request identity
AND SHALL expose the original publication as historical without a live route, even when the product survives through a successor transcript
AND SHALL NOT claim, renew, deliver, complete, retry, recover, or schedule reconciliation deadlines for delivery to the missing original transcript, including through an already-held claim
AND SHALL report only exact retained terminal execution evidence, or unavailable execution when that evidence is absent, without retargeting the original objective to a successor.

THE SYSTEM SHALL preserve public user API, historical, and Slack gateway origin decoding and objective-delivery behavior independently of the Coordinator's literal-input policy.


### REQ-CCR-014: Operation-Owned Initial Execution Evidence

WHEN the exact accepted initial objective settles as completed, failed, or cancelled
THE SYSTEM SHALL atomically retain its first terminal outcome with consumption of that objective's execution occurrence, independently of watch registration and notification delivery
AND SHALL bind the receipt to the accepted request and its original published transcript, preserve an actual failure reason, and retain it across process restart and target deletion
AND SHALL apply the same contract to captured initial objectives settled by a direct turn.

WHEN operation-specific evidence is absent
THE SYSTEM SHALL report initial execution as unavailable rather than infer a terminal outcome from publication, an unrelated later turn, the transcript's latest state, or a successor
AND SHALL NOT treat retained evidence as authority for execution, recovery, or retargeting.

### REQ-CCR-015: Creation Image Payload Lifetime

WHILE any transcript of the published product exists
THE SYSTEM SHALL retain the accepted creation-request image delivery payloads without changing their bytes.

WHEN the product's last transcript is hard-deleted
THE SYSTEM SHALL atomically retire those delivery payloads while retaining historical creation status, original identities, provenance, and bounded ordered image replay identity
AND SHALL NOT retire payloads when only the original transcript is deleted with a surviving continuation.

WHERE delivery payloads are retired
THE SYSTEM SHALL reconcile identical request retry to the retained original receipt and conflict on a changed image count, order, media type, or exact accepted data
AND SHALL retain each image's ordinal, media type, and SHA-256 digest of the exact accepted UTF-8 data string, not its payload
AND SHALL structurally distinguish retired payloads from an originally empty image list and from missing or partially corrupted live delivery data
AND SHALL fail on live payload corruption rather than silently substitute empty intent
AND SHALL NOT bootstrap, reserve resources, enqueue an objective, or retarget a successor during replay.

WHEN upgrading stored creation receipts
THE SYSTEM SHALL atomically derive replay identity from retained payloads before retiring payloads whose published product no longer exists
AND SHALL NOT fabricate erased execution outcomes.

This payload lifetime concerns creation-request delivery copies only, not independent transcript attachments.
