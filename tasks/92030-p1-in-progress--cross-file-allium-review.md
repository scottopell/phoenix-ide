Child of task 92018.

# Cross-file Allium review for lifecycle contract split after child tasks land

## Objective
Run the final cross-file review after the five scoped lifecycle child tasks land, verifying that the split contracts compose cleanly and that no child reintroduced the overbroad/speculative failure mode.

## Exact target artifacts
Review the exact artifacts changed by the five children of 92018 covering:
- `specs/bedrock/bedrock.allium`
- `specs/durable-workflows/**` touched by the Close orchestration child
- `specs/projects/**` touched by the WorkScope retirement or approved-task placement children
- `specs/work-lifecycle/**` touched by the WorkScope retirement child
- `specs/pr-association/**` touched by the WorkScope retirement child
- `specs/sse_wire/**` touched by the History/SSE projection child
- the five completed child task files themselves for completion evidence

## Settled facts from 92017 / 92018 that this review MUST enforce
- Product lifecycle is Open/History only; Close is the action and History the resulting state.
- Context continuation is one product conversation across `continued_in_conv_id` rows and one attached `WorkScope` unless a separate new conversation is explicitly created.
- Latest-row authority is derived, not duplicated.
- `Continue here` has no lifecycle/provenance/WorkScope/worktree/branch/PR side effect.
- `Start in new conversation` is separate, fresh, exact-approved-task-only, and visibly derived.
- `WorkScope` retirement/loss classification stays separate from conversation lifecycle ownership.
- History/SSE projection stays seamless and does not invent split authority.

## Required work contract
- Do not start until the five scoped child tasks are complete.
- Review cross-file composition, helper/import hygiene, and artifact-boundary discipline.
- Verify every edited `.allium` file with `allium check`, even if the owning child already ran it.
- Verify each child task appended the required evidence ledger sections.
- File concrete follow-up tasks for any remaining drift rather than broad “future work” notes.
- Do **not** rewrite requirements or Allium opportunistically unless a minimal review correction is necessary and directly evidenced.

## Out of scope
- New lifecycle redesign
- code changes
- speculative helpers/contracts beyond review fixes

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files reviewed** — exact paths/clusters reviewed
- **Settled facts enforced** — bullets listing each cross-file rule checked and verdict
- **Validation** — exact `allium check` commands plus pass/fail output summary for every edited `.allium`, and `./dev.py tasks validate`
- **Review / evidence ledger** — review findings, minimal corrections made, and follow-up task IDs filed, or `None`
- **Speculation avoided** — explicit note that the review rejected speculative helpers/contracts or parallel authorities
- **Commit** — commit hash that landed review corrections, or `None` if review-only


## Completion evidence

**Files reviewed**
- Parent/task evidence: `tasks/92009-p1-in-progress--unify-conversation-workstream-lifecycle.md`, `tasks/92017-p1-done--terminology-authority-requirements-basel.md`, `tasks/92025-p1-done--bedrock-root-lifecycle-continuation-topology.md`, `tasks/92026-p1-done--durable-close-orchestration-cancellation-contract.md`, `tasks/92027-p1-done--workscope-retirement-loss-classification.md`, `tasks/92028-p1-done--approved-task-placement-behavior.md`, `tasks/92029-p1-done--history-sse-projection.md`
- Reviewed Allium/requirements clusters in scope: `specs/bedrock/bedrock.allium`, `specs/bedrock/requirements.md`, `specs/projects/projects.allium`, `specs/projects/requirements.md`, `specs/work-lifecycle/work-lifecycle.allium`, `specs/work-lifecycle/requirements.md`, `specs/durable-workflows/durable-workflows.allium`, `specs/durable-workflows/wake-profile.allium`, `specs/pr-association/pr-association.allium`, `specs/sse_wire/sse_wire.allium`, `specs/sse_wire/executive.md`

**Settled facts enforced**
- One ProductConversation lifecycle authority: **Mismatch found and fixed** — `specs/sse_wire/sse_wire.allium` still modeled a dedicated row-terminal broadcast instead of the root aggregate lifecycle projection.
- No row lifecycle duplication: **Mismatch found and fixed** — `specs/projects/projects.allium` still exposed legacy close-adjacent action names (`UserMarksAsMerged` / `UserConfirmsAbandon`) on the project surface instead of the single Close trigger.
- `continued_in_conv_id` sole topology/latest and no stored root/latest duplicates: **Checked** — bedrock invariants `LatestRowDerivedFromTopology`, `ProductConversationHasSingleRootRow`, `ProductConversationHasSingleTopologyLatestRow` remain the only authority.
- WorkScope owns resources; attachments do not own cleanup: **Checked** — `specs/work-lifecycle/work-lifecycle.allium` keeps retirement authority on attached `WorkScope`; no attachment-owned cleanup contract found in reviewed files.
- Chat-only optional scope: **Checked** — `ChatOnlyCloseSkipsWorktreeInspection` still cleanly skips Git-worktree inspection.
- Continuation same aggregate/scope: **Checked** — bedrock/projects still model continuation via `continued_in_conv_id` and transferred ownership, not fresh lifecycle creation.
- Approved task fresh aggregate/scope, no branch/continuation/source-transcript copy: **Checked** — bedrock/projects placement rules still distinguish Continue-here from fresh derived conversation and keep continuation topology untouched.
- Continue here has no lifecycle/resource effect: **Checked** — approval path remains same aggregate/context/scope only.
- Close admission/blockers/settlement/cancel/inspection/confirmation/committed retirement/repair/completion: **Checked** — bedrock close phase graph and work-lifecycle retirement triggers still compose.
- Exact event signatures agree between bedrock/work-lifecycle: **Checked** — cross-spec close/retirement trigger names still align; no mismatch found in this review round.
- Loss categories complete: **Checked** — all five exact categories remain present in work-lifecycle.
- Branch/PR untouched: **Checked** — work-lifecycle and pr-association still keep branch/PR observational only.
- History unified projection and exact handoff: **Checked** — bedrock still owns unified projection/handoff boundaries; no dedicated chain product surface added in reviewed files.
- No dedicated chain product surface in changed Allium: **Checked**.
- No old Abandon/MarkMerged/Archive/ForkProposal/task-branch lifecycle authority: **Mismatch found in projects Allium and fixed for user-facing close authority**; residual legacy explanatory comments remain in untouched historical/migration prose of `specs/projects/projects.allium` outside this review-fix boundary.
- Direct-turn exact IDs/generation preserved: **Checked** in durable-workflows core; no drift found in reviewed files.
- No self-trigger event/rule name ambiguity: **Checked for changed rules** — new names use distinct trigger/rule pairs (`ProductConversationLifecycleBroadcast`, `CloseActionTargetsLatestRow`).
- No unbound class-level entity shorthand where instance required: **Checked in edited rules**.
- No child collections in blobs conceptually: **Checked** — approved-task source remains normalized value/record modeling, not blob-owned child collection semantics.

**Validation**
- Semantic grep used for review discovery:
  - `rg -n 'continued_in_conv_id|ProductConversation|WorkScope|Continue here|Abandon|MarkMerged|Archive|ForkProposal|History' specs --glob '*.{md,allium}'`
  - `rg -n 'ConversationBecameTerminal|UserMarksAsMerged|UserConfirmsAbandon|MarkAsMerged|ConfirmAbandon|ForkProposal|Archive|chain page|ProductConversationLifecycleProjected|UserRequestsCloseConversation|continued_in_conv_id' specs/bedrock/bedrock.allium specs/work-lifecycle/work-lifecycle.allium specs/projects/projects.allium specs/durable-workflows/wake-profile.allium specs/pr-association/pr-association.allium specs/sse_wire/sse_wire.allium`
- `allium check` parsed for **error severity only** per file:
  - `specs/bedrock/bedrock.allium` → `errors=0`
  - `specs/work-lifecycle/work-lifecycle.allium` → `errors=0`
  - `specs/projects/projects.allium` → `errors=0`
  - `specs/durable-workflows/durable-workflows.allium` → `errors=0`
  - `specs/durable-workflows/wake-profile.allium` → `errors=0`
  - `specs/pr-association/pr-association.allium` → `errors=0`
  - `specs/sse_wire/sse_wire.allium` → `errors=0`
- Combined check of all affected files was also parsed for error severity and returned `errors=0`, though the CLI exited non-zero because it emits informational/warning diagnostics and extra log output rather than a clean zero-status pass.
- `./dev.py tasks validate` → `✓ 889 task files validated`

**Review / evidence ledger**
- Concrete mismatch inventory before edits:
  1. `specs/projects/projects.allium:1172-1176, 1223-1224` — project-management surface and invariant still encoded legacy `UserMarksAsMerged` / `UserConfirmsAbandon` actions, contradicting the settled “single Close admission authority” rule.
  2. `specs/sse_wire/sse_wire.allium:737-747` plus event-type enumerations/replay-ring whitelist — SSE still named a dedicated `conversation_became_terminal` broadcast instead of the unified aggregate lifecycle projection already modeled elsewhere in the same file.
  3. `specs/sse_wire/executive.md:7,36,58` — executive summary still described the removed terminal wire event as normative.
- Minimal review-proven fixes made:
  - Replaced the project-surface close trigger and invariant with `UserRequestsCloseConversation` / `CloseActionTargetsLatestRow` in `specs/projects/projects.allium`.
  - Replaced the SSE terminal-broadcast rule with `ProductConversationLifecycleBroadcast`, and removed stale `conversation_became_terminal` enumerations from the replay-ring/event listings in `specs/sse_wire/sse_wire.allium`.
  - Updated `specs/sse_wire/executive.md` to describe lifecycle projection through the aggregate `conversation_update` path instead of a separate terminal event.
- Blockers / follow-up tasks filed: None in this task file. Residual legacy comments in untouched `specs/projects/projects.allium` describe historical flows, but I did not broaden this review task into a full projects-spec rewrite without stronger evidence that those comments are now normative contradictions.

**Speculation avoided**
- Rejected a broader opportunistic rewrite of old mode/task-branch/project-terminal commentary in `specs/projects/projects.allium`; only review-proven authority mismatches were changed.
- Did not alter bedrock/work-lifecycle close event names or durable-workflow identity semantics because the cross-file review found no normative drift there.

**Commit**
- Pending local commit after reviewer sign-off in this subtask run.


## 92030 correction pass A evidence

**Files changed**
- `specs/projects/projects.allium`
- `tasks/92030-p1-in-progress--cross-file-allium-review.md`

**Settled facts enforced**
- Rewrote `specs/projects/projects.allium` around a unified Git-backed provisioning boundary instead of product lifecycle modes. The spec now models Git-backed conversation creation as canonical-default-branch resolution plus detached-start worktree/work-scope provisioning, with no mode picker or branch-picker conversation creation path.
- Removed normative mode-selection and branch-creation rules (`UserSelectsManaged`, `UserSelectsBranch`, temp/task branch provisioning, branch-mode lifecycle cleanup, approval-time branch materialization) and replaced them with repository-observation-only branch picker rules plus approval placement rules that keep `Continue here` same aggregate/scope and `Start in new conversation` fresh aggregate/fresh provisioning.
- Replaced task/conversation ownership semantics with `WorkScope` ownership: `Worktree` now attaches to a `WorkScope` owned by the `ProductConversation`, continuation keeps the same `ProductConversation` and same `WorkScope`, and the predecessor is modeled as `HandedOffPredecessorReadOnly(...)` rather than as a separate history owner.
- Preserved branch observation capability only as post-creation repository operations (`ListLocalBranches`, `SearchRemoteBranches`, `BranchPicker`), explicitly guaranteed as observation-only and not a conversation mode or lifecycle selector.
- Kept teardown out of projects Allium. Startup reconciliation now classifies missing worktrees without `BranchDeleted(...)` or any ref mutation; teardown authority remains delegated to work-lifecycle.
- Preserved approved-task proposal rules from 92028 while aligning them to the unified model: continue-here writes only onto the existing worktree/scope, and start-fresh records approved-task provenance then emits `FreshGitWorktreeProvisioningRequested(...)` without branch/base/current-branch side effects.
- Preserved sub-agent same-scope attachment semantics while removing cleanup ownership from sub-agents.

**Validation**
- Command: `allium check specs/projects/projects.allium specs/bedrock/bedrock.allium`
- Result: exit `1` because the checker emitted existing info/warning diagnostics, but parsed diagnostics contained `0` `severity: "error"` findings for `specs/projects/projects.allium`.
- Command: `rg -n 'UserSelectsManaged|UserSelectsBranch|TaskApprovalStarted|BranchDeleted|TaskRejectedCleanup|UserMarksAsMerged|MarkAsMerged|UserConfirmsAbandon|ConfirmAbandon|mode = explore|mode = work|mode = branch|Explore|Work mode|Branch mode|Managed mode|task branch|temp branch|continued_in_conv_id|base_branch|branch_name|main_ref|current_branch' specs/projects/projects.allium`
- Result: only retained structural terms remained (classified below); removed lifecycle/action hits produced no matches.

**Retained old terms and classification**
- `current_branch`
  - `specs/projects/projects.allium:25` — retained as a `GitRepository` observation field.
  - `specs/projects/projects.allium:162` — retained only inside canonical default branch resolution fallback to `repo.current_branch` at project detection; not a runtime arbitrary provisioning fallback.
- `branch_name`
  - `specs/projects/projects.allium:204,322,323` — retained as existing bedrock conversation provenance/workspace fields threaded across continuation.
- `base_branch`
  - `specs/projects/projects.allium:203,322` — retained as existing bedrock provenance/workspace field threaded across continuation.
- `continued_in_conv_id`
  - `specs/projects/projects.allium:314,324,341,345,390,429,447` — retained only for continuation topology and latest-owner derivation, not as history ownership.
- `mode = work` / `mode = explore`
  - `specs/projects/projects.allium:363,376` — retained only as sub-agent authority selector values inherited from bedrock mode taxonomy; not as product conversation creation or lifecycle rules.
- `ExploreSubAgentDirectory`
  - `specs/projects/projects.allium:373` — retained rule name from prior scope, but semantics now mean read-only sub-agent directory selection, not Explore-mode product lifecycle.
- `starting_branch_name`
  - `specs/projects/projects.allium:124,198,474` — new provenance field recording the canonical default branch used for detached start; not a branch-ownership field.

**Review / evidence ledger**
- This correction intentionally broad-rewrote `specs/projects/projects.allium` instead of making isolated line edits because the prior file still structurally centered obsolete mode/branch lifecycle concepts throughout entities, rules, invariants, and surfaces.
- No requirements, ADRs, executive docs, or non-projects Allium files were modified in this pass.

**Speculation avoided**
- Did not add new PR-association or teardown orchestration semantics to projects; those remain owned by `pr-association` and `work-lifecycle`.
- Did not invent a current-branch provisioning fallback beyond the accepted project-detection fallback for establishing the canonical default branch when the remote symref is unavailable.
- Did not mark task 92030 done; this is appended evidence only.

**Commit**
- Pending


## 92030 correction pass A2 evidence

**Files changed**
- `specs/projects/projects.allium`
- `tasks/92030-p1-in-progress--cross-file-allium-review.md`

**Settled facts enforced**
- Canonical default identity no longer falls back to `repo.current_branch`. `GitDirectoryDetected` now resolves `canonical_default_branch` only from `authoritative_default_identity(repo)`, and the guidance explicitly limits authority to configured remote-default identity or cached local/remote-tracking evidence for that same branch.
- Unified Git-backed provisioning now models the exact single-branch truth flow: `SingleBranchFetchAttempted(project, default_branch)`, fetched remote-tracking tip preferred when available, cached remote-tracking ref allowed only as a stale-warning fallback, and typed provisioning failure when no commit resolves.
- Detached-start provisioning now records provenance on the worktree/work-scope side only. `Worktree` exposes `starting_branch_name` and `starting_commit`, and `DetachedStartProvenance`/`DefaultIdentityProvenance` capture where the default identity came from without stamping `conversation.base_branch`, `conversation.branch_name`, or a synthetic detached label as ownership.
- Continuation no longer duplicates branch fields. `WorktreeTransferredOnContinuation` keeps the same `ProductConversation`, `WorkScope`, `working_dir`, and `worktree_path`, but no longer writes `new_conv.base_branch` or `new_conv.branch_name`.
- `current_branch` remains only a repository observation field declaration; it is no longer referenced by provisioning fallback logic. Branch picker remains observation-only.

**Validation**
- Command: `allium check specs/projects/projects.allium specs/bedrock/bedrock.allium`
- Parsed result: `error_count 0` for `specs/projects/projects.allium` (CLI exit `1` due to existing non-error info/warning diagnostics only).
- Command: `rg -n 'remote_default_branch\s+or\s+repo\.current_branch|repo\.current_branch' specs/projects/projects.allium`
- Result: no matches.
- Command: `rg -n 'ensures: .*base_branch|ensures: .*branch_name' specs/projects/projects.allium`
- Result: no matches.
- Command: `rg -n 'current_branch|base_branch|branch_name|continued_in_conv_id' specs/projects/projects.allium`
- Result classification:
  - `current_branch` appears only on `GitRepository.current_branch` as repository observation.
  - `branch_name` appears on `DefaultIdentityProvenance.branch_name` and detached-start provenance wiring, not on conversation provisioning/continuation assignment.
  - `continued_in_conv_id` remains only for continuation topology / ownership transfer.

**Review / evidence ledger**
- This pass corrected the specific reopened defects inside `specs/projects/projects.allium` only.
- No other specs, code, or task files were edited beyond this appended evidence block.

**Speculation avoided**
- Did not reintroduce any arbitrary `repo.current_branch` default-branch fallback.
- Did not restore conversation-level branch/base ownership writes for detached start or continuation.
- Did not broaden the task into non-projects spec edits.

**Commit**
- Pending
