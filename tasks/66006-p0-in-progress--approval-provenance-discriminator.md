# Distinguish first approval from follow-up by durable approved-objective provenance

## Immediate objective

Repair PR #765’s still-open initial-versus-follow-up approval bypass and prove the supported same-owner recovery for owner `b8876b92-5db4-40fa-9a26-1fafc848977c` / WorkScope `f659e833-a1aa-41bf-8f47-388fba693773` without replaying approval, editing production state, replacing the owner, or disturbing task 06009’s accepted artifact and commits.

Implement this as the next early coherent commit on branch `task-66005-atomic-explore-work-capability-transition` / PR #765. No merge or deployment is authorized; Global retains supervision of user turns and sole merge/deploy authority.

## Observed journey and preserved facts

- Deployed SHA `02488dde1` and PR HEAD `934354ecc5adf908069eddaa0ae9ec7873892d80` both route `ApproveTask` through `approve_follow_up_in_existing_scope` whenever `resource_authority == Work && work_scope_worktree.is_some()`.
- That predicate describes an allocated capability/environment, not whether initial approval has already established a durable approved objective.
- At about `2026-09-27T20:46:27Z`, `/approve-task` returned 200 for task 06009 and execution resumed, but the initial path was bypassed. Git rebase was denied, pathname patch stayed task-Markdown-only, and Work-child spawning still saw the parent as read-only.
- Read-only production inspection confirms:
  - conversation mode remains `detached_product_creation` and state is now `idle`;
  - WorkScope `f659…` is active with `authority_kind='work'` and the original allocated worktree `.phoenix/worktrees/11c02077-36b3-4b2b-8870-aa4e0f90ab2a`;
  - approved objective task `06009` is durably bound to owner `b887…` and that scope;
  - the accepted ready-task identity remains stored; existing commits `608614d8e` and `dc6f8fed1` above `924fb0b` must be preserved.
- The durable transition therefore succeeded, but the wrong approval branch skipped initial live capability installation.
- Current PR HEAD does **not** fix how this state is reached. Merging `934354ecc` alone is insufficient to prevent another first approval from taking the follow-up branch.
- Current PR HEAD **does** contain the required reconstruction projection: `DetachedProductCreation + Work authority` rematerializes with `git_backed_writing_parent`, including ordinary Work Bash/Git/patch and Work-child admission. Deployed `02488dde1` instead restricted this mode despite a durable objective.

## Owning invariant

First approval versus follow-up approval is determined by durable approval provenance, not by present resource capability, worktree allocation, or conversation mode:

- no approved objective currently granting the attached WorkScope → initial approval/reconciliation path;
- an approved objective currently granting the attached WorkScope → follow-up approval path.

The existing `Database::get_approved_task_objective` query already expresses that scope-bound durable fact through `conversation_approved_task_objectives` and `work_scope_approved_task_authorities`. Work authority and objective provenance are deliberately separate schema facts; one cannot stand in for the other.

## Smallest shippable implementation

1. Expose the scope-bound approved-objective lookup through the executor’s storage abstraction and its database/in-memory implementations. Prefer a typed approval-lifecycle decision rather than another Boolean whose meaning can drift.
2. Replace both uses of `has_existing_write_scope` in `ConversationRuntime`:
   - the `execute_approve_task` initial/follow-up branch;
   - effect-loop bookkeeping that treats `ApproveTask` as atomically committing post-approval state.
   Compute/use one durable lifecycle decision for the approval operation so routing and persistence bookkeeping cannot disagree.
3. Preserve the initial path’s existing early-worktree/retry behavior. A detached product-creation owner may already have Work authority and an allocated worktree before first approval; absence of a durable objective must still select the initial path, preserve existing commits/task identity, establish objective/state atomically, and publish the complete live Work capability.
4. Preserve genuine follow-up behavior when an objective already grants the scope: operate in the existing worktree, promote/commit the newly reviewed artifact, replace the objective, and do not create a new scope or owner.
5. Keep mode unchanged per REQ-BED-028. Do not invent a mode special case, rewrite `DetachedProductCreation`, or add a parallel approval flag.
6. Retain current PR reconstruction logic for the durable incident state. If implementation exposes an objective snapshot already loaded during materialization, ensure registry selection still requires the canonical Work authority and remains structurally consistent with that objective provenance.
7. Commit and push this causal slice early on the same PR branch with an explicit lease before proceeding to unrelated review findings.

## Deterministic regressions

### First approval despite preallocated Work capability

Construct the exact counterexample:

1. `DetachedProductCreation` (and, where useful, Explore provenance coverage).
2. Active attached WorkScope with Work authority and allocated existing worktree.
3. Existing accepted task artifact/approval-only Git history as appropriate for the early-worktree retry path.
4. No durable approved objective granting the scope.
5. Approve in the current conversation.

Assert:

- initial approval/reconciliation is selected, never `approve_follow_up_in_existing_scope`;
- accepted task identity/body and pre-existing commits are preserved;
- task promotion/commit is not duplicated or overwritten;
- objective, approval message/obligation, Work authority, and `LlmRequesting` state settle atomically;
- the same live actor receives ordinary Work Git/Bash/patch and may admit an attached Work child;
- conversation mode, owner, WorkScope, worktree, and provenance remain unchanged.

### Genuine follow-up

Use the same mode, capability, and worktree shape but seed a durable objective granting the attached scope. Assert follow-up approval preserves the existing branch/worktree, promotes the reviewed follow-up artifact, replaces the objective, and does not run initial approval provisioning. Update existing follow-up failure tests to seed prior objective provenance when they claim to exercise follow-up behavior.

### Durable recovery/rematerialization

Persist the incident shape directly through supported storage APIs:

- `DetachedProductCreation` mode;
- active Work WorkScope plus allocated worktree;
- durable approved objective bound to the same scope;
- idle or interrupted recoverable state.

Rematerialize the same conversation and assert the complete authority projection, not only registry-name helpers:

- ordinary Work Bash rather than sandboxed Explore Bash;
- Git and pathname patch available;
- Work-child admission succeeds for the inherited scope;
- no approval replay, duplicate task mutation, new conversation, or new worktree occurs.

Also retain a negative reconstruction case: the same mode/worktree without Work authority/objective remains Restricted.

## Supported same-owner recovery after landing

Do not call `/approve-task` again: the owner is no longer awaiting approval, and approval is not a public idempotent repair command. Do not use `/continue`, cancel, model upgrade, manual database writes, source/Git workarounds, a replacement owner, or copied files.

After Global merges and separately authorizes/deploys the qualified PR, the deployment’s ordinary process replacement plus reconnecting/opening the same conversation stream is sufficient: the stream path calls `get_or_create` for non-terminal states, and PR #765’s materializer reconstructs `DetachedProductCreation + persisted Work authority` as a writing parent. No additional restart or lifecycle mutation is required. Before allowing further task work, run bounded, non-destructive same-owner probes that establish:

1. the registry exposes ordinary Work Bash/Git/patch;
2. Bash is not using Explore scratch confinement and can perform a harmless create/remove in the worktree/Git common-dir boundary;
3. attached Work-child admission succeeds against WorkScope `f659…`.

If any probe fails, stop and preserve the owner/scope/worktree; do not improvise another recovery path.

## Validation and delivery

- Focused executor tests for initial and follow-up routing, including failure/retry bookkeeping.
- Runtime/materialization integration test for the exact detached persisted recovery shape.
- Database/storage tests for the scope-bound objective discriminator.
- Affected-crate `cargo check`, tests, and clippy; `cargo fmt --all`; `git diff --check`.
- Rebase once onto exact current `origin/main`, then run the complete gate only once when needed; classify common harness flakes against their existing owners rather than expanding this PR.
- Push same PR branch with explicit lease, request fresh exact-head Codex, wait for hosted CI, and paginate review cursors to exhaustion before qualification.

## Separate current PR findings — not part of this causal slice

Keep these independently actionable and unresolved while the urgent approval commit lands:

1. **Platform-temp grant race** in `crates/phoenix-tools/src/bash/sandbox.rs`: attached-child confined Bash independently resolves/grants platform temp after worktree validation, permitting a symlink replacement to widen write access. Handle as a separate coherent authority-hardening commit or establish it as a duplicate with an existing owner.
2. **Destination retirement-fence race** in `crates/phoenix-browser/src/session.rs`: a completed reopen permit can be removed after Stop retires an empty destination key, allowing later promotion unless retirement generation/tombstone authority survives. Handle separately after the approval fix.

Neither finding changes the durable approval discriminator or the supported task-06009 recovery action. Both still block claiming PR #765 qualified until resolved or conclusively deduplicated.

## Explicit non-goals

- No task-06009 implementation in this scope.
- No production mutation, merge, deployment, process restart, continuation, cancellation, or user-turn supervision.
- No new owner, conversation, WorkScope, worktree, task reduction, or copied source files.
- No raw database repair or Git workaround.
- No Global Coordinator `present_svg`, subscription work, permission menu, or common tmux/test-harness fix.
