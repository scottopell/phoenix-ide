# ADR-286: Close uses exact blockers and explicit retry only

- **Status:** Accepted
- **Date:** 2026-11-23
- **Affects:** REQ-WL-002/002a/002b/002c/002d/002e/002f/004; REQ-BED-029; Git-backed WorkScope retirement and unified native `close_conversation`

## Context

The accepted scope narrows Close to a bounded retirement operation rather than a recovery or cleanup assistant. ADR-288 proposed a proof-gated automatic metadata-only successor with an original allowance, admission and suppression machinery, and special completion finalization. That adds an automatic authority path even when no resource effect remains. The selected product need is one idempotent native operation with exact retained observation and an explicit Retry or Stop decision after failure.

The existing inspection also separates ordinary loss warnings from ignored/build content and allows an exact discard confirmation. That distinction obscures the actual blocker decision: repository metadata cannot tell Phoenix that a particular file is safe to delete. A parallel disposable/reconstructibility classifier or in-Close salvage flow would add policy and authority beyond retiring the registered environment.

This decision records the Coordinator-verified accepted simplification. It does not certify implementation, qualification, migration compatibility, or production execution.

## Options considered

1. **Retain ADR-288's one proof-gated automatic completion-only successor.** Preserves automatic completion after an all-success interruption, but needs a separate allowance, admission, fencing and finalization authority that does not serve the explicit Retry/Stop model. Rejected.
2. **Retain discard confirmation and a disposable-state classifier, with salvage/repair inside Close.** Can remove more environments without separate owner action, but requires Phoenix to decide which data is expendable and adds new mutation/approval paths. Rejected.
3. **One idempotent Close, one exact blocker decision, explicit bounded retry only.** Preserves source and retirement safety while leaving unresolved content intact. Owner cleanup is separate and may leave disk usage unresolved until a deliberate action. Selected.

## Decision

Choose option 3.

### One operation, no automatic successor

Native `close_conversation` owns initial Close, exact retained observation and explicit safe retry. Duplicate, concurrent, replayed and lost-response requests return the same retained operation/result, not another attempt or implicit retry. Trusted current source identity, exact request/target/latest-row binding, immutable original authority and evidence, and refusal of missing/stale references remain required. Observation cannot supply stop-work confirmation or broaden permissions. Direct HTTP behavior is not widened.

Every stopped run remains stopped. A successor, including one with only final metadata left, requires an explicit user Retry and fresh exact safety evidence under the original Close authority. It uses a new monotonically increasing run ordinal, performs only exact remaining effects, never repeats completed effects, and stops at first failure. Stop/pause/cancellation and source-authority loss still fence dispatch and finalization. Startup only observes; elapsed time, metadata, external cleanup or source replacement never authorize retry. Stop preserves unresolved targets and evidence and ends further Close effects for that operation.

There is no automatic completion-only allowance, successor admission, rearming or automatic finalization path in this policy. Removing that path does not remove explicit-retry source fencing, ordinary outcome finalization, exact success/absence proof, or mandatory failure delivery.

### One blocker decision and projection

Inspection materializes exact per-scope evidence and one authoritative aggregate decision. Its user projection reads those same rows, not a parallel classifier. Rows name exact affected paths/commits or expected resources and concrete operational reasons, including writers, permissions, layout, inspection errors and deadlines. Incomplete or uncertain inspection is a blocker, never an empty successful inspection.

Unpreserved staged, unstaged/conflicted/unmerged, untracked, ignored/build, initialized-submodule content and unreachable detached commits block destructive retirement. `.gitignore`, its absence, and other repository metadata grant no discard permission. Close has no discard override, salvage, repair, cleanup convenience or disposable-state classifier. Separate owner cleanup or preservation requires separate authority, followed by fresh Close inspection; that action alone is not retry admission.

At each relevant mutation boundary, Close rechecks exact identity, ownership, permissions and blocker state. Newly observed failures become concrete blockers in that same decision/projection. A prior clear snapshot is not a guarantee of later mutation success.

### Preserve lifecycle and safety truth

Close acts only on registered original-authority WorkScopes and resources, including independent WorkScope retirement. WorkScope gates, one ordinary owner, live process permits, exact tmux identity, private-directory identity validation, external-writer evidence, exact absence adoption, no branch/PR mutation, and no automatic recovery artifacts remain required. Missing or uncertain authority preserves the target.

The first failure stops effects across all targeted scopes and durably retains successful prefixes, exact failed/remaining resources and mandatory once-per-failure Global delivery. Positive proof that conversation execution and owned processes stopped may move an Open conversation out of Active into History only with conspicuous incomplete/disk-retained cleanup attention, exact blockers and explicit Retry/Stop. Uncertain shutdown remains visible Open with `CloseIncomplete`. Later uncertainty preserves already committed History and positive shutdown evidence, without implying retirement success or reopening execution. Failure notification grants no cleanup or repair authority.

### Supersession boundary

This decision supersedes branch-local ADR-288's automatic completion-only successor policy, including its initial-interruption extension and the allowance/admission/suppression machinery specific to that automatic path. ADR-288's source-bound idempotence, exact retained identity, explicit safe-retry authority and independent resource/scope proof are retained.

It partially supersedes ADR-088's disposable-state classification and explicit-discard deletion alternative: neither is an in-Close path. ADR-088's stop-on-first-failure, observation-only startup, separation of shutdown from resource retirement, explicit fresh bounded retry, mandatory failure delivery and original identity/ownership safeguards are retained. Separate owner-authorized work is not a Close phase. ADR-241's immutable exact-run writer evidence remains required. Historical ADR text is left unchanged; this record and the index identify the superseding policy.

## Consequences

- **Positive:** One explicit authority path governs retry, and one decision governs blocking and its projection; no automatic metadata exception or second disposable classifier can diverge from it.
- **Positive:** Ignored/build data and operational impediments remain exact visible blockers instead of implicit deletion permission or misleading success.
- **Negative:** Close can leave environments and disk usage unresolved indefinitely. Owners must clean up or preserve content separately, then explicitly retry after fresh inspection.
- **Negative:** An interruption after all resource effects succeeded still requires explicit retry to finalize; observation alone does not converge it.
- **Neutral:** Bedrock contracts and implementation/acceptance obligations must be reconciled with this policy. This record makes no implementation or test-pass claim and grants no database surgery, migration rollout, deployment or production cleanup authority.

## References

- [Work Lifecycle requirements](../work-lifecycle/requirements.md), especially REQ-WL-002/002a/002b/002c/002f/004.
- [Work Lifecycle Allium](../work-lifecycle/work-lifecycle.allium) and [executive status](../work-lifecycle/executive.md).
- [Bedrock requirements](../bedrock/requirements.md), REQ-BED-029, and [Bedrock Allium](../bedrock/bedrock.allium).
- [ADR-288](288_unified-close-idempotence-and-bounded-completion-retry.md): branch-local proposal; its automatic completion-only policy is superseded, while source and exact-operation safeguards are retained.
- [ADR-088](088_close-stops-on-failure-and-separates-cleanup-from-lifecycle.md): retained bounded execution/lifecycle policy; superseded in-Close discard/disposable alternative.
- [ADR-241](241_close-writer-authority-is-immutable-run-evidence.md): retained writer evidence contract.
