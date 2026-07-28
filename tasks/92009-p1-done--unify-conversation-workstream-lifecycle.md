# Unify conversations around Open work, disposable worktrees, and retained History

## Purpose

Replace Phoenix's overlapping conversation modes, cleanup verbs, archive behavior, and chain UI with one coherent product model:

- A user sees **one conversation**, which may contain **many durable transcripts** joined by explicit continuation boundaries.
- An **Open** conversation is interactive. When project-backed, it owns one disposable worktree and its work-scope resources.
- **Close conversation** explicitly ends the interactive work, removes the owned environment, and moves the complete conversation into read-only **History**.
- A conversation owns its worktree, never branches or PRs. Branches and PRs are repository artifacts Phoenix observes.
- **Delete permanently** is a History-only retention action.
- Later work starts a new conversation with a fresh worktree and a typed link to its source; History is never reopened.

This task is the durable completion owner for the whole migration. The decisions below are settled product scope, not prompts to rediscover the taxonomy during implementation.

## Why the current model is wrong

Phoenix currently exposes several operations with overlapping or misleading effects:

- **Clean up** means mark merged, remove resources, and leave a terminal row in Active.
- **Abandon** captures a diff snapshot, removes similar resources, and also leaves a terminal row in Active.
- **Archive** looks organizational but irreversibly removes live resources and makes the conversation read-only.
- **Active** means merely “not archived,” so it includes conversations that cannot advance work.
- Work and Branch modes imply that a conversation owns a task branch or selected branch, despite the multi-PR architecture already observing plural branches and PRs by work scope.
- Continuation chains appear as multiple conversations and have a dedicated page even though the user understands them as phases of one conversation.

These concepts mix four independent concerns: whether interaction can continue, what environment Phoenix owns, what repository facts Phoenix observes, and whether retained history still exists.

## Vocabulary and authoritative model

### User-facing vocabulary

Use these product terms consistently:

- **Open** — the conversation can receive messages and advance work.
- **History** — the conversation is closed, read-only reference material and owns no live resources.
- **Close conversation** — explicitly transition the whole conversation from Open to History.
- **Start follow-up** — create new Open work from a History conversation.
- **Delete permanently** — remove a History conversation and its retained transcripts.

Do not expose Archive, Clean up, Abandon, Mark as merged, Work mode, Branch mode, or Chain as lifecycle concepts after migration.

Open/History applies to ordinary project and non-project conversations. A non-project conversation follows the same explicit Close/read-only History lifecycle but skips worktree-specific inspection and teardown when it owns no worktree. The durable `/global` Coordinator is a special singleton surface: preserve its existing availability and exclude it from ordinary conversation Close/Delete controls unless its own global-recall specification is deliberately changed in separate scope.

“Workstream” may be used internally in implementation prose to mean the complete user-facing conversation, but **conversation** is the product noun. “Current” is not a lifecycle label.

### Lifecycle versus execution state

Open and History are the only user-facing lifecycle states. Working, cancelling, awaiting recovery, awaiting continuation, context exhausted, awaiting task approval, and recoverable error are durable execution/blocking conditions **inside Open**. They may temporarily suppress ordinary message input without changing lifecycle. `HandedOff` predecessor rows are read-only transcript segments inside an Open aggregate while the latest successor remains Open; they are not independent History items.

The permitted user actions are fixed:

| Condition | Ordinary message | Continuation/task decision | Close | Delete permanently |
|---|---|---|---|---|
| Open and idle/resumable | Per existing durable-turn rules | When offered | Yes | No |
| Open and actively working | Per existing steering/queue rules | No competing decision | Yes, through stop-and-close confirmation | No |
| Open and cancelling/recovering | Blocked until typed settlement | No | Continue the one durable Close obligation after settlement; do not start another | No |
| Open and awaiting continuation | No ordinary message | Continue or explicitly decline according to bedrock | Close only after that blocking decision is resolved | No |
| Open and awaiting task approval | No ordinary message | Request changes, Reject, Continue here, or Start in new conversation | Not offered until the blocking decision resolves | No |
| History | No | Start follow-up only | Already closed | Yes |
| `/global` | Existing global-recall policy | Not applicable | Not offered | Not offered |

Only the latest row is a live execution target. If a continuation already exists, another continuation request resolves/navigates to that same winner; it never creates a branch in the linear topology.

### One conversation, many durable transcripts

The existing durable conversation state machine, persisted messages, continuation rows, durable-turn authorities, transcript generation, and SSE projections remain authoritative. Do not create a parallel workstream, phase, turn, message, or transcript authority.

A continuation creates a new durable successor conversation row linked through the existing linear `continued_in_conv_id` topology. Product presentation projects the root-to-latest row sequence as **one conversation containing multiple transcripts**:

- conversation rows remain the durable transcript segments;
- persisted messages remain the sole content authority;
- the linear continuation relation remains the topology authority;
- durable direct-turn state remains the authority for accepted turn identity, prepared semantics, ownership, generation, terminal outcome, and child effects;
- UI transcript state, SSE state, search results, and list entries are one-way projections and cannot redefine those facts.

Account for the durable direct-chat program accurately: it is **active work in progress**, not abandoned or blocked work, and its major authority/repository foundations have landed. Additional roadmap slices may land while this task is planned or implemented. At the start of each child task, rebase/re-ground against the then-current durable-workflow specs, ADRs, schema, and production-consumer status. Integrate at the typed boundaries that exist at that point; keep this migration isolated from unrelated direct-chat roadmap work, but never bypass landed authority with an older emit-only path or introduce a second authority. This task owns only the durable-turn changes required by its user journeys and invariants.

## Product decisions

### 1. Unified project conversations

Creating a project conversation does not ask the user to choose Explore, Work, or Branch.

Every new project conversation receives one isolated disposable worktree:

- one user-facing project conversation has exactly one live work scope and one worktree path at a time;
- continuation transfers that same work scope to the successor transcript row instead of creating another worktree;
- multiple independent conversations for one repository use distinct worktree paths and may run in parallel;
- a pre-existing target path is a typed provisioning/reconciliation case and must never be silently adopted as a different conversation's workspace.

- create it from the current canonical default-branch commit;
- start it at detached `HEAD` rather than creating or checking out a Phoenix task branch;
- use it for exploration, planning, implementation, review, and PR work;
- allow the agent to create, checkout, switch, and stack branches as the work requires.

The detached starting commit has one bounded freshness rule. Resolve the project's canonical default branch using the project's authoritative default-branch identity. Perform the existing **single-branch best-effort fetch/materialization** for that branch—never a blanket `git fetch --all` or `git fetch --prune`—then detach at the fetched remote-tracking tip. If network refresh fails but a previously resolved local/remote-tracking ref exists, proceed from that cached commit and surface that the starting point may be stale. If no default-branch commit can be resolved, provisioning fails clearly rather than guessing from an arbitrary checked-out branch. New conversations, derived tasks, and follow-ups use this rule; continuation reuses its existing worktree and does not fetch merely to continue.

Explore versus implement is agent behavior and permission/tool availability within the interaction, not Git ownership encoded in a conversation mode. Remove branch name, base branch, task branch, and branch-disposition fields from conversation ownership semantics. Normalize persisted data rather than retaining old and new semantic authorities in parallel.

### 2. Worktree-only ownership

A project conversation owns:

- its disposable worktree;
- terminal/PTY resources scoped to that worktree;
- bash handles and process groups scoped to the work scope;
- tmux resources scoped to the work scope;
- browser sessions scoped to the work scope;
- equivalent live execution resources.

It does not own:

- the branch currently checked out in the worktree;
- any branch created or visited by an agent;
- a stack of branches;
- a task branch;
- any open, draft, closed, or merged PR.

Preserve ADR-008's model: observe settled Git state at supported reconciliation boundaries, retain plural WorkScope-keyed PR associations, and target one explicit active PR for PR-specific actions without turning that PR into an owner.

Conversation lifecycle code must never create, move, fast-forward, delete, merge, close, push, or retarget a branch or PR as a consequence of Close or Delete. Removing a worktree is authoritative; branch and PR mutation is outside the lifecycle.

A named Git ref is independent repository state, not conversation-owned state. For Close loss detection, commits reachable from `refs/heads/*`, `refs/remotes/*`, `refs/tags/*`, or `refs/stash` survive worktree removal and are not reported as lost. Reflog-only reachability is not durable enough: detached commits reachable only from reflogs are reported as at risk. This classification changes warnings only; Close never moves or deletes any ref, including stashes.

### 3. Open and History navigation

Replace Active/Archived and chain blocks with two conversation-list projections:

- **Open** contains only interactive conversations that can still advance work, including durable busy, cancellation, recovery, and continuation states where the work has not been closed.
- **History** contains closed, read-only conversations.

Each linear continuation chain appears exactly once, using its durable root identity and latest member for live state, routing, and activity ordering. Opening an item always uses the normal conversation page.

Use one stable user route keyed by the durable root identity/slug. Opening from Open, History, search, provenance, or a legacy member link resolves to that unified page; the latest row supplies live execution state and the default scroll target. Member-row identity remains available for exact message links and internal reconciliation but is not a separate lifecycle route. List title/name belongs to the root aggregate, and ordering uses latest activity across its transcript rows.

Derived-task and follow-up provenance is visible in the conversation header/info surface using **Derived from** or **Follow-up to**. It is navigational metadata, not transcript membership. Legacy chain URLs and member URLs must redirect/resolve to the unified page while compatibility is needed; final UI and navigation expose no chain route.

Remove the dedicated chain page and its user-facing route, sidebar affordances, rename/archive/delete actions, and fallback navigation. Backend linear-topology and bounded-retrieval helpers may remain where they are the correct authority, but they are not a separate product surface.

The existing `/global` Coordinator remains a separate chat-only global-recall surface. This migration does not replace it, add ambient global memory to ordinary conversations, or make the Coordinator a workstream manager. Open-work references used by the Coordinator must resolve to the latest live transcript row while retaining the durable conversation root identity.

### 4. Seamless transcript with significant continuation boundaries

Render all transcript segments in root-to-latest chronological order on one scrollable conversation page.

A context boundary is a significant semantic event:

- present the continuation prompt clearly before creating the successor;
- persist the exact handoff summary used to initialize the successor model context;
- render that summary once at the join as a clearly labelled, expandable boundary;
- distinguish the old transcript, handoff, and new transcript without forcing separate-page navigation;
- do not duplicate the handoff as unrelated ordinary messages on both sides;
- do not inject the full earlier transcript into the successor automatically.

The next model receives the persisted handoff summary. Earlier messages remain available through bounded conversation search. The visible boundary must be derived from the same durable handoff data the successor received, so the user can inspect what crossed the context boundary.

Long conversations may use pagination or virtualization, but those optimizations cannot change chronology, hide continuation boundaries, break message identity, or make earlier transcripts unsearchable.

### 5. Close conversation

**Close conversation** is the sole ordinary Open-to-History transition. It is always explicitly initiated by the user.

External observations—such as a merged PR, clean workspace, closed PR, or absent PR—may influence guidance and make Close prominent, but they never automatically close the conversation and never classify the whole conversation as merged or abandoned.

Close applies to the complete linear conversation, not one transcript row. It performs this user-visible sequence:

1. **Resolve active durable work.** If an LLM turn, tool, sub-agent, wake obligation, or background command is active, explain that Close will stop it and ask for confirmation using language such as **Stop work and continue closing**.
2. **Cancel and settle through existing authorities.** Invoke the typed durable cancellation/recovery machinery, wait until runtime ownership is released or a typed failure is returned, and preserve all already committed transcript history. Close orchestrates existing cancellation; it does not invent a second cancellation lifecycle.
3. **Inspect worktree-only loss.** Determine whether removing the worktree would discard state that has no durable Git ref.
4. **Warn exactly when needed.** If loss exists, show an exact categorized inventory and ask the user to confirm discard.
5. **Do not preserve automatically.** Confirmed Close does not create a branch, tag, commit, stash, patch, diff snapshot, or other recovery artifact.
6. **Release owned resources.** Remove the worktree and all remaining work-scope resources while leaving repository refs and PRs untouched.
7. **Commit History state.** Make every transcript segment read-only and move the one user-facing conversation into History.

The loss inventory uses these exact categories:

- staged tracked changes;
- unstaged tracked changes, including conflicted/unmerged paths;
- untracked, non-ignored files;
- dirty or untracked state inside initialized submodule checkouts;
- detached commits not reachable from a named durable ref under `refs/heads/*`, `refs/remotes/*`, `refs/tags/*`, or `refs/stash`.

Show categories independently when they coexist, including paths and detached commit identities. Ignored files are intentionally excluded: Close is not a filesystem backup and must not inventory build caches or other ignored output. LFS-tracked edits are ordinary tracked changes; missing remote LFS objects are not Close-loss state. Do not recursively invent preservation behavior for nested repositories outside declared submodules.

A local branch is independent durable repository state: its commits remain reachable even if unpushed, so Close may report useful status but must not claim that removing the worktree loses those commits. Stashed commits also survive and are not reported as lost. Reflog-only detached commits are reported as at risk. Associated PR state does not block Close.

Close is one recoverable, idempotent durable operation with this persisted phase order:

1. record one close obligation for the root conversation before destructive side effects;
2. settle/cancel active durable work;
3. record the user's loss confirmation against the inspected worktree generation/status fingerprint so a changed workspace requires reinspection;
4. release external resources and remove the worktree;
5. finalize the entire linear conversation as History.

A retry or restart resumes that same obligation from durable phase evidence and converges on the same result; it cannot create another Close, duplicate cancellation, duplicate transcript events, or automatic recovery artifacts. Before worktree removal, the user may cancel Close and return to Open after active-work settlement. Once destructive resource removal has begun, Close is committed and must finish or expose a retryable repair state rather than pretend it was cancelled.

Required DB phase transitions are transactional. External resource teardown cannot be one SQLite transaction: each step records enough completion/error evidence to retry safely. A worktree-removal failure leaves the conversation in a visible retryable Closing condition, not History. If the worktree is already absent on retry and identity/evidence shows this Close removed or adopted that absence, continue finalization. Final History is written only after owned-resource retirement has completed. If automatic cleanup cannot make progress, retain a typed visible **Closing needs repair** execution condition under Open, record the exact residual resource/error, and offer retry/operator-resolution; do not call the conversation History while it still owns resources. Residual cleanup is visible and logged, never silent.

A crash must not leave contradictory authority such as History owning a live worktree, Open after irreversible removal without a durable close obligation, or only some continuation rows finalized. From the UI's perspective the whole linear conversation changes to History together; topology and messages are unchanged.

`propose_task` approval is a blocking conversation interaction. While that interaction is active, the user resolves it through Approve, Request changes, or Reject before ordinary Close is offered; there is no independent background “pending plan” lifecycle.

### 6. History and permanent deletion

History is a hard boundary:

- it is read-only;
- it owns no live environment;
- it cannot be reopened or made writable in place;
- later work uses Start follow-up.

Delete permanently is available only from History. An Open conversation must first complete Close, including busy-work and loss confirmations.

Delete removes the entire retained conversation—all durable transcript rows, messages, task-approval history owned by those rows, retrieval-index projections, attachments, and other solely owned records—without affecting branches, PRs, derived conversations, follow-ups, or siblings. Related conversations survive. If a surviving provenance link points to deleted History, degrade honestly to unavailable/deleted source metadata rather than cascading deletion or silently retargeting it.

Deletion is idempotent and non-cascading across provenance. Preserve a tombstone-grade source identity sufficient for surviving typed links to render **Deleted source** without retaining transcript content; do not use nullable omission that makes “never had a source” indistinguishable from “source was deleted.” Repeated Delete either finishes remaining solely-owned cleanup or reports already deleted. FTS rows are derived sinks and must be pruned/rebuilt consistently with message deletion.

### 7. One `propose_task` flow with approval placement

`propose_task` continues to accept one task/brief file path. The file is the complete curated handoff; the tool does not gain chain, search-scope, context-copy, or lineage arguments.

Preserve existing file-safety semantics that remain independently valid:

- the proposal refers to a real allowed Markdown file in the conversation worktree;
- Phoenix snapshots it for review;
- approval revalidates/re-reads the intended file so the approved content is exact;
- taskmd and plain-brief distinctions remain typed where they have different task-tracking semantics.

Remove the mode-dependent split where Explore proposals park for approval but writing-mode proposals use a separate non-blocking fork behavior. In the unified project-conversation model, a proposal enters one blocking review interaction. The user may Request changes, Reject, or Approve with one of two placements:

#### Continue here

- Keep the same user-facing conversation and worktree.
- Use the approved task as the next objective.
- Preserve the useful conversation context.
- Do not rename/create a branch as an approval side effect.

#### Start in new conversation

- Create a new independent Open conversation.
- Create a fresh detached-default-branch worktree.
- Use the exact approved task file content as the new conversation's initial objective/context.
- Materialize/preserve the task artifact according to its taskmd or plain-brief contract without creating an owned task branch.
- Do not copy or inject the source transcript or an automatic source summary.
- Record one typed **derived from** provenance link automatically.
- Start execution in the new conversation.

The source conversation remains Open and resumes after spawning. The new conversation is not a continuation member of the source. Tasks are planning/delivery artifacts, not owners of conversations, worktrees, branches, or PRs. One conversation may propose and implement multiple tasks.

After successful Start in new conversation, navigate the user to the spawned conversation because it is the newly approved work destination; the still-Open source remains available in Open navigation. Continue here is the preselected/default placement because it has fewer side effects, while the user can deliberately choose fresh context.

The approved task content must survive independently of the source worktree that will eventually close. Persist one normalized approval/task-source record for the spawned conversation containing the exact approved body and original repo-relative path/taskmd identity. Materialize the file in the spawned worktree according to taskmd/plain-brief rules. Do not make later source-worktree deletion erase the spawned objective, and do not store a child collection of task artifacts inside a conversation JSON blob.

Approval placement is deliberately user-controlled: Continue here retains productive context; Start in new conversation escapes context pollution after extensive discarded exploration.

Existing product-level parallelism is preserved through Start in new conversation. Do not replace it with same-worktree concurrent agents that can race on one checkout.

### 8. Continuation, derivation, follow-up, and reference are distinct

Model these relationships separately:

- **Continuation** — same unfinished conversation, same worktree, new durable transcript row, linear `continued_in_conv_id` chain, exact handoff summary.
- **Derived task** — an approved task starts an independent conversation with a fresh worktree and a typed source link.
- **Follow-up** — new work prompted from History, with a fresh worktree at current repository reality and a typed source link.
- **Reference** — contextual linkage with no lifecycle, ownership, or automatic search implication.

Continuation chains remain linear. Derived-task and follow-up links may form a provenance tree for navigation, but that tree is not a chain, does not merge transcripts, and is never the scope of Close or Delete.

Start follow-up does not synthesize context from the old transcript. It creates a new conversation linked to History and lets the user provide the new objective (for example, later feedback on a merged feature). The prior conversation is reference material, not a stale execution environment.

### 9. Bounded transcript search

Reuse the existing SQLite FTS5/BM25 message index and in-query `RetrievalScope::Conversations(member_ids)` scoping. Do not create per-chain indexes, embedding infrastructure, or a parallel transcript store.

Search rules are host-bound:

- ordinary transcript search from a conversation covers all durable transcript rows in that conversation's own linear continuation chain;
- a derived-task or follow-up conversation may explicitly search its recorded source conversation when its task/objective is insufficient;
- source transcripts are never injected or searched automatically;
- sibling derived/follow-up conversations are excluded unless separately and explicitly referenced;
- the agent cannot widen scope through an argument to `propose_task` or the search tool;
- search provenance identifies the source conversation/transcript/message.

“Search source conversation” is an explicit agent-readable escape hatch authorized by the typed provenance link. The link is also visible to the user. It does not make a provenance family into one chain.

Expose retrieval as two host-scoped operations/capabilities rather than a model-selected scope parameter: **search this conversation** and, only when one typed source link exists, **search source conversation**. Results identify source conversation, transcript row, message, and continuation-boundary context and can navigate to the exact message in the unified page. A deleted source returns typed unavailable-source behavior; it never broadens to global or sibling search.

This is separate from Coordinator global recall: `/global` retains its existing bounded global message-search and relational-read authority, while ordinary agents receive only the workstream/source scopes above.

## User journeys that must work

### New project conversation

1. User selects a project and starts a conversation without choosing a mode or branch.
2. Phoenix creates a disposable worktree detached at the canonical default-branch commit.
3. The agent explores, plans, implements, reviews, and manages branches/PRs as needed.
4. Phoenix observes plural branch/PR facts without treating them as owned.

### Task approval in place

1. Agent proposes one task file and enters blocking review.
2. User chooses Continue here.
3. The approved task becomes the next objective in the same transcript/worktree.
4. Approval performs no lifecycle branch creation or rename.

### Task approval in fresh context

1. Agent proposes one task file after useful or polluted exploration.
2. User chooses Start in new conversation.
3. Phoenix starts an independent conversation in a fresh detached worktree.
4. Only the exact approved task is initial context; the source transcript is not copied or summarized.
5. The source resumes and the new conversation begins work in parallel.
6. Both sides display truthful provenance without becoming one chain.

### Context continuation

1. The durable state machine reaches the continuation decision and clearly prompts the user.
2. Phoenix generates and persists the handoff summary.
3. On approval, it creates a successor row, transfers the same work scope, and initializes the next model from exactly that summary.
4. The sidebar still shows one conversation.
5. The normal page shows the earlier transcript, one expandable exact-summary boundary, and the later transcript in one scroll.
6. Same-conversation search retrieves relevant messages from either transcript.
7. Reload/reconnect/restart preserves the same boundary, chronology, active row, and accepted durable-turn identity without duplication.

### Close while busy

1. User clicks Close while durable work is active.
2. Phoenix explains that the work will stop and asks for confirmation.
3. Confirmation cancels and settles work through typed durable authorities.
4. Only then does Phoenix inspect and remove the worktree.
5. Failure or restart resumes/reports the same close obligation instead of starting a conflicting close.

### Close with mixed workspace risk

1. The worktree contains modified files, untracked files, and unreachable detached commits.
2. Close displays each category and exact affected paths/commit identities.
3. Cancel leaves the conversation/worktree intact so the user may ask the agent to preserve work.
4. Confirm discards without creating recovery state.
5. Local branches, including unpushed commits, and every associated PR remain unchanged.

### Close cleanly into History

1. Close finds no workspace-only loss.
2. Phoenix releases the entire conversation's work scope and records the whole linear chain as History.
3. Open no longer lists it; History lists it once.
4. The History page is the same seamless transcript page in read-only form.

### Close a non-project conversation

1. User explicitly chooses Close conversation.
2. Phoenix settles any active durable turn through the same typed cancellation rules.
3. Because no project worktree is owned, Phoenix skips Git-loss inspection and worktree removal.
4. The complete conversation moves to read-only History and remains available for follow-up or permanent deletion.

### Follow up on historical work

1. User opens History and selects Start follow-up.
2. Phoenix creates a new Open conversation and fresh detached worktree at current default-branch reality.
3. The user supplies the new objective; Phoenix does not inject the old transcript.
4. The new conversation shows its source link and can explicitly search the source when useful.
5. It does not search source or sibling work by default.

### Permanent deletion

1. Open offers Close but not Delete permanently.
2. History offers Start follow-up and Delete permanently.
3. Delete removes the complete retained conversation and retrieval projections.
4. Related conversations, branches, and PRs survive; source links degrade honestly if their target is gone.
5. No chain route or hidden compatibility UI remains as another deletion path.

## Legacy-data mapping and compatibility decisions

The migration must establish one normalized lifecycle authority for the root conversation aggregate; do not permanently derive lifecycle from a mixture of `archived`, `ConvState`, and continuation fields. Backfill deterministically:

| Legacy shape | New aggregate lifecycle |
|---|---|
| Latest row is live, idle, busy, recoverable, awaiting approval/continuation, errored-but-closable, or context-exhausted without a successor | Open |
| Earlier row is `HandedOff` and a latest successor exists | Same lifecycle as the latest successor; earlier row is a read-only transcript segment, not History |
| Latest row is old `Terminal` from mark-merged/abandon and has no successor | History |
| Existing archived standalone conversation or fully archived chain | History |
| Mixed archived flags inside a chain with a live latest successor | Open aggregate; preserve every segment and discard member-level archive as lifecycle authority after backfill |
| Provisioning/creation-failed legacy row | Preserve its typed creation condition under Open until creation is retried or the user closes it; do not fabricate a worktree |

Backfill one aggregate lifecycle value per durable root and verify no chain becomes mixed Open/History. Retain old fields only as read-only migration inputs until every writer/reader is cut over, then remove them. A temporary compatibility projection may have one authoritative writer; two writable lifecycle representations are forbidden.

Legacy project conversations map as follows:

- preserve an existing live worktree and its checkout exactly; do not detach, rename, clean, or otherwise rewrite a user's in-flight legacy workspace merely to satisfy the new-creation rule;
- stop treating its recorded branch/base/task fields as ownership immediately after the observation/provenance data they uniquely carry is normalized;
- an Open project conversation without an owned worktree provisions a fresh detached worktree before its next filesystem-capable interaction, using the canonical-default freshness rule; its former cwd is not owned and is never cleaned or migrated implicitly;
- legacy non-project conversations remain non-project and do not acquire a worktree;
- historical rows never provision a worktree.

Relationship fields remain distinct. `continued_in_conv_id` stays continuation topology; sub-agent `parent_conversation_id` stays execution parentage; seed relationships stay seed relationships. Add normalized typed provenance for `derived_task` and `follow_up`. Backfill a legacy top-level task fork's existing `spawned_from_conversation_id` as `derived_task` only where its typed proposal/fork records prove that meaning; do not guess from a nullable ID alone. Do not overload any of these fields to represent another relation.

Legacy API and offline-queue behavior is a migration concern, not a second product taxonomy:

- do not repurpose one old endpoint so the same verb means different lifecycle actions by row shape;
- during dependency-ordered cutover, temporary deprecated adapters may map old Archive/mark-merged/abandon callers into the single Close command only after preserving the new confirmation and loss-safety contract; unsafe callers must receive a typed conflict requiring the new flow;
- old hard-delete callers must not bypass History-only deletion;
- compatibility adapters must log their use, have one-way authority, and be removed before this parent task completes;
- migrate or safely drain persisted offline operations so stale Archive/unarchive operations cannot resurrect the old taxonomy.

Existing terminal/archived data is historical evidence, not a declaration that merged/abandoned outcome remains a top-level product state. Preserve any existing outcome messages and snapshots in the transcript; do not synthesize them for new Close operations.

## Normative work required

Before or alongside code changes, update all affected timeless requirements and Allium behavior so they describe this model as present truth. Add a project ADR for the ownership-boundary decision. Do not leave design rationale or operation sequences only in code comments.

At minimum reconcile:

- `specs/bedrock/`: Open/History semantics, continuation prompt and exact handoff, whole-conversation Close, typed busy cancellation, terminal projection.
- `specs/projects/`: unified project conversation, detached worktree creation, one proposal flow, approval placement, continuation transfer, worktree-only ownership, removal of task-branch lifecycle.
- `specs/work-lifecycle/`: replace or retire merged/abandon semantics after moving surviving safety requirements.
- `specs/chains/`: retain linear continuation/search invariants while replacing the dedicated product page and chain-scoped lifecycle with normal conversation presentation.
- `specs/work-actions-bar/`: replace FINISH/Clean up/Abandon taxonomy with Close guidance; preserve PR observation/action honesty.
- `specs/conversation-retrieval/`: own-chain and explicit typed-source retrieval for ordinary agents.
- `specs/pr-association/` and ADR-008: preserve plural observation and explicit active-PR targeting without ownership.
- `specs/global-recall/`: preserve Coordinator separation and root/latest reference behavior.
- `specs/durable-workflows/` and applicable ADRs: integrate Close/continuation with direct-turn acceptance, cancellation, exact-ID reconciliation, and one-authority constraints; update executive status truthfully as production cutover advances.
- SSE/wire specs: restart-safe seamless transcript projection, History gating, exact handoff boundary, and generated wire parity.

Run the spec-authoring preflight in `specs/AUTHORING.md` before pushing spec changes. Timeless specs must not mention this task ID, rollout phases, or implementation status; executive docs own current-reality status.

## Implementation boundaries and starting points

Re-ground symbols after each child task because this migration will move them. Likely authorities include:

- `phoenix-core` conversation state/schema types (`ConvState`, continuation and recovery request types, conversation/work-scope fields).
- `phoenix-workflow` durable-turn aggregate and child effects plus `phoenix-db::workflow::direct_turn` persistence/reconciliation.
- conversation messages, `transcript_generation`, exact message/turn identity, and SSE init/replay.
- `continued_in_conv_id` chain CTEs and latest-member resolution.
- project creation, worktree materialization, Git reconciliation, and cleanup cascades.
- task-file validation, proposal interception, approval execution, and existing fork-proposal persistence.
- conversation list/page routing and chain grouping/page components.
- Work Actions disposition and PR rail.
- FTS retrieval scopes and agent-facing tool registration.
- DB migrations for lifecycle, modes, ownership fields, provenance, and normalized child records.

Use normalized columns/rows for aggregate lifecycle, provenance, Close phases/evidence, and child collections. Do not add these as fields inside existing JSON blobs when SQL must query or migrate them. Each semantic fact has one writable authority. Temporary compatibility projections are mechanically derived/read-only and have an explicit removal gate; avoid parallel representations of one semantic value.

## Autonomous execution plan and phase gates

The implementing agent must first create dependency-ordered child tasks from these gates, then execute them rather than treating decomposition as the deliverable. Rebase/re-ground each child against active durable-direct-chat work before implementation. Keep each intermediate commit shippable, but do not mistake a compatibility stage for completion.

### Gate 1 — Normative authority and schema contract

- Write the ownership ADR and update timeless requirements/Allium for the complete target behavior.
- Define the single normalized root lifecycle authority, typed provenance rows, durable Close obligation/phases, and exact legacy backfill.
- Reconcile durable-turn integration with whatever roadmap slices have landed.
- Prove migrations and pure state/refinement models before external side effects.

**Exit:** specs validate; migration tests cover every legacy mapping table row; no unresolved product question remains in a spec.

### Gate 2 — Worktree-only ownership foundation

- Provision new/follow-up/derived project worktrees at detached canonical-default commits using single-branch best-effort fetch.
- Preserve legacy live checkouts without mutating them.
- Remove branch creation/deletion/rename from lifecycle and task approval; retain observation/PR association.
- Implement exact worktree-loss inventory and one-worktree-per-conversation isolation.

**Exit:** agents can switch/create/stack branches; lifecycle tests prove no branch/ref/PR mutation; legacy workspaces and parallel conversations are safe.

### Gate 3 — Durable Close, lifecycle backfill, and History-only deletion

- Backfill the normalized aggregate lifecycle.
- Implement idempotent close obligation, typed busy cancellation, fingerprinted loss confirmation, restart reconciliation, resource retirement, Closing-needs-repair, and whole-chain History finalization.
- Implement non-project Close and History-only idempotent deletion/tombstones.
- Keep temporary old-API adapters only where they cannot bypass safety.

**Exit:** deterministic failpoints cover every durable/external boundary; restart tests converge from every phase; no History aggregate owns resources; legacy and new rows list correctly through backend APIs.

### Gate 4 — One conversation presentation

- Switch list/navigation to Open/History aggregate projections.
- Render one stable route and seamless paginated transcript with exact persisted handoff boundaries.
- Update SSE/init/replay and generated types without creating transcript authority in the client.
- Redirect legacy member/chain links, then remove chain page/actions/routes when no caller depends on them.

**Exit:** browser/reconnect/restart journeys show one item/page, exact chronology and handoff, latest-row targeting, and read-only History.

### Gate 5 — Unified proposal placement and independent task context

- Replace mode-dependent approval/fork split with the one blocking review interaction.
- Implement Continue here default and Start in new conversation placement.
- Persist exact approved task source independently, provision fresh worktree, preserve taskmd/plain semantics, navigate to spawn, and leave source Open.

**Exit:** both placements are crash/retry safe; no approval branch side effect remains; spawned context contains the task and no source transcript.

### Gate 6 — Follow-up provenance and bounded retrieval

- Add Start follow-up, visible typed provenance, own-conversation search, explicit source search, exact-result navigation, and deleted-source behavior.
- Preserve Coordinator global-recall authority separately.

**Exit:** retrieval tests prove in-query scope, source opt-in, sibling/global exclusion, restart behavior, and FTS consistency after deletion.

### Gate 7 — Remove legacy authority and complete QA

- Remove old lifecycle/mode/branch ownership writers, endpoints, offline queue operations, chain UI/API surface, compatibility projections, obsolete generated types, and contradictory specs/tests.
- Deliberately retire or rewrite legacy `design.md` references under spEARS v2 rules; do not silently delete normative content.
- Run every journey and full validation.

**Exit:** repository search and tests show no user-facing or writable Archive/Clean up/Abandon/Mark merged/Work mode/Branch mode/chain-page authority; `./dev.py check --all` passes.

Child tasks are implementation units. This parent task remains Open until every gate and acceptance item is complete. If a child uncovers a genuine conflict between landed normative durable-turn behavior and this brief, resolve that product conflict explicitly before proceeding; ordinary code movement or implementation difficulty is not grounds to narrow scope.

## Acceptance evidence and completion mandate

**Drive this migration through full product completion.** Do not stop after specs, schema scaffolding, durable-turn aggregate work, compatibility projections, one backend endpoint, or new UI labels. Continue until the new model is authoritative across persistence, durable turns, state machines, worktree/Git handling, APIs, wire/SSE, restart recovery, retrieval, navigation, and UI.

If implementation is divided into child tasks, create and execute them in dependency order and continuously reconcile them against this brief. Do not mark this parent complete while any child, acceptance journey, migration, legacy-authority removal, generated type update, or contradictory normative rule remains outstanding. Treat failures crossing crates or frontend/backend boundaries as remaining scope, not follow-up suggestions.

Completion requires evidence that:

- every new project conversation gets a detached disposable worktree without mode/branch selection;
- default-branch provisioning uses a single-branch refresh, cached-ref fallback with truthful stale warning, and typed failure rather than arbitrary-ref guessing;
- one conversation has one worktree path, independent conversations never share it, and continuation transfers rather than duplicates the work scope;
- agents can freely use multiple branches/PRs and Close mutates none of them;
- proposal review always offers Continue here and Start in new conversation with the exact context/provenance behavior above;
- Continue here is the default placement and has no branch side effect; Start in new conversation persists the exact approved task independently, navigates to the new work, leaves the source Open, and copies no source transcript/summary;
- a continued conversation appears once and renders every durable transcript with the exact visible handoff summary;
- durable accepted turns, continuation boundaries, chronology, and active/latest targeting survive reload, reconnect, process restart, retries, and transcript pagination without duplication or reacceptance;
- own-conversation search spans all transcript rows, explicit source search is correctly bounded, and siblings/global data are excluded;
- busy Close cancels and settles through durable authorities before resource removal;
- one durable Close obligation survives retry/restart at every phase; changed workspace state invalidates stale loss confirmation; pre-destruction cancellation returns to Open; post-destruction retry converges or shows Closing needs repair;
- risky Close reports staged, unstaged/conflicted, untracked non-ignored, dirty-submodule, and reflog-only detached-commit risk independently; ignored output and unchanged LFS storage do not create noise; confirmation creates no recovery artifact;
- clean and risky Close both produce one read-only History item for the complete conversation;
- non-project conversations use the same Close/History taxonomy without fabricating worktree or Git ownership, while `/global` remains outside ordinary lifecycle controls;
- local/unpushed branches and all PRs survive unchanged;
- History alone offers Start follow-up and Delete permanently;
- permanent deletion removes all solely owned records and search projections without cascading to related conversations or repository artifacts;
- surviving provenance links to deleted History render typed Deleted source metadata and cannot be mistaken for absent provenance or silently retargeted;
- legacy API/offline operations cannot bypass Close safety or History-only deletion, and every temporary adapter is removed before completion;
- existing active, terminal, archived, continued, task-derived, and fork-derived data migrate without transcript, task, PR-association, or provenance loss;
- every row in the legacy mapping table has migration coverage, existing live worktrees are not rewritten, and no user-facing conversation has mixed Open/History transcript rows;
- no user-facing Archive, Clean up, Abandon, Mark merged, Work mode, Branch mode, chain block, chain page, or alternate member lifecycle remains;
- no parallel authority remains for lifecycle, turn identity, transcript content, worktree ownership, provenance, or retrieval scope;
- deterministic failpoints and restart tests cover Close before/after each durable write and external teardown step, continuation persist/broadcast cuts, task-spawn cuts, and delete/FTS cleanup cuts;
- SSE init/replay always prefers durable content/generation, never duplicates the exact handoff boundary, and exact message links remain stable through pagination;
- all affected Allium specs validate, codegen is current, focused migration/state-machine/property/restart/browser tests pass, and full `./dev.py check --all` passes.

## Risks to handle explicitly

- Durable direct-chat is active work in progress with major foundations landed; additional roadmap slices may land during this migration. Each child must re-ground and integrate at current typed boundaries while keeping unrelated roadmap scope isolated.
- Whole-conversation Close spans several durable rows and external resources, requiring explicit retry/recovery semantics without pretending filesystem teardown is one SQLite transaction.
- Detached-commit reachability must inspect all durable refs correctly.
- Existing task files and fork proposals assume Phoenix-created task branches; migration must preserve approved work without retaining branch ownership.
- Seamless long transcripts require pagination/virtualization that preserves exact IDs and boundaries.
- Removing chain UI must not remove linear topology needed by continuation, retrieval, latest-member targeting, and Coordinator references.
- Data migrations must normalize new queryable relations rather than reach into mutable JSON blobs or keep dual representations.

## Non-goals

- Automatically closing on PR merge or any external signal.
- Automatically merging, closing, retargeting, pushing, creating, or deleting branches/PRs during lifecycle actions.
- Automatically preserving workspace-only state on Close.
- Reopening or writing into History.
- Copying or automatically summarizing source transcripts into derived/follow-up conversations.
- Searching source, siblings, provenance families, or global history by default.
- Turning linear continuation chains into lifecycle trees.
- Making Coordinator recall ambient memory for ordinary conversations.
- Adding search/lineage controls to `propose_task`.
- Building arbitrary multi-conversation attachments before typed source links and bounded retrieval prove a broader need.
