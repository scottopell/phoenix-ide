# Reclaim terminal continuation-chain worktrees

## Problem

Phoenix retains worktrees after the final live conversation in a continuation chain reaches a terminal lifecycle state. Production currently has 38 registered Phoenix worktrees with no intended live owner, consuming 81.33 GiB.

The reported `commission-review-feedback` chain is representative:

- `4379884a…` — Explore `HandedOff` → `365820bb…`
- `365820bb…` — Work `ContextExhausted` → `2d147b37…`
- `2d147b37…` — Work `Terminal`
- all three rows reference the ancestor path `…/.phoenix/worktrees/4379884a…`
- the directory remains Git-registered and consumes 31 GiB, almost entirely `target/`

## Root cause

Commit `52bd98c7` (PR #265) added a “dead-end protector” rule to `RuntimeManager::conv_is_scope_owner`: a `HandedOff` predecessor reacquires ownership when no downstream continuation remains live. During abandon, mark-merged, archive, or hard-delete of the leaf, `scope_has_live_conversation_excluding` therefore reports the scope as still owned and `cascade_projects_on_delete` skips worktree and branch cleanup.

This overgeneralized a valid data-loss safeguard. An uncontinued `ContextExhausted` row must preserve its worktree pending Continue/Abandon/Mark-as-merged. A `HandedOff` row is read-only and has permanently transferred ownership to its successor; it must never reacquire ownership when the successor chain terminates. The current behavior conflicts with REQ-PROJ-015 and REQ-BED-032, which require cleanup when no other live conversation owns the scope.

A related latent defect exists in `RuntimeExecutor::cleanup_worktree_if_present`: it reconstructs `…/.phoenix/worktrees/{current_conversation_id}` instead of resolving the conversation's canonical `WorkScope`. A continued Work/Branch/top-level-Explore leaf therefore probes the wrong path on non-lifecycle terminal transitions. The cleanup boundary should consume the typed scope: remove exactly the path carried by `WorkScope::Worktree(path)`; perform no filesystem worktree cleanup for `WorkScope::Conversation(id)` (Direct and Explore sub-agents, even when their cwd is another conversation's worktree) or `WorkScope::Global`. This preserves the existing distinction between a transcript ID and the durable unit of work instead of introducing another path derivation.

Startup `reconcile_worktrees` cannot repair either leak. It only handles database rows whose directories are already missing; existing Git-registered directories with entirely terminal owner chains are ignored.

## Measured production impact

Read-only inventory against `~/.phoenix-ide/prod.db` and the filesystem:

- 127 distinct historical worktree paths in the database.
- 53 database-recorded paths currently exist on disk.
- 38 existing paths have no non-archived live owner under the normative ownership rule.
- Those 38 consume 81.33 GiB.
- 36 paths / 81.27 GiB have a `HandedOff` ancestor and match this regression directly.
- 2 paths / 0.06 GiB are older archived-idle residue unrelated to handoff.
- All 38 are still registered by Git.
- 3 stale trees have untracked files; remediation must inventory/preserve evidence before forced removal.
- 15 intended-live paths consume another 75.35 GiB and must not be touched.

The reported tree is clean, its task branch is not an ancestor of local `main`, and it consumes 31 GiB (`target/` alone accounts for approximately 31 GiB). Branch disposition must follow the recorded lifecycle/mode rather than inferring safety from local merge ancestry.

## Implementation plan

1. **Correct scope ownership.** Make `HandedOff` always cede ownership to its continuation chain. It may cause a scope to remain live only through an actually live downstream member; it must not become a dead-end protector. Keep uncontinued `ContextExhausted` protection unchanged.
2. **Make `WorkScope` the cleanup authority.** Resolve scope once from the conversation ID plus the persisted `ConvMode::worktree_path()` / runtime's synchronized `work_scope_worktree`, then thread that typed value through liveness and cleanup. `WorkScope::Worktree(path)` is the only variant with a filesystem worktree cleanup target; `Conversation(id)` and `Global` structurally cannot trigger worktree removal. Do not infer ownership from cwd: Direct continuations and Explore sub-agents remain conversation-scoped even when cwd happens to be inside another scope's checkout.
3. **Unify the ownership predicate.** Ensure lifecycle cleanup, terminal fallback cleanup, and reconciliation use one typed definition of a live `WorkScope` owner so terminal, archived, continued, multi-hop, and shared sub-agent cases cannot drift. Preserve the intentional worktree-cleanup skips when the transition itself enters `ContextExhausted` or `HandedOff`; those transitions transfer/pause ownership rather than ending the unit of work.
4. **Repair startup reconciliation.** Group conversations by normalized worktree scope and identify Git-registered Phoenix worktrees with no live owner. Reuse normal project cleanup semantics for worktree and Work-branch disposition. Log structured counts, paths, sizes when practical, and failures. Never touch a scope with a live owner.
5. **Safely reclaim existing residue.** Before removing a dirty stale tree, capture the same durable diff/status evidence required by its terminal disposition, including untracked-path metadata, or leave it quarantined with an actionable warning if lossless capture is unavailable. Reclaim the 36 handoff-regression trees; handle the two older archived-idle trees through the same ownership-safe reconciliation if their lifecycle metadata is sufficient.
6. **Add regression coverage.** Replace tests asserting HandedOff dead-end protection with tests proving:
   - fresh Explore→Work handoff preserves the worktree while the successor is live;
   - a terminal/archived/deleted final successor permits cleanup;
   - multi-hop `HandedOff → ContextExhausted → terminal leaf` permits cleanup;
   - an uncontinued `ContextExhausted` owner still protects against sibling/sub-agent deletion;
   - continued Work/Branch/top-level-Explore leaves resolve to and clean the inherited `WorkScope::Worktree(path)`, not a leaf-ID-derived path;
   - Direct continuations and Explore sub-agents resolve to `WorkScope::Conversation(current_id)` and cannot remove a cwd/shared-parent worktree;
   - `WorkScope::Global` cannot enter conversation worktree cleanup;
   - startup reconciliation reclaims only fully dead worktree scopes and is idempotent;
   - dirty stale scopes are captured or explicitly quarantined rather than silently destroyed.
7. **Align normative artifacts.** Update Bedrock Allium behavior if needed to make permanent handoff ownership transfer explicit, update executive verification anchors, and remove code comments/tests encoding the contradictory dead-end-protector policy. Run the spec authoring pre-flight and Allium validation.
8. Run focused Rust tests and `./dev.py check`, then verify the production inventory query reports zero reclaimable handoff scopes after deployment/remediation while all 15 intended-live scopes remain present.

## Acceptance criteria

- Ending the final live conversation in a handoff/continuation chain removes its shared worktree according to lifecycle policy.
- `HandedOff` never independently owns a worktree after transfer.
- Uncontinued `ContextExhausted` conversations still preserve work and remain actionable.
- Cleanup is keyed by canonical `WorkScope`: only `Worktree(path)` removes a checkout; `Conversation(id)` and `Global` cannot.
- Work/Branch/top-level-Explore continuations retain one worktree scope across transcript IDs, while Direct and Explore-sub-agent continuations remain conversation-scoped.
- Startup detects and safely repairs fully terminal Phoenix-owned worktree scopes without touching live scopes.
- Existing dirty residue is not silently discarded.
- The reported 31 GiB worktree and all other safely reclaimable production residue are removed, with before/after counts and disk usage recorded.

## Completion record

- Removed 35 clean stale production worktrees before implementation, reclaiming 81.20 GiB with zero failures. The reported `commission-review-feedback` scope accounted for approximately 31 GiB.
- Retained three dirty stale scopes containing untracked evidence. Post-cleanup inventory: 3 stale scopes, 0.136 GiB, all dirty.
- Replaced recursive continuation-chain polarity logic and the duplicate deployment-disk predicate with one persisted-row `conversation_owns_work_scope` predicate.
- `HandedOff` and continued `ContextExhausted` rows permanently cede ownership; an uncontinued `ContextExhausted` row remains the terminal owner exception.
- Terminal fallback cleanup resolves canonical `WorkScope`; only `Worktree(path)` can remove a checkout.
- Startup reconciliation reclaims clean unowned scopes, removes ignored build output safely, applies mode-specific branch disposition, and retains dirty/unreadable scopes with warnings.
- Added lifecycle, multi-hop handoff, inherited-path, Direct/shared-cwd, startup reclamation, dirty retention, ignored-output, idempotence, live-scope, and deployment-disposition tests.
- Updated project requirements, Allium guarantees, and executive verification notes.
- `./dev.py check`: all 18 checks passed.
