# ADR-288: Unified Close is idempotent with one bounded completion-only retry

- **Status:** Proposed
- **Date:** 2026-11-14
- **Affects:** REQ-WL-002f; REQ-BED-029; unified native `close_conversation`, retained Close operation/run identity

Identity note: this branch-local proposal is ADR-288 after integration numbering
reconciliation. Main's published ADR-283 (ordinary foreground batches), ADR-284
(eight-owner foreground window), and both existing ADR-285 records (Kache
qualification and Coordinator evidence labels) remain unchanged. Its original
date, Proposed status and decision substance are unchanged; branch-local ADR-286
supersedes its automatic completion policy.

## Context

The user requested one `close_conversation` capability that is idempotent and automatically retries where appropriate, replacing the separate retry tool. This proposal chooses a conservative limit of one automatic completion-only successor per original operation; that numeric limit is a design choice, not a user-specified budget. Existing Close remains fail-closed and run-scoped: each run stops at first failure, every distinct failure has its own mandatory delivery, startup observes only, and resource/destructive retry requires fresh explicit authority. A repeated request must not become consent, target discovery, stop-work confirmation, or a second destructive run. The narrow automatic exception requires exact positive retained proof.

Existing `SafeRetryClose` is not retained as an alternate native tool. Replacement is gated on equivalent explicit initial-close and exact-retained-retry capability being qualified.

## Options considered

1. **Keep separate initial and safe-retry tools.** Preserves split surface but leaves callers to choose correctly and makes a retry-shaped lost response ambiguous. Reject.
2. **Make every repeated close request a fresh attempt.** Simple invocation semantics but duplicates destructive authority, changes identities/topology, and can bypass explicit confirmation. Reject.
3. **One source-bound retained operation with an exact bounded completion-only retry exception.** Repeated calls observe the same durable request and result; resource/destructive retries remain explicit and exact. Choose.

## Decision

- Expose one native `close_conversation` on the current Coordinator. Require trusted executing-source identity and server-derived current-Coordinator identity; no caller-input authorizer. Capture exactly one ordinary ProductConversation/latest-row admission and persist request, attempt, run, topology snapshot, and retained result identity. Duplicate, concurrent, replayed, and lost-response calls return that same operation/result. Absent or stale references for a retained action fail without selecting latest or creating fallback identity. Preserve exact attempt, ordinal, and ProductConversation references, including active, failed, completed, and cancelled records.
- Replace `SafeRetryClose` only after equivalent explicit initial and exact-retained-retry capability is qualified. An explicit retry needs fresh safety proof and admits one exact next ordinal under original authority; duplicate retry observations resolve to its retained successor, never a second successor. Old failed-run evidence remains immutable.
- Permit at most one automatic fresh completion-only successor per original operation. Admission requires fresh exact retained proof that every originally planned effect durably succeeded and that no resource or destructive effects remain. Consume the durable nonrenewable budget atomically with exact successor admission. Explicit retry, repeated calls, progress, continuation, restart, lost response, and Coordinator replacement do not replenish it. Only the exact latest stopped ordinal greater than one, with typed `AttemptInterrupted` primary failure authority and zero resource-failure children (not absence of the mandatory primary failure/outbox), may qualify. Positive provenance is a nonempty persisted resource plan with every exact durable success, or a zero-effect verified-completion plan whose retained chain preserves that positive provenance. Initial-run interruption, an interruption label alone, an unproven empty plan, unknown results, or incomplete evidence is ineligible. There is no hidden bootstrap retry; an independently authorized explicit exact-run retry can establish later-run proof. Exhaustion and ineligibility are distinct from unavailable storage, and each is reported truthfully without a loop.
- Automatic admission cannot replay effects or admit resource/destructive work. The live admitted service may use the remaining budget after fresh eligibility proof; startup only observes and may persist budget/suppression bookkeeping, never dispatches or rearms. Stop, pause, and cancellation durably suppress pending admission and fence racing callbacks; no repeated or newer request silently clears suppression or rearms a cancelled attempt. Suppression that wins after automatic admission but before completion atomically fences the callback and stops that successor with its exact failure/outbox, preserving already completed effects and the consumed allowance. A losing callback observes the terminal result; it cannot strand a Running successor or complete through suppression.
- Preserve explicit busy/loss confirmation, exact snapshot/scopes, stop-first-failure per run, once-per-run failure outbox, shutdown/resource truth, sealed gates, live permits, fingerprints, loss guards, and no branch/PR mutation or recovery-artifact creation. Repeated observation is not confirmation, cancellation/rearm, History reopening, or Stop bypass. Direct HTTP behavior is unchanged.
- ADR-088 remains historical and unchanged. This ADR partially supersedes only its absolute explicit-only retry phrasing for the narrowly specified completion-only successor; it does not supersede stop-first-failure, startup observation-only, explicit resource retry, or any identity/safety boundary.

## Typed application and durable decision boundary

The shared Close application boundary distinguishes intent, authority, and outcome rather than accepting optional identifiers as implicit permission:

- **Intent:** initial Close of an ordinary ProductConversation; observation of an exact ProductConversation/attempt/run; or explicit retry of an exact retained failed run. A new operation after cancellation additionally names the exact cancelled predecessor and carries explicit new intent. Partial/missing/stale retained coordinates are refusal, never an initial/latest fallback.
- **Authority:** a non-caller-constructible currently executing-source capability, checked against the current Coordinator and exact target before any receipt replay. Request correlation is derived from that source and cannot authorize it. Fresh safety evidence proves safety, not independent permission to act. Explicitly stopped/paused/cancelled sources are refused; unavailable verification is not silently classified as semantic refusal or eligibility.
- **Decision:** admitted exact run, observed exact retained result, typed refusal (including input collision/ineligible/exhausted), or unavailable durable authority/storage. The admitted/observed result identifies the actual attempt/run, not a speculative new UUID or cached initial acceptance after progress.

Initial admission commits its trusted request receipt, one captured member snapshot, operation/run1, and original allowance together. Explicit admission commits its exact N-to-N+1 binding, fresh safe plan, and source receipt together. Unique exact bindings serialize concurrent winners; a losing invocation rereads the committed winner after independently rechecking live source authority. There is no post-admission receipt attachment window. Reusing an input identity with different target/intent is a collision, not another admission.

A supported fresh-capture transaction writer atomically captures the full inventory and establishes its immutable original snapshot anchor (inspection generation plus fingerprint). A resumed/later inspection generation may capture its own inventory, but cannot replace, adopt, or mint a different original anchor. The schema enforces full sealed shape and immutability of positive originals and negative exclusions; the supported-writer transaction guarantees atomicity for a fresh full capture and its positive anchor, not a temporal guarantee against arbitrary future SQL writers. Every pre-migration attempt is negatively excluded from original-anchor provenance, including zero-ambiguity/no-retained-inventory cases; positive provenance is neither backfilled nor restored through native adoption. Legacy shared capture, reinspection, observation, and generic retry continue normally without gaining native original-anchor proof, and partial repair or copied inventory writes cannot originate that proof. The supported fresh-capture anchor is constrained by the exact captured scope set and complete sealed inventory/resource rows, including each scope's WorkScope identity.

Automatic completion has a distinct admission/finalization path, not the resource-settlement `CloseSafeRetryAdmitted` path. One immutable admission record binds the original allowance and exact failed/successor runs, atomically consuming the allowance with ordinal allocation. Explicit retries and retained adoption never create or replace that allowance. Admission and finalization require fresh live generation/token/process-epoch authority; original receipt identity does not recreate a permit. Suppression and successful finalization serialize at this boundary. Successful completion clears matching cleanup attention and publishes entry to History only on a real Open-to-History commit; failed/suppressed successors keep exact failure/outbox and truthful lifecycle state. History is not reopened by later uncertainty.

The application owns these decisions and passes typed exact commands to persistence. Persistence enforces atomic bindings and current durable identities; it does not reconstruct application authority from caller IDs. Legacy HTTP adapters preserve their existing behavior in this decision; they are not silently rerouted into new automatic authority. The old native retry tool stays until equivalent explicit capability and adapter/schema/discovery behavior are qualified.

## Review corrections and falsification cases

The v1, v2 and v3 exact eight-document freezes received **request changes**, not acceptance. The v4 corrections below remain proposed and require superseding whole-freeze review; no local Allium validator or semantic implementation tests were executed. The revised draft makes server resolution and completion assessment required-payload variants; binds newly created records through their actual creation results; and resolves replay from the uniquely keyed request receipt before checking its exact operation/target, never by a target-only operation lookup.

Explicit retry admission copies its exact typed plan into immutable persisted plan/effect rows (or a zero-effect verified-completion predecessor), with successor and receipt in the same transaction. The live worker consumes that binding rather than restarting generic initial settlement/inspection. Each effect outcome binds to those exact rows. Primary failure authority maps to `close_cleanup_failures.authority_kind`; resource children map separately to `close_cleanup_failure_resources`. Neither a reason string nor absence of the mandatory primary failure is positive completion proof.

Exact cancellation owns allowance suppression and callback fencing directly, including exact retained stopped and admitted completion-only runs. Durable Stop/pause, source replacement, and process-authority closure use the explicitly declared owning source-fence commit boundary: suppression, unfinished-successor terminalization, failure, and outbox commit with admission, not through an optional subsequent notification. A losing callback only observes that disposition. Already committed effects and History are preserved, together with truthful unresolved attention. The live completion-only admission has an owned read-only observation driver and success/failure returns; authority/proof loss cannot silently strand a Running successor. Inability to establish durable terminal state closes process authority under REQ-BED-033. Legacy operations lacking an allowance remain observation-only/ineligible at startup without fabricated rows.

The four cancellation/source-fence rules now select exact unfinished automatic **and explicit** admissions in pre-state and impose unconditional successor terminalization on those sets. Finalization serializes at that same owner; no terminal state depends on a callback from revoked authority. Each owning failure decision supplies typed resource evidence and inserts normalized children with its primary failure/outbox. WorkLifecycle's resource/precondition failures carry nonempty exact affected sets; lost process permission is typed source-authority loss with its actual affected set. Startup and observation carry actual exact failed-resource evidence or an intentional empty set, never string-derived classification or guessed success for unknown outcomes.

A monotonic `native_managed` operation binding partitions new native admissions from the retained user/Global modern generic retry adapter. Initial native creation sets it true; first native receipt/admission on an existing operation promotes it atomically. Generic admission is refused afterward, while an exact non-native run admitted before promotion can still finish; adoption cannot strand it. The generic adapter is **not** the legacy HTTP compatibility service. `UserRequestsCloseConversation` remains a separately typed legacy initial ingress, not a second native entry. REQ-WL-002c qualifies its old mandatory generic event accordingly; no HTTP permission widening is proposed. Existing unadopted rows begin false and never gain an absent allowance through adoption/startup. New legacy initial operations initialize their original allowance in the initial owning transaction, not via a later creation observer.

The remaining shutdown-uncertainty clause now separates initial Open/CloseIncomplete from later History-preserving observation. Shape/binding and uniqueness quantifiers are split into separately named single-expression invariants; this is static reference review, not parser certification.

V4 preserves independent WorkScope retirement in the exact plan vocabulary (`retire_work_scope`), not merely tmux/worktree effects. Its immutable row binds the sealed expected scope resource and original scope, authority/gate generation, fresh evidence and fingerprint. It is ordered after pending scoped resource effects, does not repeat retained successes, requires fresh no-live-owner/resource proof, and records its own exact durable outcome. Scope-only residuals remain representable; any remaining scope defeats verified-completion or automatic completion proof. Process-epoch stop effects intentionally remain excluded: missing original live permits fail closed, never reconstructed from a receipt/restart or routed through generic fallback. This is not a claim that the old retry tool could retry processes.

Exact retained cancellation now commits lifecycle from its stable exact shutdown proof with its cancelled outcome and terminal failure/outbox. Open plus confirmed conversation-and-owned-process shutdown enters History with unresolved cleanup attention; Open plus uncertain shutdown stays Open with typed CloseIncomplete; existing History stays History. It publishes History only on actual Open-to-History commit and does not rely on a later failure classifier that excludes completed operations. The claimed additional WorkLifecycle producer contradiction was retracted after exact v3 artifact checks found no such producer; no fictitious rule/event repair is included in v4.

These are static design traces to falsify, **not executed tests or implementation evidence**:

| Case | Required trace/result |
|---|---|
| Exact receipt A after cancelled A and explicit new B | Independently verify current source first; resolve receipt A by trusted request identity, then check its source/fingerprint/target. Return A's retained operation/run/result, never target-only selection or B. A changed fingerprint is collision. Stale or stopped sources cannot replay either operation. |
| Concurrent duplicate initial or explicit retry, including lost response | Same trusted request binding produces one winner. Initial commits operation/run1/member snapshot/allowance/receipt together; explicit retry commits N+1/exact immutable plan/receipt together. Loser rolls back and rechecks authority before reading that exact receipt. It returns the admitted identity, never a second run, receipt-attachment gap, new snapshot, or N+2. |
| Stop/cancel after automatic admission before finalization | If the fence wins, allowance stays consumed/suppressed; exact successor becomes stopped with one primary failure/outbox and no further effect or success commit. If finalization wins, later suppression observes the completed outcome without undoing effects. Source replacement/lost permit has the same terminal fencing obligation. History never reopens. |
| Legacy operation without allowance at startup | Perform universal observation/first-failure bookkeeping, but no allowance lookup is required to match and no row is created/backfilled. No execution, bootstrap, refund, rearm, or automatic successor. |
| Typed explicit resource plan versus verified completion | Resource retry persists and executes precisely the remaining normalized effect rows, excluding prior successes and rechecking exact safety/authority. Verified completion persists positive predecessor provenance with zero resource rows and only observes/finalizes. Neither enters generic initial settlement/inspection; only the qualifying stopped ordinal>1 with typed interruption, zero resource-failure children, and positive plan/chain can later qualify for the original automatic allowance. |
| History cleanup-only successor interrupted at startup | Existing positive shutdown and History remain true. Record exact interruption/failure and cleanup attention without dispatch or rearm. New uncertainty does not change History to Open or erase prior positive evidence. |

Additional v3 falsification traces (also **not executed tests**):

| Case | Required trace/result |
|---|---|
| Explicit successor loses source and worker never returns | Pre-state source fence captures its exact admission and stops successor, persists actual normalized failures and mandatory outbox, revokes effects, and preserves History independently of any callback. Late callback cannot overwrite terminal state. |
| Resource failure versus no-effect observation | Resource decision constructs nonempty expected-resource/scope children atomically with typed primary/outbox. Observation-only interruption can have zero children, but unknown resource outcomes cannot prove eligibility; residual text cannot decide either case. |
| Generic retry races native adoption | Generic admission first: that exact run can finish after receipt adoption; no subsequent generic admission. Adoption first: generic event is refused and cannot mint N+1 or dispatch generic settlement/inspection; native exact-plan admission alone can do so. Legacy HTTP receives no new permission. |

Additional v4 falsification traces (also **not executed tests**):

| Case | Required trace/result |
|---|---|
| WorkScope-only exact retry | Retain prior process/tmux/worktree successes. Original scope retirement failed due to a live owner; after fresh proof resolves the blocker, persist only that exact `retire_work_scope` row and its bound identity/authority/fingerprint. Recheck no live owner/resource and commit its own outcome. Do not replay earlier successes, use zero-effect verified completion, or fall back to generic admission. |
| Scope effect fails or is unknown | Stop with exact typed resource evidence/outbox for definite failure; unknown never counts as scope success. No full-plan or automatic completion proof while independent scope retirement remains. |
| Cancel explicit successor from Open with confirmed shutdown | Stop exact successor with confirmed failure/resource evidence/outbox, keep operation completed/cancelled, enter History with cleanup attention and one actual History publication in the same owner transaction. No later classifier is needed. |
| Cancel explicit successor from Open with uncertain shutdown | Stop exact successor with uncertain failure/resource evidence/outbox, keep completed/cancelled and Open with typed CloseIncomplete; no History publication or resumed execution. |
| Cancel explicit successor already in History | Preserve History and retained positive shutdown/effects/cleanup evidence, keep completed/cancelled and terminal successor/outbox; no second History publication even if current observations are uncertain. |

## Consequences

The durable request/attempt/run receipt and atomic budget/suppression boundaries require semantic implementation and qualification before the old retry tool is removed. This decision does not certify implementation or acceptance; the existing acceptance baseline remains distinct from evidence for these new semantics.

## References

- [REQ-WL-002f](../work-lifecycle/requirements.md) and [REQ-BED-029](../bedrock/requirements.md).
- [Work Lifecycle Allium](../work-lifecycle/work-lifecycle.allium) and [Bedrock Allium](../bedrock/bedrock.allium).
- [ADR-088](088_close-stops-on-failure-and-separates-cleanup-from-lifecycle.md): historical stop-on-failure and explicit retry policy, narrowly qualified here.
- [Work Lifecycle Executive](../work-lifecycle/executive.md): status and acceptance obligations.

## Superseding decision: proof-gated initial completion

The selected completion-only policy now also admits the original native Initial
run after an all-success interruption, subject to the same single original,
nonrenewable allowance. This supersedes the original ordinal-greater-than-one
restriction, not the resource-retry or startup restrictions. The implementation
decision was delivered by the Coordinator under saved delegated authority
(primary `user_api` messages `9f3977a8-9ef7-4032-a416-150ba68aab5a` and
`fa7abcd2-0f75-4892-a9d9-d1b409cf111e` in transcript
`8fa4b22f-b23c-44ef-bc72-0a02e52713bc`); it is not a new direct-human approval.

Eligibility requires the retained exact Initial receipt, immutable original
inspection anchor, complete sealed captured inventories, exact dispatch
membership, successful original outcomes for every expected resource including
independent WorkScope retirement and required process-step successes, and
confirmed conversation/owned-process shutdown. Missing, foreign, residual, or
ambiguous proof fails closed. Original proof never grants current live source
authority. Stop, pause, cancellation, revoking source replacement, and startup
recovery cannot automatically complete. No successful resource effect repeats.
The interruption is a truthful unsuccessful outcome with mandatory notification,
zero fabricated resource failures, and cleanup attention rather than successful
Close finalization, even when confirmed shutdown permits History archival.

The first implementation slice only establishes that durable interruption and
its positive proof boundary. Initial successor admission and assembled live
execution remain separate incomplete obligations. Migration 167 in this branch
is provisional only: it does not allocate a main/production number, rewrite
installed history, reconcile migration collisions, or authorize integration or
production schema execution.
