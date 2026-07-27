Child of task 92018.

# Approved-task placement behavior across bedrock + projects

## Objective
Specify the exact approved-task placement behavior across the bedrock and projects spec cluster, limited to how approved work resumes in place versus starts in a separate conversation.

## Exact target artifacts
- `specs/bedrock/bedrock.allium`
- `specs/bedrock/requirements.md` only if a normative clarification is strictly required to match the settled facts already accepted in 92017
- `specs/projects/requirements.md`
- any existing `specs/projects/*.allium` files if they already own approved-task placement behavior
- read-only context from `tasks/92017-p1-done--terminology-authority-requirements-basel.md`
- read-only context from `tasks/92018-p1-in-progress--lifecycle-close-allium-behavior.md`

## Settled facts from 92017 / 92018 that this task MUST encode
- `Continue here` resolves approved-task review and resumes the same conversation context.
- `Continue here` has no lifecycle, provenance, `WorkScope`, worktree, branch, or PR side effect.
- `Start in new conversation` creates a separate Open conversation with fresh `WorkScope`/worktree, exact approved task only, and visible Derived from provenance.
- Follow-up is separate and fresh rather than a continuation of the origin.
- This task owns approved-task placement behavior only; it must not absorb root lifecycle topology, durable Close orchestration, WorkScope retirement classification, or seamless History/SSE projection.

## Required work contract
- Keep the contract focused on approved-task placement across bedrock + projects.
- Encode only settled placement behavior already grounded by 92017/92018 facts and current requirements/code reality.
- Do **not** add speculative review states, guessed UI flows, placeholder provenance helpers, or unrelated lifecycle/resource obligations.
- Run `allium check` on every edited `.allium` file and record exact commands/results in the evidence ledger.

## Out of scope
- `specs/durable-workflows/**`
- `specs/work-lifecycle/**`
- `specs/pr-association/**`
- `specs/sse_wire/**`
- code, ADRs, executive docs

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files changed** — exact paths changed
- **Settled facts encoded** — bullets mapping the task facts above to concrete rules/requirements
- **Validation** — exact `allium check` commands plus pass/fail output summary for each edited `.allium`
- **Review / evidence ledger** — self-review findings, reviewer findings, and any corrections made; write `None` if none
- **Speculation avoided** — explicit note that no speculative helpers/contracts/imports were added beyond the task contract
- **Commit** — commit hash that landed the work


## Completion evidence

**Files changed**
- `specs/bedrock/bedrock.allium`
- `specs/projects/projects.allium`
- `tasks/92028-p1-in-progress--approved-task-placement-behavior.md`

**Settled facts encoded**
- `propose_task` interception in `specs/bedrock/bedrock.allium` now uses one blocking `AwaitingTaskApproval` path for every Git-backed parent conversation; the writing-mode fork split was removed.
- `ProposeTaskInvalid` now covers the same validation boundary for every Git-backed parent shape, so there is no parallel proposal-validity authority.
- `UserApprovesTaskCurrentConversation` / `TaskApprovalExecuted` preserve Continue here as same-conversation resumption only: no continuation edge, no fresh conversation, no provenance side effect, and no worktree transfer authority.
- `UserApprovesTaskNewConversation` / `TaskApprovalNewConversationExecuted` define Start in new conversation as a separate Open parent conversation with fresh worktree/workscope boundary, exact approved-task-only starting context, and derived-from provenance through normalized `ApprovedTaskSource` snapshotting.
- `ApprovedTaskSource` types taskmd vs plain markdown explicitly and keeps the original repo-relative markdown path plus exact body/title/priority snapshot independent of source worktree lifetime.
- `ApprovedTaskPlacementsAreMutuallyExclusive` and `ApprovedTaskDerivedChildCarriesOnlyProvenanceLink` provide the requested structural self-review proof: Continue here and Start new cannot both be authoritative, and the derived child carries provenance without becoming a continuation.
- Obsolete `ForkProposal*` review/spawn/promotion behavior and the old `UserApprovesTaskFreshWorkConversation` continuation-like handoff behavior were removed from Allium.

**Validation**
- Command: `allium check specs/bedrock/bedrock.allium specs/projects/projects.allium`
- Result: exit `1` with info/warning diagnostics only; no `severity: "error"` diagnostics were emitted after the edits.

**Review / evidence ledger**
- Self-review: confirmed the two placements are mutually exclusive structurally because `approved_continue_current` and `approved_start_fresh` now dispatch distinct approval rules, while `ApprovedTaskPlacementsAreMutuallyExclusive` forbids both authorities simultaneously.
- Self-review: confirmed there is no parallel proposal authority left in Allium by removing `ForkProposalIntercepted` / `ForkProposalInvalid` / `ForkProposalReview` and replacing them with the single `ProposeTaskIntercepted` + `ProposeTaskInvalid` boundary.
- Self-review: confirmed Start in new conversation has no continuation edge because `UserApprovesTaskNewConversation` keeps `continued_in_conv_id = absent`, and the projects child-placement rule records provenance via `spawned_from_conversation_id` / `ApprovedTaskDerivedFromRecorded` instead.

**Speculation avoided**
- No requirements, code, ADR, executive, or unrelated lifecycle/workscope-close surfaces were changed.
- No provisioning internals, continuation boundary invention, or extra review state machine beyond the requested blocking review / two placement outcomes was added.

**Commit**
- Pending


## Reopen review evidence

**Files changed**
- `specs/bedrock/bedrock.allium`
- `specs/projects/projects.allium`
- `tasks/92028-p1-in-progress--approved-task-placement-behavior.md`

**Settled facts encoded**
- `specs/bedrock/bedrock.allium` now defines a single blocking `propose_task` review authority for Git-backed parent conversations, gated by Git-backed/worktree capability or attached WorkScope rather than old mode splits.
- Bedrock approval decisions now distinguish `approved_continue_current`, `approved_start_fresh`, `request_changes`, and `rejected`, so request-changes vs reject semantics are explicit and the old `feedback_provided` branch is gone.
- `UserApprovesTaskNewConversation` now emits only `FreshGitWorktreeProvisioningRequested(...)` for the fresh target and no longer writes work-mode/branch/base-branch/continuation/handoff state at approval time.
- `specs/projects/projects.allium` removes task-fork authority from proposal placement: no nonblocking fork rules/entities, no writing-mode fork split, and no approval-time branch creation/deletion side effects.
- `ApprovedTaskSource` now preserves the exact approved body, repo-relative path bookkeeping (`task_path`), and typed identity (`ApprovedTaskIdentity`) independent from source lifetime.
- Start-in-new placement is now owned by the target `ProductConversation`: it records the approved source on the target aggregate, emits the root `approved_task` provenance edge from source product conversation to target product conversation, keeps the source product conversation `open`, and forbids any continuation edge.
- Continue-here placement remains same aggregate/context/scope only, with objective/approval-state changes but no provenance, branch mutation, or worktree/provisioning side effect.

**Validation**
- Command: `allium check specs/bedrock/bedrock.allium specs/projects/projects.allium`
- Result: exit `1` with warnings/info only after the reopen edits; no `severity: "error"` diagnostics remained.

**Review / evidence ledger**
- Restored the prior completion evidence from commit `afe95432` and appended this reopen review section rather than replacing the historical note.
- Repository-wide semantic grep (scoped to proposal symbols and branch side effects) confirmed removal of competing proposal authorities from the edited specs: no surviving `ForkProposal`, `UserApprovesTaskFreshWorkConversation`, `feedback_provided`, or approval-time `fork_branch_name` usage in `bedrock.allium` / `projects.allium`.
- Review correction: the prior version still let start-fresh approval allocate `mode = work`, `branch_name`, `base_branch`, `worktree_path`, and `spawned_from_conversation_id` at approval time. This reopen pass replaced that with a typed fresh-provisioning boundary plus target-owned approved-task source recording.
- Review correction: prior prose still described direct-mode/nonblocking fork proposal authority in `projects.allium`; this reopen pass replaced it with explicit disjointness so branch picker / provisioning rules cannot become alternate product proposal paths.
- Structural proof captured by invariants and rule shape:
  - exact one placement per approved proposal (`ApprovedTaskPlacementsAreMutuallyExclusive`)
  - one spawn winner on retries (`ApprovedTaskFreshPlacementHasOneSpawnWinner`)
  - no continuation edge / source stays open (`ApprovedTaskFreshPlacementCarriesNoContinuationEdge`)
  - no approval-time branch mutation for start-fresh placement (fresh placement emits only `FreshGitWorktreeProvisioningRequested`)

**Speculation avoided**
- No requirements were changed because this reopen was reconciled within existing Allium ownership.
- No unrelated branch-picker, restart reconciliation, terminal cleanup, or downstream provisioning mechanics were rewritten beyond adding disjointness / removing proposal-flow coupling.

**Commit**
- Pending


## Final correction evidence

**Files changed**
- `specs/bedrock/bedrock.allium`
- `specs/projects/projects.allium`
- `tasks/92028-p1-in-progress--approved-task-placement-behavior.md`

**Settled facts encoded**
- Bedrock `propose_task` eligibility now flows through the declared `conversation.task_proposal_eligible` predicate rather than repeating a hidden `mode in { explore, work, branch }` gate at each proposal rule.
- That predicate is Git-backed by construction: `is_git_repository(working_dir)` and `(has_git_work_intent or has_attached_work_scope(this))`, so chat-only/direct conversations are excluded unless they carry attached work scope.
- Projects no longer contains `TaskApprovalStarted(...)` or any approval-time branch-materialization rule; approval never fast-forwards, creates, or materializes a branch.
- `TaskFile` comments now describe the exact reviewed file as already present in the current worktree before approval, with start-fresh persistence owned independently by `ApprovedTaskSource`.
- Proposal-neighborhood comments no longer claim approved/rejected tasks live on a task branch or temp branch during approval review.
- Start-fresh approval now requests provisioning from `canonical_default_identity(conversation.project.repo, conversation.project.main_ref)`, making the semantic target the repository canonical default identity rather than a legacy current-branch fallback path.
- Branch-picker rules were left intact, and the `BranchPicker` surface guarantee now states proposal / approved-task placement has no path to that surface.

**Validation**
- Command: `allium check specs/bedrock/bedrock.allium specs/projects/projects.allium`
- Result: exit `1`; diagnostics remained `info` only (no `severity: "error"` output).

**Review / evidence ledger**
- Exact semantic grep command:
  - `rg -n "TaskApprovalStarted|fork_branch_name|ForkProposal|propose_task.*mode in|mode in \{ explore, work, branch \}.*propose_task|task branch|temp branch|BranchDeleted\(" specs/bedrock/bedrock.allium specs/projects/projects.allium`
- Exact output:
  - `specs/projects/projects.allium:615:            BranchDeleted(worktree.branch_name)`
  - `specs/projects/projects.allium:666:    ensures: BranchDeleted(worktree.branch_name)`
- Retained matches rationale:
  - `BranchDeleted(worktree.branch_name)` at lines 615 and 666 is unrelated to approval/rejection flow; both are terminal cleanup rules (`WorktreeRemovedByConversationDelete`, `ExploreWorktreeCleanupOnTerminal`) outside task approval / rejection semantics.

**Speculation avoided**
- No unrelated `UserSelectsBranch` rules were altered.
- No typed provisioning internals were invented beyond changing the start-fresh semantic target to canonical default identity.

**Commit**
- Pending
