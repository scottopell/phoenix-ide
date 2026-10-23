# Execution-owned cancellation and terminal outcomes

## Review checkpoint — proposal, not implemented behavior

Task 98021. Human approved development with contract review before runtime changes. This document proposes that contract; it is not a normative specification or a passing-test claim. Only this document and the approved general AGENTS.md guidance change in this checkpoint. No merge, deploy, activation, historical repair, or heavy execution is authorized here.

Working scope: `669e7283-4c21-48c6-a75f-dfaa529c7dcb`, baseline `66005abb0bd54f4c03e539573c2893a992cafb05`. Canonical reference: `c1125c6eff420adefa819bb6ecf62565b0c1728d`. Goals owner retains the a52 worktree and shared runtime/core/DB seams; its reported source `ecc9ca96e18d418313a9a38274dd15ad351b7083` is not a qualified integration base. Re-resolve before implementation; do not rebase or absorb its moving fixes implicitly.

## 1. Forensic findings

Product conversation `58cff20d-81c3-4655-b9d8-61fe615bda28`; parent transcript `0ace32b0-dc4a-4d4d-ba3c-0271131841e9`. Local deployment status recorded committed source `23badb8be4bc8d05a7e1c49c5367ceb14ebccd92`; this corroborates the supplied deployed identity, not an independent live-binary attestation. Evidence was read from prod.log, read-only prod.db, and bounded VictoriaTraces queries.

| UTC, 2026-10-21 | Independently observed evidence |
|---|---|
| 00:03:00.486358 | Sub-agent completion timeout, count 1 |
| 00:03:00.486560 | Parent AwaitingSubAgents → CancellingSubAgents |
| 00:03:00.493075 | Child `41794b10-f5d4-4043-99cd-c8c0a961a6e3` executor ends Failed("Sub-agent timed out", TimedOut), not generic cancellation |
| 00:03:00.504310 | Parent resumes LlmRequesting |
| 00:37:43.277378 | Parent enters AwaitingUserResponse |
| 01:03:57.756833 | Parent resumes LlmRequesting |
| 01:07:30.764829 | Parent reaches Idle |
| 01:07:30.773363–.778357 | Trace `39886102ddbe8f84f07faa90d052993d`: direct_turn.settle, turn 7586, generation 0, target Idle, terminal cancelled, committed |

Database terminal fact: turn 7586 = Cancelled. Watch events `9b14ffff-c44d-4f15-89a7-7605bdd69f54` (direct_turn/7586) and `0e55f5aa-a171-47c3-aad3-14b6ecc2f0d4` (interaction_response/55b3335b-f9d9-4c46-90e6-3aa7e4acf111), both generation 0, cancelled, null reason, times 1792544850774774 and 1792544850777358 microseconds. These are distinct source occurrences, not demonstrated same-ID duplicate delivery. 01:07:30 UTC is 21:07:30 New York on October 20.

Deployed executor timeout dispatch emits UserCancel(Timeout). `classify_active_direct_turn_terminal` sets `direct_turn_cancellation_initiated` for any UserCancel; later Idle uses it to choose Cancelled. Helper timeout handling deliberately resumes parent work. Direct-turn settlement also settles a captured interaction-response occurrence. Those facts explain the false outcome and its two publications.

Separately, `coordinator_watches::record_watch_event_tx` consumes conversation-keyed `execution_cancel_observations` and can override the supplied terminal kind. Its user-request-only producer for no-active-turn cancellation does not establish that the timeout inserted such a record. Its contribution to this incident is unproved. User/Coordinator non-involvement is supplied evidence, consistent with logs, not provable from absence of log entries alone.

### Source comparison and restart

The classifier and publication override remain in canonical c1125c6 and this checkpoint baseline; earlier inspected Goals source 3a641490 also retained them. No assertion is made about unreviewed later Goals edits. The executor Boolean initializes false on materialization and resets with turn ownership cleanup; it is not durable Stop authority. Conversation-scoped observations persist independently until consumed by publication. Thus restart can forget one representation while retaining another. Existing recovery consumes terminal obligations, exact generations and transcript-tail evidence; reuse that machinery rather than assuming a persisted replacement Boolean solves recovery.

## 2. Governing contract

1. A helper timeout can terminate that helper and return control to its parent. It cannot by itself produce a cancelled parent.
2. A control request is not a terminal outcome. Accepted execution Stop, helper teardown, tool interruption, error, and shutdown preserve distinct causal ownership.
3. One execution has one authoritative terminal fact. Publisher/UI projections cannot independently adjudicate it.
4. An accepted Stop prevents normal dispatch/completion for that execution. Completion already committed before Stop acceptance remains historical fact.
5. Cancellation does not roll back external effects or prove that an already-issued external request never executed.
6. Question/approval waits and successful continuation are not terminal cancellation. Restart/replay cannot manufacture a new execution or terminal notification merely by losing process memory.

## 3. Typed operations and state model

Use existing accepted-turn identity and generation, child admissions, tool effects, workflow receipts and terminal obligations. Names below describe required roles, not new parallel ID systems. Private constructors must bind receipts to existing authority; callers cannot construct cancellation evidence from a free-form cause or status flag.

| Typed operation | Initiator / cause | Exact target and scope |
|---|---|---|
| StopExecution | authorized user or explicitly authorized system principal / manual or named policy | owning accepted execution and generation; owned pending descendants |
| ExpireChildWait | runtime / absolute deadline | spawn invocation and captured pending child admissions |
| ExpireToolAttempt | runtime / deadline | exact tool effect attempt, handled under its existing retry/ambiguity policy |
| PropagateExecutionStop | owning parent / original accepted Stop receipt | exact child admission, retaining root causal authority |
| SettleForClose | lifecycle orchestrator / Close request | attempt-captured accepted-turn targets under existing Close receipt contract |
| InterruptForShutdown | system / shutdown | exact attempt under existing recovery policy; not a fabricated user Stop |

Do not model these as a Cartesian product of arbitrary initiator/cause/target strings. Expose those dimensions as exhaustive projections from valid variants. A transport cancellation token is an interruption mechanism, never evidence for terminal classification. Runtime failure remains failure even when it requires subordinate teardown.

Execution lifecycle:

- Active(Running | AwaitingQuestion | AwaitingApproval | AwaitingChildren | DrainingTimedOutChildren).
- Stopping(accepted Stop receipt, remaining owned teardown).
- Settled(Completed evidence | Failed evidence | Cancelled evidence).

DrainingTimedOutChildren cannot supply a parent-cancellation witness. Cancelled evidence must name the accepted execution-level interruption authority, including lifecycle Close when that policy is approved; no public `Cancelled` constructor accepts only a Boolean. Generation invalidation and teardown must preserve the accepted receipt after old dispatch authority is revoked. Terminal obligations encode selected outcome and causal reference durably; a second shadow lifecycle table is not the default solution.

## 4. Ordering and structured concurrency

| History | Required result |
|---|---|
| Helper deadline, recovered parent finishes | Parent Completed; helper timeout retained separately |
| Helper deadline, parent later errors | Parent Failed with actual failure evidence |
| Stop accepted during helper drain/question wait | Parent Stopping; no normal resumption |
| Stop accepted before terminal checkpoint/settlement wins arbitration | Cancelled, no later normal completion |
| Completion committed before Stop | Immutable completion; Stop fences future execution according to existing control contract |
| Child completion races timeout/Stop | One accepted child result; exact admission fan-in idempotent |
| Teardown error after accepted Stop | Retain Stop cause plus teardown failure, never fabricate success |
| Explicit Complete/Blocked goal report | Actual report checkpoint decides before automatic host follow-through; no invented cancellation |
| Continuation admitted | Explicit handoff, no terminal watch notification for transfer |
| Terminal exhaustion/error | Failed according to owning contract, not Cancelled merely because teardown ran |

Stop and terminal checkpoint acceptance must serialize against the same durable turn/control authority. The implementation must identify whether a committed terminal obligation already fixes the winner; it cannot invent a later competing arbitration point. Stop after a nonterminal response checkpoint still stops remaining work. The existing Goals/preprovider Stop fence must stay independently committable: watch append failure cannot undo Stop. Goal replacement/release cannot resurrect the stopped execution. Child draining retains exact admission ownership and one original deadline; nested cancellation cannot extend grace indefinitely.

## 5. Identity, idempotency and recovery

Distinguish execution (accepted turn), attempt/generation, input/source occurrence, settlement fact, and notification event. Reuse existing identities and uniqueness constraints wherever possible. Do not introduce a new root execution UUID simply to join records already causally bound by accepted-turn identity.

- Question response contributes a source occurrence to unfinished work. Pausing does not settle the execution; resumption retains its Stop authority.
- Retry retains logical work identity and existing generation fencing. Stale attempts cannot settle, dispatch, or enrich a replacement execution.
- Continuation records explicit predecessor/successor authority transfer; never infer transfer from matching transcript text or timestamps.
- New accepted work after settlement is a new execution. No conversation-wide leftover marker may cancel it.
- Terminal settlement, ownership release, durable reducer projection and notification obligations follow existing atomic boundaries. Ambiguous commit probes exact identities; replay cannot add another fact/outbox obligation.
- Recovery loads accepted interruption/terminal evidence before dispatch. It neither infers Stop from a cancellation token nor fabricates normal completion because process memory was lost.
- Late child results satisfy child admission and attempt fences; repeated acceptance is idempotent, not new parent progress.

A crash-cut suite must cover receipt commit, external interruption dispatch, response checkpoint, terminal obligation, terminal commit, outbox acceptance, and continuation handoff. External provider/tool ambiguity follows existing reconciliation/manual-resolution policies; no exactly-once external guarantee is added.

## 6. Publication, provenance and presentation

Remove publication's power to override terminal outcome from `execution_cancel_observations`. New events must reference committed execution settlement. Store addressable provenance as constrained relational fields/references, not an untyped JSON reason bundle. Initiator, target and cause are derived from accepted evidence, not guessed from UI strings. Historic unknown provenance stays unknown.

The existing cancelled-event null-reason schema constraint means provenance needs an explicit schema/type/wire change. Preserve downstream distinctions through Rust, generated TypeScript, SSE schemas, coordinator input and UI; unsupported sinks log capability gaps. A cancelled parent should say who/what stopped that execution when known; helper timeout appears with the helper, not as parent Stop.

Two valid source occurrences may refer to the same terminal fact. Preserve existing event cardinality for this fix: distinct source events must agree with the same authoritative outcome. Optional execution-level grouping changes coordinator input, not just visual cards, and awaits a separate explicit user decision; it is not an acceptance prerequisite for authority/timeout correction. Do not alter occurrence uniqueness or merge unrelated executions by timestamp. Event IDs remain transport dedup identities; any future semantic grouping must use proven settlement identity.

Close suppresses unaccepted watch notifications under existing subscription rules. It must not destroy settlement evidence or invent delivery acknowledgement. No watch is re-enrolled or history replayed as part of migration.

## 7. Migration and compatibility boundary

Forward-only feature migration, under specs/compatibility/requirements.md. Extend existing execution/terminal evidence with typed causal references as needed; constrain new writes structurally. Historical terminal rows/events remain unchanged with explicit legacy/unknown provenance. Do not claim historical Cancelled rows were genuine Stops, and do not silently correct this incident.

Unconsumed legacy conversation cancellation observations cannot be assigned to the next execution. Proposed policy: retain them as legacy audit evidence, exclude them from new settlement adjudication, and reconcile an active ambiguous legacy execution only through existing exact-authority recovery or explicit manual resolution. The review must approve this retirement/reconciliation policy and data-retention contract before migration implementation. No permanent dual-write or cross-version fallback is proposed. No downgrade or live replacement guarantee is introduced.

## 8. Proof obligations and regression plan

| Obligation | Falsifying test |
|---|---|
| Helper noninterference | Compare successful parent histories with/without recoverable helper deadlines; terminal outcome identical |
| Genuine authority | Every newly Cancelled execution joins an accepted execution-scoped interruption receipt; child deadline cannot satisfy constructor/sink |
| Monotonic Stop | Exhaust event interleavings through waits/drains/provider dispatch; no Stopping → ordinary dispatch |
| One terminal truth | Race failure/completion/Stop and replay; at most one fact, agreeing durable turn/source events/UI |
| Incident | Timeout child, recover, question wait/resume, final Idle; parent Completed and sources agree |
| Crash invariance | Restart at each durable cut; same winner and no new notification obligation |
| Fencing | Late completion, stale generation, nested helpers, duplicate fan-in cannot mutate settled/replacement execution |
| Goal integration | Complete/Blocked actual report, exhaustion, Stop/release and host follow-through preserve existing ownership decisions |
| Controls | Cancel versus Close versus Stop tested independently, including captured Close target replay |
| Compatibility | Legacy unknown facts preserved; leftover observation cannot contaminate later work |
| No rollback fiction | Interrupted external-effect result remains reconciled/unknown where appropriate, not erased |

Use pure state-machine/property tests plus exact-authority DB transactions and disposable integration fixtures. Require real pause/resume and replay paths, not fabricated direct calls bypassing receipts. Watch rollback must not roll back Stop acceptance. Test assertions cover DB and coordinator input as well as UI count/text. No tests/builds have run for this checkpoint.

## 9. Decisions for contract review

These are proposal decisions, not unresolved questions in a normative Allium spec:

1. **Execution notification grouping (deferred, nonblocking):** retain existing cardinality in this fix. Optional grouping will be presented separately for explicit user decision; distinct source events must agree with authoritative settlement regardless.
2. **Close provenance (behavior preserved):** distinguish lifecycle Close from user Stop where evidence supports it, without changing existing Close settlement or control behavior. Any public reason-taxonomy expansion needs explicit contract review.
3. **Legacy pending observations (implementation review required):** preserve historical facts and propose exact forward-only retirement of ambiguous pending rows without granting them new cancellation authority. Bind the proposal to actual schema and existing recovery/manual-resolution behavior; no inferred historical repair.
4. **Terminal linearization (source-grounded implementation decision):** map accepted Stop versus already-durable terminal obligation onto the existing transaction authority before choosing schema/API changes. No duplicate generic lifecycle table.

Do not ask the user to decide compiler-level representation details. Present concrete product choices for 1–3; resolve authority mechanics with source evidence and review. Turn approved outcomes into timeless requirements/Allium and a new project ADR, following specs/AUTHORING.md. This proposal itself remains historical design evidence.

## 10. Owners and incremental acceptance

- This task: approved general guidance and contract document now. No parallel runtime writer.
- Goals owner b477b7e7: current shared runtime/core/DB seams; coordinate one writer/handoff after its bounded fix, re-resolving integration HEAD and qualification separately.
- UI ownership: keep #964 independent; coordinate later event provenance/grouping components before edits.
- Host 5d57fcc6-e336-469f-850b-63cc253ea0ea: heavy-execution admission, not documentation permission.

After contract review and ownership handoff: introduce typed operations/settlement authority with all-target compile inventory; integrate durability/recovery; then publication/provenance and any approved grouping. Prefer coherent small slices with no public fallback accepting ambiguous cancellation evidence. Obtain exact-HEAD adversarial review and host-admitted focused/crash/property qualification before broad validation. Report source review, compile, executed tests and deployment separately. No merge or deployment is authorized by task approval.

## 11. Review narrowing and concrete implementation seams

Contract feedback after fa54ac46e: grouping is optional and deferred; preserve event cardinality. Close provenance must not change Close behavior. Historical facts stay untouched. The following source mapping is against checkpoint baseline 66005abb, not an endorsement of a future Goals integration.

- API `cancel_active_direct_turn` loads exact runtime-turn authority, then calls `terminate_authoritative_turn(TurnCommand::Cancel { turn_id, expected_generation })`. Runtime event delivery is downstream of that durable command; do not replace it with a process-local Stop flag.
- `WorkflowRepository::persist_terminal_obligation` opens and commits a transaction; its `_tx` form participates in an existing response/state checkpoint transaction. `load_active_terminal_obligation(&ConversationAuthority)` recovers the obligation bound to owning turn/generation. This is the candidate completion linearization seam, distinct from eventual terminal settlement.
- `terminalize_authoritative_turn_with_occurrence_at_cut` uses BEGIN IMMEDIATE, captures exact source occurrence, invokes `terminalize_authoritative_turn_in_tx`, and settles captured occurrence with the committed/replayed turn outcome in that transaction. Its AlreadyTerminal branch reads the persisted terminal rather than accepting the caller's proposed replacement. Preserve that path.
- Before implementation, inspect the post-handoff definitions of `terminate_authoritative_turn`, terminal-obligation insertion and generation CAS together. Determine whether Cancel checks an existing terminal obligation or can win after it. Specify one winner at those existing transactions and test both commit orderings. Merely changing later executor classification is insufficient. This remaining source question is engineering review work, not a product decision for the user.

### Precise pending-marker retirement proposal

The legacy table has only `conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE`. It contains no accepted-turn identity, generation, initiator, timestamp, or cause. Therefore migration cannot honestly bind any row to an execution or reconstruct user intent.

Propose a forward migration that renames the table to `legacy_execution_cancel_observations` without altering its rows, retaining its FK/deletion behavior. Remove every production write/consume path to that retired table in the same release; production settlement/publication must have no API that accepts these rows as authority. Do not copy rows into a new cancellation-receipt table, clear them as processed, emit historical notifications, or rewrite any terminal result. Their sole meaning is retained evidence of an ambiguous pre-migration marker, not accepted Stop.

Active legacy work is classified only using existing exact turn/generation, committed terminal obligations, and existing recovery rules. If those rules cannot resolve an execution, use the already-supported failure/manual-resolution contract; this proposal does not invent a new fallback or turn an ambiguous marker into cancellation. Migration tests must preserve row counts/identities and terminal/event payloads, prove next-execution noninterference, and verify that no runtime publisher consults the retired table. Exact migration registration/DDL ownership and the corresponding normative retention clause require implementation review before coding. No database migration is applied by this checkpoint.

## 12. Pinned arbitration analysis

Read-only comparison: canonical `c1125c6eff420adefa819bb6ecf62565b0c1728d` versus separately pinned Goals `ecc9ca96e18d418313a9a38274dd15ad351b7083`. Goals worktree HEAD resolved to that commit during this analysis; dirty source was not consumed.

**Existing winner:** `terminate_authoritative_turn` passes `projection: None` into the existing BEGIN IMMEDIATE settlement transaction. Its inner path loads the durable turn, applies `DurableTurnModel::apply`, and CASes workflow head/version/generation. Before updating the durable turn, it rejects projection-less terminalization when a `direct_turn_terminal_obligations` row exists. Although the workflow CAS occurs first in statement order, that error aborts the transaction: it is not an accepted partial cancellation.

Conversely, `persist_terminal_obligation_tx` inserts only from the exact owning, nonterminal turn at the expected generation, with first-payload-wins replay verification. A committed Cancel advances generation and releases ownership; subsequent terminal-obligation insertion for the old authority fails. A committed terminal obligation prevents the projection-less Cancel from consuming it. Thus the durable terminal obligation, not later watch publication or Idle rendering, already supplies a completion commitment boundary. Both pinned sources retain this guard. Model this as existing authority, not a new competing winner flag.

| Commit order | Existing durable behavior to preserve |
|---|---|
| Cancel settles before terminal obligation | Cancelled turn/generation advancement; old completion cannot establish obligation |
| Terminal obligation commits before projection-less Cancel | Cancel transaction rejects; obligation's response/outcome remains owed for settlement |
| Competing terminal settlements | Existing turn model, workflow CAS and terminal replay/conflict checks adjudicate |

Goals adds a distinct prospective control boundary: `apply_goal_control` writes/replays an owner/principal/request receipt under BEGIN IMMEDIATE and commits epoch revocation independently. That controls future dispatch; it is not itself proof that a previously committed completion was cancelled. A Stop can fence future work while a prior committed terminal obligation remains owed. Preserve both distinctions in ingress responses and tests rather than relabelling a rejected execution Cancel as accepted cancellation.

In pinned Goals canonical checkpoint `provider_replay::persist_canonical_checkpoint`, request-boundary selection precedes `_tx` terminal-obligation persistence inside the canonical transaction. Nonterminal paths use `round::selected_state_tx`. Preserve this host decision/checkpoint ordering, the full admitted request receipt and existing control fence. The cancellation fix need not replace this response transaction.

**Smallest implementation seam:** split helper-wait expiry from execution cancellation in core events/transitions and executor dispatch; derive cancellation outcome from exact committed interruption/turn evidence instead of any UserCancel history. Keep terminal-obligation/CAS arbitration intact. Remove publisher-side marker override so both existing source-event publications project the same settlement. Where typed provenance requires storage, extend existing accepted-turn/terminal evidence rather than adding a lifecycle table. Exact no-active-turn execution-occurrence cancellation still needs an explicit identity-bound path; do not simply delete its behavior when retiring the marker.

**Ownership handoff requested:** after the Goals bounded fix is frozen/reviewed, request one pinned integration commit containing its accepted checkpoint/control changes. Proposed cancellation ownership covers `phoenix-core/src/domain/sm_event.rs` and relevant sm_state types, `phoenix-state-machine/src/transition.rs`, `phoenix-ide/src/runtime/executor.rs` plus cancellation/terminal helpers in traits and API ingress, and `phoenix-db/src/workflow/direct_turn.rs` plus `coordinator_watches.rs`. Sequence: typed helper-expiry separation and exhaustive consumers; identity-bound terminal classification and tests; publication projection; separately reviewed migration/provenance. Goals writer must finish or explicitly yield those seams before runtime edits. Schemas/migrations and public wire changes require a subsequent exact file list and approved retirement contract; no migration implementation is implied by this analysis.

## 13. First implementation slice — not qualified

Goals explicitly yielded cancellation-only event/transition/terminal hunks while reserving LlmCompletion, process_outcome replay, Goal checkpoint and GoalStart owners. Rebased the owned docs-only branch onto exact canonical c1125c6 before source edits; no unqualified Goals commit was consumed.

The first slice adds `SubAgentWaitExpired` with captured child identities, maps it through parent/sub-agent core adapters, routes only a matching current wait into timeout teardown, and makes stale expiry a no-effect state preservation. Executor helper-wait expiry uses this event, not UserCancel. An accepted user Stop during timeout draining changes its terminal direction to stop so the final child result cannot resume LLM work. Added transition regressions and a classifier regression; the existing genuine-user cancellation regression remains.

This is not the completed correct-by-construction correction: the old UserCancel cause surface, process-memory terminal flag, publication override, no-active-turn identity path, and durable provenance/legacy retirement remain to migrate. The existing bedrock Allium cancellation model also lacks the already-existing timeout-resume distinction (LastSubAgentDuringCancellation and deadline rules say Idle unconditionally); align its drain-direction model before completing the lifecycle slice. Record this known normative-model gap rather than silently claiming alignment. No migration or publication-cardinality change is in this slice.

Validation: source formatted and diff whitespace checked only. Host declined local compilation and preferred narrow hosted validation; exact workflow dispatch/commands must be agreed before execution. Tests are written, not executed. No compile-pass, behavior-pass, merge or deployment claim.

## 14. Second-slice review packet: authority dependency, not a flag substitution

Exact retained first-slice evidence: source 9c09b889f1515a6afeab1335759cada83eaa2778, qualified workflow successor 7eba4f358a01cfe0e787dbdd5afe46271eedaf36, run 39710488682 attempt 1/job 119600390381. Three-package all-target check exit 0; focused tests 3+1+1 passed. Artifact 12397566065 digest sha256:7bafbb11da58f0a3c3346e82c77a6cdc0fa7aa00616a146cf5a1295243c8430c. Prior parser-only failure 39710375736 remains distinct. No next-run budget is inferred.

Added tests-first publication regressions in coordinator_watches: (a) authoritative Completed direct-turn and interaction-response publications must both remain Completed despite a leftover legacy marker, preserving two source events; (b) authoritative Cancelled publication replay retains exactly those two sources. These test the publication seam, not full runtime acceptance. The first is expected red by source inspection because the current publisher rewrites the first outcome; neither new test has been compiled/executed. Existing override-behavior test remains present to expose the intentional contract replacement; do not delete it without replacing genuine no-active-turn Stop coverage.

### Concrete dependencies discovered

`StateStore::record_execution_cancel` is conversation-keyed and defaults to success. DatabaseStorage inserts the legacy marker. Executor calls it only for user-requested cancellation when no active direct turn exists and a non-Idle state transitions into stopping/Idle. Separately, `steering_execution_occurrences` has a conversation primary key plus message_id and source_kind. That is the existing exact input identity to preserve, but today it has no interruption evidence. Public `Database::settle_execution_occurrence` accepts only Completed or Failed; it cannot express genuine Cancelled without the publisher override. Therefore deleting the override alone would regress genuine no-active-turn Stop. Moving it earlier under the same conversation key would merely relocate the bug.

For active turns, the Idle/terminal API branch commits exact TurnCommand::Cancel before event delivery; the busy API branch only looks up authority for telemetry and sends UserCancel. It does not precommit Cancel. Other production UserCancel senders include runtime propagation, materialization/cancel/recovery paths and one-shot drive timeout. These must be classified by causal ownership before removing the Boolean. `classify_active_direct_turn_terminal` currently observes only an event and state; it cannot establish or verify durable cancellation. Replacing its Boolean with a sticky Cancelled enum is not an acceptable fix. Existing settlement error reconciliation probes state plus active ownership, not a typed retained terminal cause, so it must not be treated as sufficient proof of which terminal result won.

### Smallest safe sequencing for review

1. Extend the existing exact-occurrence path to carry accepted execution interruption evidence, bound to the captured message identity; make acceptance and replacement/replay rules explicit. No conversation-wide sink with a success default. Preserve current Stop/Close control behavior and teardown publication timing. This requires a reviewed forward schema/API delta on the existing occurrence, not another lifecycle table. Exact retirement rename is still a separate unapproved proposal.
2. At execution cancellation ingress, bind accepted durable authority before state transition/teardown. For accepted direct turns use existing Cancel command/result or retained exact terminal evidence; distinguish previously committed completion and revoked authority rather than manufacturing cancellation. For non-turn work use the exact occurrence evidence above. Fail closed if acceptance is unavailable; never reconstruct receipt from matching Idle state.
3. Remove `direct_turn_cancellation_initiated` and every reset. Runtime terminal proposals come from selected completion/failure state or accepted durable interruption, with retained fact checked at existing obligation/settlement sinks. A rejected/stale Cancel cannot furnish Cancelled evidence. Preserve ordinary response/Goal checkpoint transaction ownership; any needed hook there requires exact-hunk coordination, not parallel edits.
4. Remove marker read/override from `record_watch_event_tx`. Supply terminal result from authoritative direct-turn or exact-occurrence settlement; preserve cardinality, unknown historical provenance, Close behavior and existing outbox atomicity.
5. Add acceptance/restart tests for real Stop with/without active direct turn, question response, late/stale Cancel, precommitted terminal obligation, lost acknowledgement/replay, publisher rollback and legacy noninterference before whole-fix qualification.

An exact shared-hunk request was sent to Goals for classifier/reset/StateStore/DB publication seams; reserved process_llm_completion, process_outcome replay, apply_event Goal checkpoint and canonical response owners remain untouched. If acceptance must be threaded through a reserved owner, partition that exact hook with the sole writer first. The next implementation review must approve the concrete occurrence-evidence schema/API shape and map all production cancellation senders; this is an engineering contract review, not a request for user decisions on compiler details. No migration, marker deletion, runtime-flag removal or behavior-pass claim is made by this tests-first packet.

## 15. Independent bounded contract challenge at 85b286695

Read-only independent review found two important constraints beyond the initially requested pre-classifier hook:

- Busy API cancellation is not already durably accepted. Additionally, process_event claims/aborts an LLM attempt before the old marker write site. Acceptance must precede those side effects; simply adding a fallible storage call at the marker site is too late. Rejected completion-obligation conflicts must not tear down already-owed completion.
- Canonical `provider_replay` also UPSERTs interaction-response occurrences. Occurrence replacement, recovery and all settlement sinks must understand accepted interruption. Updating only StateStore and publisher would lose authority across replacement/restart.

### Concrete proposed schema/API for review, not yet implemented

Extend existing `steering_execution_occurrences` with nullable `accepted_interruption_kind` and `accepted_close_attempt_id`, constrained to exactly no interruption, user_stop without Close ID, or close with an existing validated Close attempt. SQL CHECK must use explicit NULL predicates. No duplicate execution UUID, interrupted-message field, fresh generation or lifecycle table. Internal runtime causes require typed sender inventory; they must not be coerced to user_stop. Historical conversation markers cannot backfill these fields.

Define exact target variants for DirectTurn(turn ID + expected generation) versus Occurrence(conversation + message + source). A non-defaulting StateStore acceptance API returns Accepted(opaque receipt) versus NotAccepted(typed missing/replaced/stale/committed-result outcome); storage error remains error. Only an exact repository Cancel command/replay yielding actual Cancelled can construct a direct receipt. Occurrence acceptance uses BEGIN IMMEDIATE exact identity CAS, rejects a standalone path while direct-turn authority owns that work, validates Close captured-target relationship, retains first accepted cause and commits before runtime teardown. No fallback from rejected direct target to whatever occurrence is current.

Keep the existing Completed/Failed proposal type for natural occurrence settlement. Interrupted settlement requires the accepted receipt; the transaction reloads exact identity and chooses actual accepted result before atomic publication/deletion/state projection. Restart uses the same sink; acceptance must not publish early. Do not expose a free-standing Cancelled proposal as a substitute for receipt evidence.

Replacement rules: same message/source replay preserves acceptance; different identity does not inherit it. Do not silently discard accepted-but-unsettled old work: settle it in the existing transaction or defer adoption. All steering/fork/question and canonical response writers, `record_initial_execution_outcome_tx`, requesting projection, direct-turn captured-occurrence cleanup and runtime recovery must observe this rule. Existing direct settlement continues to determine associated source outcomes; no competing standalone acceptance under an owning turn.

Close dispatch currently has captured exact targets but sends a conversation-addressed UserCancel. Its captured target/attempt must survive delivery so a stale Close A cannot stop replacement B. Preserve Close behavior and existing receipt replay; reason text and UserRequested cannot serve as authority. Parent cycle-cap and propagated child cancellation also use UserRequested, so that variant does not prove a human Stop.

Required exact partitions before integration: (1) process_event acceptance before cancellation-claim/abort and deadline clearing; (2) canonical provider_replay interaction-response UPSERT and its existing transaction-bound settlement hooks. Neither requires replacing the Goals checkpoint algorithm. Independent review performed source analysis only, no compile/tests; this proposal requires bounded schema/API review before migration/source implementation. No additional execution authority was requested or used.

## 16. Exact hook-2 proposal for the existing canonical transaction owner

**Review-only diff; not applied to provider_replay or migrations.** Its owner retains that file. Replace only the interaction-response UPSERT at canonical provider_replay's UserApi-origin branch:

```diff
- sqlx::query("INSERT INTO steering_execution_occurrences(conversation_id,message_id,source_kind) VALUES (?1,?2,'interaction_response') ON CONFLICT(conversation_id) DO UPDATE SET message_id = excluded.message_id, source_kind = 'interaction_response'")
-     .bind(conversation_id).bind(message_id).execute(&mut *tx).await?;
+ crate::execution_occurrence::adopt_tx(
+     &mut tx,
+     conversation_id,
+     message_id,
+     crate::coordinator_watches::MessageExecutionSource::InteractionResponse,
+ ).await?;
```

Proposed crate-private signature (new focused helper module, not a new subsystem):

```rust
pub(crate) async fn adopt_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    conversation_id: &str,
    message_id: &str,
    source: MessageExecutionSource,
) -> DbResult<()>;
```

The function never begins/commits a transaction, publishes a message, changes replay, chooses a Goal report, or handles commit uncertainty. Those remain entirely with the caller. It uses the caller's existing writer transaction to read exact occurrence message/source and accepted interruption fields:

| Existing row | Requested identity | Result |
|---|---|---|
| Absent | new | Insert with NULL interruption fields |
| Same message and source | exact replay | Preserve row and accepted evidence unchanged |
| Same message, different source | conflict | Return typed identity-conflict error, rolling back caller; do not relabel source |
| Different message, no accepted interruption | replacement | Existing replacement behavior, new fields NULL |
| Different message, accepted interruption | collision | Return typed interrupted-occurrence-pending error, rolling back caller; no eager synthetic settlement or erased notification |

Choose the last-row fail-closed policy rather than introducing settlement during adoption. Before landing, the runtime admission path must consume/reconcile the old accepted interruption and retry through existing receipt rules. If a valid existing question-response flow requires atomic old settlement plus new adoption, provide the contrasting exact identity/transaction trace and revise this helper contract with the transaction owner; do not assume a blanket rejection is behavior-neutral.

All other occurrence writers must call the same helper under their existing transaction. Seeded fork insertion has no conflicting row at creation but still shares the constrained shape. A helper accepting source enum avoids divergent SQL source strings. Exact natural/interrupted settlement reloads the row and deletes by conversation AND message AND source, never by conversation alone. `record_initial_execution_outcome_tx` must use that resolver before publication, including restart and normal state updates. Direct-turn captured-source settlement uses its existing authoritative terminal; standalone acceptance is forbidden while that accepted-turn ownership applies.

No migration is authorized by this review packet. Proposed columns require first-acceptance validation and explicit table CHECK/FK described in section 15; their rollout, legacy pending-marker retirement, and Close target binding must be reviewed together before production callers depend on them. This is a precise source/API contract proposal for one-writer integration, not an instruction to the Goals writer to redesign its canonical response pipeline.

### Hook 1 guard placement and captured target

The yielded guard belongs after existing terminal-obligation/continuation-settlement absorption, but before any LLM terminal claim, abort, retry/deadline clearing or pure cancellation transition. It must receive an identity captured at ingress (exact accepted turn/generation or exact occurrence message/source), not select whatever happens to be current when a delayed UserCancel is processed. A mismatch is stale/no-op with observable rejection, never fallback cancellation of replacement work. Close carries its existing captured target and attempt; internal propagation carries child admission/parent cause. Public event adapters cannot turn a generic reason or UserRequested into an accepted receipt.

When source acceptance succeeds, runtime teardown proceeds under that receipt. When an established completion obligation wins, preserve owed completion and the independent prospective Stop fence; do not abort the task first and then discover rejection. Lost acknowledgement replays the exact acceptance target/command. Existing LLM-attempt outcome arbitration must be tested against durable Stop acceptance: attempt-completed does not by itself prove execution-terminal completion, while a committed terminal obligation does. This is the concrete contrast that the early guard must encode.

## 17. Independent Hook 1 target-validation slice and owner response

Implemented `Event::DirectTurnStopRequested { turn_id, expected_generation }` as a runtime-only request: pure parent/sub-agent adapters reject it until runtime validates its captured target. process_event compares it with the currently materialized exact turn/generation before any attempt claim/abort/deadline mutation; stale, replaced or absent targets return without teardown or fallback to current standalone work. Matching requests proceed to the existing cancellation path. This is target validation only, NOT durable acceptance or an accepted-cancellation receipt. No production ingress is switched to this new request until the acceptance contract is integrated; generic UserCancel senders are not silently reinterpreted.

Added three unexecuted regressions: adapter bypass rejection, stale ID/generation preserving the model task handle until explicit fixture cleanup, and absent-target nonfallback. This slice adds no occurrence schema, authority record, API behavior change, Goal checkpoint edit or extra execution. It does not yet remove sticky inference. Next required wiring is the non-defaulting StateStore acceptance API that can return exact retained Cancelled evidence versus committed terminal obligation/revoked target before runtime task cancellation. That wiring cannot safely be completed by normalizing a target match into proof of acceptance; the intermediate normalization must be replaced when the accepted receipt is threaded.

Canonical transaction owner's read-only Hook2 response is conditional agreement with caller-owned adoption helper, not schema approval. Normal question answers create a fresh UserApi message and replace a prior **unaccepted** occurrence; old row presence or NULL settlement alone cannot trigger collision. Same identity replay must preserve evidence; only accepted-but-unsettled interruption blocks replacement. The owner reported a newer source helper `record_message_execution_tx`; re-resolve its exact declared source at handoff, rather than applying the canonical-baseline UPSERT diff blindly. Provider/report/replay transaction semantics remain that owner's responsibility.

Remaining precise dependency: approve concrete existing-occurrence interruption column/check/FK allocation and typed acceptance/rejection declaration, then have its single canonical owner integrate the reviewed adoption/settlement helper. Existing migration/legacy-retirement proposal remains unimplemented. Without that, no-active-turn genuine Stop cannot be represented durably without the old marker, so removing that marker/flag now would be an unsound partial fix. Stop-between-model-requests remains in the invariant test matrix; model-attempt terminal outcome must not replace execution-level terminal obligation arbitration.

## 18. Independent schema disposition and required corrections

Independent reviewer 76f8afbb reviewed exact 7c5c9004bf7e1852837f3b04187b67cf75bfa45d read-only. Existing-row design is viable, conditional on two blockers being resolved in the concrete implementation contract. No DDL, migration allocation, build or execution accompanied that review.

### Retention is part of accepted authority

The existing occurrence conversation/message FKs cascade. Accepted-but-unsettled evidence must not disappear through parent deletion. A Close ID FK must explicitly use NO ACTION/RESTRICT and acceptance must validate captured membership in `close_attempt_members` plus admissible attempt phase in the same BEGIN IMMEDIATE; mere existence of a Close attempt is insufficient. Existing exact Close direct-turn target/generation checks remain mandatory for the direct target variant.

Before schema coding, choose and review the narrow retention enforcement: rebuild the same occurrence table with restrictive retention, or prove/structurally enforce that deleting a parent with accepted-unsettled evidence is forbidden until exact settlement. Do not broaden deletion behavior for all historical/unaccepted rows accidentally. If restrictive FKs change those behaviors, enumerate the affected Close/deletion paths and update the contract deliberately. Accepted evidence cannot be removed as a cleanup workaround. Historical occurrence rows receive NULL provenance only; legacy markers furnish no backfill authority.

### Adoption collision must have a progress owner

Section 16's generic typed collision is insufficient without mandatory progress. Concrete proposed result:

```rust
pub(crate) enum AdoptOccurrence {
    Inserted,
    ExactReplay { interruption: Option<AcceptedOccurrenceInterruption> },
    ReplacedUnaccepted,
}
pub(crate) enum AdoptOccurrenceError {
    SameMessageDifferentSource,
    AcceptedInterruptionPending(AcceptedOccurrenceInterruption),
    Database(DbError),
}
```

`adopt_occurrence_tx` returns the pending receipt without changing any row; the outer canonical transaction must roll back its response/replay/state changes. Its owning orchestration layer then settles **that exact old receipt** outside the rolled-back transaction, and retries the same new-message operation under the existing admission/replay contract. It must not generate another question-answer UUID, mask an uncertain commit, or retry forever without authority/progress. This is a required narrow canonical-owner integration, not a helper allowed to commit its caller's transaction. If exact old settlement is rejected/stale, classify and reconcile through existing authority rules before retry; never cancel replacement work.

Acceptance distinguishes Accepted, AlreadyAccepted and NotAccepted, returning an opaque DB-issued receipt for the first two. Same identity and cause replay preserves stored evidence; different cause/source/identity cannot overwrite it. Exact settlement reloads conversation/message/source plus interruption kind/Close attempt. Natural Completed/Failed settlement finding accepted evidence returns the retained interruption for interrupted settlement/recovery rather than publishing a natural outcome. No free-standing Cancelled proposal is added.

All raw occurrence writers and settlement/recovery consumers migrate coherently. A publisher-only override removal is still unsafe. Add negative deletion, wrong Close membership/phase, same-ID/cause replay, different-source collision, accepted A → answer B rollback/settle/retry, and ordinary unaccepted A → B tests. Existing cardinality and historical outcomes remain unchanged.

### Allocation status

Reviewer's bounded scan found canonical migration ceiling 129, reservations 130–133 for #902 and 134–137 for Goals #959. 138 is only a first-known candidate, **not reserved or allocated**. Recheck through the integrator immediately before registration; do not hand-assign it from this document. Schema/API implementation proceeds only after the concrete retention and canonical progress-owner contract review, not a new user/compiler decision.

## 19. Simplified adoption contract — supersedes external retry proposal

Canonical owner proved ordinary QuestionResponse has no retained original operation after persistence failure. Therefore section 18's external settle/retry proposal is rejected: do not build a saved-response/retry subsystem to make it work. Independent reviewer subsequently accepted the narrower same-transaction alternative, conditional on actual admission fences:

- Exact replay preserves evidence; unaccepted replacement remains normal.
- Only accepted **user_stop** may be terminalized and replaced within the existing execution-start transaction, after re-reading a quiescent durable predecessor (Idle or exact AwaitingUserResponse boundary), ordinary mutation admission, and absence of an owning direct turn. Exact old terminal publication/deletion and new response/replay/state either all commit or all roll back.
- Accepted **close** is not finalized by new-work adoption. Revalidate the Close fence in the same transaction and return CloseAdmissionBlocked without replacing its evidence; existing Close teardown owner settles it.
- Non-quiescent accepted Stop returns AcceptedStopNotQuiescent unchanged. No blanket settlement rule for raw writers, and no inference that accepted cancellation proves physical teardown.
- Delayed runtime Stop/Close delivery carries exact old identity and cannot affect new work after atomic replacement.

Concrete canonical-owner counterexample: an in-memory validated QuestionResponse can race Close acceptance before response persistence. The old canonical write lacks a same-TX Close admission/wait/owner fence; unconditional settlement+adoption would commit LlmRequesting despite Close. The new helper must check that boundary, not merely accepted fields. Accepted evidence alone does not authorize new work. No new retry owner or scheduler is proposed.

### Narrow retention enforcement candidate

Keep existing parent CASCADE behavior for ordinary rows. Reviewer proposes a same-row one-way `settled_terminal_kind`, constrained to NULL or cancelled with accepted provenance, as a real settlement witness. SQL guards prevent deleting accepted-unsettled rows (including parent deletes) and changing accepted identity/cause; exact settlement changes NULL→cancelled, publishes, and exact-deletes in one transaction, so any publication/commit failure restores the whole old fact. No session bypass flag, event-existence heuristic, blanket restrictive parent FK or new lifecycle table. Close receipt FK is restrictive and membership/phase validation remains required. This is still a reviewed candidate for coherent schema implementation, not an applied migration or policy allowing indefinite Close/deletion blockage.

## 20. Schema-independent direct-turn acceptance implementation slice

Added `WorkflowRepository::accept_direct_turn_stop(turn_id, expected_generation)` that invokes the existing Cancel transaction and returns an opaque private-field `AcceptedDirectTurnStop` only for its exact Cancelled Terminal/TerminalReplay result at the expected successor generation. No new persistence record is introduced. Exact replay reuses existing command history; stale/terminal conflicts become typed NotAccepted; corruption/storage errors remain errors.

The existing projection-less terminal-obligation guard now raises a typed DbError variant rather than a serialization string. Its transaction location/rollback behavior is unchanged. The API's exhaustive internal-error mapping adds this variant, preserving the existing HTTP classification. Added three unexecuted DB regressions for exact Cancel replay versus wrong generation, completion obligation winning without generation/owner mutation, and Cancel winning before old completion-obligation insertion.

This slice is not yet wired to StateStore/runtime ingress and does not create a success-default acceptance API. It provides actual durable evidence for that next wiring independently of the occurrence schema. The direct request target-validation slice remains unqualified after the first event slice; new DB tests also remain unexecuted. No new hosted run is authorized or started by this commit. The deliberately tests-first watch regression remains expected red until publisher authority is migrated; do not call this branch merge-ready.

## 21. Direct acceptance review correction and required storage plumbing

Independent exact-diff review of 8af68 found one wiring blocker: a same-process SQLite Cancel can commit before acknowledgement fails. Added one exact post-error classification query on turn ID, expected successor generation, Cancelled terminal and owns_conversation=0; that yields the same opaque receipt. Exact original live generation/NULL terminal/owns=1 preserves the original error. Anything else or query failure is typed unclassifiable authority error, never NotAccepted or guessed normal completion. Corrupt aggregate errors remain hard errors rather than being converted to cancellation.

Added BeforeCommit/AfterCommit existing-cut classification regression; corrected wrong-generation replay oracle to generation 1; strengthened Stop-first obligation test with typed stale conflict, zero obligation rows and actual durable generation 1. The existing obligation-insert stale error payload can report input generation rather than actual generation; these tests verify actual SQL authority independently and do not endorse that payload as an exact fact. Its canonical error-detail repair is a separate exact-hunk coordination item if needed, not a new arbitration rule.

StateStore now requires `accept_direct_turn_stop` with no default implementation. Arc forwards it; DatabaseStorage calls the repository; InMemoryStorage explicitly fails closed because it has no durable Cancel transaction. The targeted runtime request calls that sink before abort and verifies its private receipt against captured ID/generation. Typed NotAccepted returns without teardown; storage failure remains error. An execution-level accepted receipt cannot be overridden by a mere LLM-attempt Completed result. Generic UserCancel handling is unchanged during this intermediate migration. Added a storage-failure-preserves-model-task regression. No production sender is switched yet and sticky inference remains for unmigrated senders.

These source changes are not compiled/tested. No new CI budget/run is inferred. Next source review must verify exact-error reconciliation and guard order, then admit a focused execution lane; schema/standalone occurrence work and canonical adoption remain coordinated separately. Existing publisher regression remains intentionally expected red, so whole-branch tests/full qualification are not claimed.

## 22. Acceptance/projection split is a wiring blocker

Independent review of ec95 identified that projection-less Cancel commits terminal/release/watch before runtime teardown; later TerminalReplay returns before applying the runtime projection. Thus a matching targeted request must NOT be activated in production from this intermediate packet. Target types and stale rejection remain useful, but the accepted path is not qualified. Added `accepted_stop_finalization_must_commit_the_reducer_projection`, a real-DB regression reproducing active durable state → accepted Cancel → existing projection-bearing replay; expected red by source inspection, not executed.

Do not introduce a second generic finalization protocol to patch this automatically. First compare two existing boundaries: if the pure reducer already selects the actual terminal target, commit that projection, exact occurrence and replay disposition with the existing Cancel transaction at acceptance; teardown is an idempotent postcommit effect. If Stop enters CancellingTool/CancellingSubAgents, the terminal reducer target is not available yet; blindly persisting Idle would change drain/queue behavior. In that case evaluate an accepted Cancelled **existing terminal obligation** retaining owning authority until existing terminal settlement, with existing recovery discovering the owed work. Its dispatch fence, exact replay and first-payload arbitration must be proved before adoption; no new lifecycle authority or shadow cancellation table is justified.

Next end-to-end acceptance criterion: actual DB-backed runtime request with reducer target, projection, source occurrences, watch count and replay disposition agree at the commit boundary, plus crash after acceptance/before teardown and recovery. Helper receipt tests alone cannot prove it. Question/queued input and Stop-between-model-requests must use existing execution authority, not attempt-terminal metrics. The earlier section 21 wiring description is historical intermediate source, superseded by this blocking disposition. No production ingress switched, merge, deployment or additional CI run occurred.

## 23. Corrective terminal-projection packet and exact drain gap

The targeted request now previews the existing pure cancellation reducer before storage acceptance. Only a reducer-selected terminal Idle target is passed to the existing projection-bearing Cancel transaction. That transaction commits turn terminal/release, actual durable conversation projection, captured interaction source and watch outcome together before idempotent runtime teardown. It is not a hardcoded projection for arbitrary states. The repository rejects nonterminal projection input; runtime fails closed for a drain preview before any acceptance or abort. Production senders remain on the old path until all cases are coherent.

Projection-bearing Cancel must not bypass a previously established Completed/Failed obligation: the existing in-transaction obligation guard now also rejects that conflict for Cancel with projection. A matching Cancelled obligation remains consumable by its normal projection-bearing settlement. This is the exact shared arbitration hunk for independent/integrator review; ordinary checkpoint code is untouched. Exact replay/commit-error receipt verification additionally checks the persisted reducer state and absence of a replacement owning turn; a historical projection-less Cancel is not treated as proof of projected settlement.

Added a real DB-backed runtime regression: accepted direct turn in persisted LlmRequesting, targeted request, actual DatabaseStorage acceptance/teardown/settlement, durable Idle and exactly one Cancelled watch event, runtime drop, exact command replay and no pending terminal obligation. Existing crash-cut regression now begins with active persisted state and asserts BeforeCommit retains it while AfterCommit persists Idle. These are source tests, not yet executed; runtime drop/replay is not a claim that every startup recovery path was exercised.

**Exact nonterminal gap:** a cancellation reducer can select CancellingTool/CancellingSubAgents. Existing terminal obligations retain a final terminal target/response and are loaded as owed final settlement; they do not retain an accepted nonterminal cancellation state, pending child/tool identities and dispatch fence. Writing terminal Idle there before teardown would bypass the actual reducer direction, queued-input ordering and physical-drain owner. Therefore this packet does not force a terminal obligation or introduce another authority record for that gap. The new targeted variant rejects it while production ingress remains unchanged. The coherent remaining choice must reuse existing accepted-turn state/checkpoint ownership to persist the real drain state and accepted causal direction before effects, with terminal publication only at actual final settlement; exact schema/transaction signature is subject to review, not another lifecycle subsystem.

No new migration, standalone occurrence implementation, publisher-override removal, production ingress switch or CI run is included. The intentionally red legacy-publication regression still prevents claiming a whole-fix green branch. Next action: independent exact packet review, then host admission for a bounded compile/focused lane rather than unbounded reruns.

## 24. Exact 6984 review rejection and corrective handoff

Independent reviewer rejected 6984 on three source-proven blockers: acceptance timestamp A versus ordinary transition timestamp B caused exact settlement reconciliation failure; acceptance preserved replay despite terminal Idle; replay/error comparison omitted timestamp and could bless an old unprojected Cancel. No qualification run was dispatched from that rejected packet.

Correction carries the receipt and selected projection as a local value through process_event. After the pure reducer recomputes the terminal target and immediately before classification/apply, verify target/result and install the already-committed exact state/time with `install_live_state`. Existing apply_transition_result_inner observes no state change and reuses that timestamp; its existing exact terminal reconciliation can recognize the committed projection rather than fabricating a second timestamp. No reserved apply body, Goal checkpoint algorithm, additional finalizer or mutable shadow timestamp field is introduced. This is subject to independent confirmation, not a source-pass assertion.

Acceptance derives replay settlement from the same terminal state (Idle → Clear), closing the acceptance-to-abort crash window in the original transaction. Exact replay/error proof parses persisted state/time and uses existing `projections_match`, plus no replacement owner. Replay using a fresh timestamp is now a negative assertion, not accepted evidence. Runtime DB regression begins with a valid nonempty replay payload and verifies it is absent after Stop; replay uses the actual installed timestamp. Before/AfterCommit tests use the same fixed exact projection and correct Clear disposition.

The round-2 workflow is prepared but uncommitted/unpushed until corrected independent review clears. Host reviewed its exact command/resource/trigger contract; only the reviewed source-parent literal may change without renewed host review. No compile/test run, production ingress activation, schema/migration, merge or deployment occurred. Standalone occurrence, nonterminal drain and publisher authority remain incomplete.

## 25. Corrected packet still held: preserve source state explicitly

Reviewer rejected 45aa because preinstalling Idle erased the original reducer source: queued steering drain and AbortLlm trusted-result cleanup depended on that source. Also fixed the malformed Clear struct constructor in the crash-cut test and a declaration audit found/corrected the missing conversation argument to `for_conversation_state`. These are source corrections, not compile evidence.

The real DB regression now covers both no-queue and queued-steer cases and seeds pending trusted results, asserting cleanup and immediate queued-work progress. The preinstall workaround is still present until the exact tiny reserved timestamp selector is yielded; this packet therefore remains rejected and must not run or activate production ingress.

Exact requested seam: preserve existing `apply_transition_result_with_input` callers via a None-default wrapper, thread an optional already-committed projection through the existing inner call, validate its state equals the reducer target, and choose its exact timestamp instead of generating a new one. Keep original `old_state` untouched for cleanup/drain. This is one private optional projection argument, not another lifecycle, retry state or finalizer. Goals integrator has been asked to yield this narrow signature/call/selector hunk or assign its sole writer to apply the exact diff. No generic ownership wait is claimed; this is the specific remaining source dependency. Host reviewed round2 workflow remains uncommitted/unpushed and no qualification run occurred.

## 26. Yielded exact projection handoff replaces preinstall

Goals integrator explicitly yielded only private wrapper/signature/call/timestamp-selection hunks. Implemented `apply_transition_result_with_projection`: existing with_input delegates None, ordinary callers retain behavior; accepted targeted Stop alone supplies the local receipt-validated committed projection. Wrapper checks target state; inner retains original source state and timestamp, selecting the committed timestamp for install. Removed preinstall. No persistence/Goal checkpoint body, replay owner or postcommit effect was skipped or rewritten.

The existing named DB-runtime test now exercises both queue-empty and queued-steer variants with pending trusted results. It asserts trusted cleanup, queue consumption and model-requesting progress for queued work; the no-queue case asserts Idle, one terminal event, replay clearing and exact projection replay. Compile declaration corrections supply Clear's conversation_id and for_conversation_state's conversation argument from exact turn binding. These changes are not yet executed. No nonterminal drain activation or standalone occurrence migration is claimed.

Next immutable source review covers this exact handoff plus source fixes, not only docs. Round2 workflow remains held until independent clearance; only its source-parent literal needs adjustment under existing host admission if test names/resources remain unchanged. No new CI run or broad green claim.

## 27. Pre-mutation stale replay guard

Exact f751 review closed prior source-state/constructor blockers but found a stale replay mutation: replay Clear ran and committed before postcommit classifier could reject projection mismatch or replacement ownership. Added the exact projected-Cancel replay predicate inside the existing terminal transaction **before** replay clearing: persisted state/time must match via projections_match and no owning replacement may exist, otherwise error rolls back without adjunct mutation. Scope is Cancel with projection; projection-less Close and unrelated terminal commands retain their existing behavior. Post-error classification remains a read-only reconciliation layer, not permission to mutate first and reject later.

Historical projection-less negative now seeds a provider replay row and asserts its exact payload bytes survive rejection. This is a storage nonmutation assertion; it does not decode/send the deliberately opaque payload. No historical row or outcome is repaired. Source review and round2 run remain held; no automatic retries or new execution occurred.

## 28. Round-2 executed result: compile failure, no behavioral qualification

After independent source clearance of ff9bd30f, workflow successor 820165c2b8e4e96c741b2c0afa38c2eb4a22f4cf ran as 39718164656 attempt 1/job 119625177524. Terminal FAIL in 3m33s; compile exit 101. All ten selected regressions NOT RUN because compile failed closed. Three introduced compiler errors: two `?` uses on bool-returning projections_match, plus an exhaustive DbError mapper in retrieval missing both added variants. These were missed by implementation/declaration review; source clearance was not compilation evidence.

Artifact 12400012069 digest sha256:99e25aa47b2ca4e85b9efc4be4037a8212f1aa4dedfbb888861edcbad0430335 retained exact commands/compile log/exit/source and disclosed expected-red watch test. Upload and guarded runner-target cleanup succeeded. No rerun authorized or started. Minimal source correction removes the two invalid operators and adds explicit variants to the unchanged retrieval internal-only error classification; no semantic predicate or runtime authority change. This successor requires source review and fresh bounded execution admission before any replacement run. First-slice run 39710488682 remains separate passing evidence only for its original scope.

## 29. Downstream error-consumer inventory after compile correction

Independent review rejected 87ac before rerun: compile had stopped inside phoenix-db, leaving three downstream exhaustive IDE mappers uncompiled. Added both new variants only to existing Internal arms in send_chat_service::map_conversation_load_error, map_direct_turn_accept_error and runtime/fork_resolve::map_db_resolve_error. Inventory using the previous enum tail sentinel identifies five production full-enum consumers: retrieval, global_read, the two send-chat mappers and fork-resolve; all now explicitly include both added variants. The two bool predicate calls and definition were also enumerated. This is source inventory, not a passing compiler result. No replacement qualification or runtime test has run.


## 30. Successor fixture correction and declaration audit

Replacement run [39719171533](https://github.com/scottopell/phoenix-ide/actions/runs/39719171533), exact c005a381, failed all-target compilation with exit 101. All ten selected regressions were NOT RUN. Both diagnostics were in the new executor fixture: dependency-private cfg(test) replay helper and Arc<Database> passed to DatabaseStorage::new(Database). Artifact 12400257143, digest sha256:7f0ef4fa0414409bbf9c66f42457e23f6c87741c5e6742c5fa767c6e08ca1d2f, retained logs; upload and guarded runner-target cleanup succeeded. This consumed the prior replacement, not the subsequently admitted conditional successor.

After the WorkScope launch fence, no alternate launch, writer, cleanup or automatic retry was used. Same-owner post-restart bounded observation re-established original worktree identity, clean c005a381 and only the two terminal failed round-2 runs before edits resumed. No local compilation or tests accompany this correction.

The corrective packet changes fixtures only: plain cheaply cloned Database; production update_state_and_replay with validated AnthropicResponseSet; actual accept/claim/materialize setup and canonical message assertion; broadcaster sequence initialized after that message; exact durable steering admission before runtime queue insertion and durable consumption/message assertions. The projection-finalization fixture reuses its one accepted projection and verifies persisted microsecond timestamp. Its setup and crash-cut fixture set coherent state/state_kind columns.

The workflow and its ten historical filter names are unchanged. In particular, the executor name ending `and_restart` is a retained qualification filter, **not a restart-recovery claim**: it drops the runtime and replays the exact Cancel against the same repository. It does not reopen a database or instantiate manager startup recovery. The queued variant verifies source-state-sensitive trusted cleanup, actual durable queue consumption and new model-requesting projection; it does not claim all startup/crash paths or provider delivery acceptance.

### All-ten declaration/type audit (source-only)

| Named filter | Declaration, constructor and invariant audit |
|---|---|
| accepted_direct_turn_stop_replays_exact_command_not_replacement_generation | Repository acceptance takes exact turn ID/u64 generation/reference projection; private receipt matches exact target. Fixed 1us replay and 2us negative are distinct. This does not construct a real replacement owner. |
| established_terminal_obligation_rejects_stop_without_revoking_owner | DirectTurnTerminalObligationInput accepts optional response ID. Serial completion-first oracle checks typed rejection, retained Completed obligation and durable generation/owner. |
| accepted_stop_fences_later_terminal_obligation | Stop-first path checks typed stale conflict, zero obligations and durable successor generation; no concurrent scheduler proof inferred. |
| accepted_stop_finalization_must_commit_the_reducer_projection | Same local projection flows into acceptance and TerminalizeAuthoritativeTurnInput; exact persisted timestamp and Idle are asserted. Setup has coherent llm_requesting discriminator. |
| stop_commit_error_is_classified_from_exact_durable_fact | Private transaction-cut and classifier calls are accessible within DB unit module; exact fixed projection reused; coherent state discriminator. BeforeCommit/AfterCommit are synthetic cut evidence, not actual failed SQLite acknowledgements. |
| db_backed_targeted_stop_commits_projection_before_teardown_and_restart | DatabaseStorage::new owns cloneable Database. Public replay update takes ResponseSet, not Payload; cfg(test)-only store is not exposed. Claim/materialize input matches existing runtime restart fixture declarations; established result must be Materialized. Durable append_steering_entry receives exact local entry and fingerprint. Queue depth and canonical/drained message observations use public DB APIs. Same-repository replay only, as qualified above. |
| targeted_stop_does_not_fabricate_idle_for_nonterminal_drain | Exact targeted event and CancellingSubAgents constructor are valid; preview rejection is before durable acceptance. Synthetic empty child list is not physical child-drain proof. |
| direct_stop_storage_failure_cannot_abort_model_work | InMemoryStorage explicitly rejects durable Cancel; task remains live until explicit test cleanup. This is not a real DB uncertain-commit test. |
| stale_direct_turn_stop_is_rejected_before_teardown | Stale ID/generation event types match declarations; task survival and explicit cleanup test pre-teardown rejection. No durable replacement-owner scenario inferred. |
| direct_turn_stop_cannot_fall_back_to_unowned_parent_work | No active target; event must not fall back to generic UserCancel. Retained requesting state/no pending terminal oracle is narrower than actual task-survival proof. |

Shared API audit: create_conversation is available via test-support; dependency cfg(test) alone is not. Context/database IDs are conv-direct throughout executor setup and conv-a throughout repository fixtures. Constructor argument order and Arc mock forwarding match runtime traits. Existing five full-enum DbError consumers (retrieval, global_read, both send-chat mappers, fork-resolve) retain both explicit new variants; this fixture-only delta adds no enum or production consumer. Fixed-schema comparator uses timestamp_micros, not nanosecond identity.

Independent whole-frozen-packet review and host exact-correction/unchanged-workflow/no-duplicate admission remain required before the one conditional successor push. No production Stop ingress switch, standalone occurrence migration, legacy publisher fix, grouping, historical outcome rewrite, merge or deployment is included. The excluded legacy-watch expected-red test remains unresolved; passing these ten would qualify this slice only.

## 31. Executed arbitration evidence and canonical-identity fixture correction

Run [39854804491](https://github.com/scottopell/phoenix-ide/actions/runs/39854804491), exact 3c559c99, completed in 24m38s: four-package all-target compilation exit 0; all ten named regressions executed, nine passing individually with exit 0. The real-DB accepted-Stop regression failed with exit 101 before Stop processing: its setup queried submitted ID `stop-input`, while `canonical_message_id_for_turn` namespaces the persisted ID by conversation. The audit and independent source clearance missed that distinction. The five repository arbitration/projection/cut tests and four runtime rejection tests have executed evidence; accepted real-DB Stop, cleanup, steering, replay and watch assertions were not reached and remain unqualified.

Artifact 12460272577, digest sha256:fab473972e5f33a598b389c358cfd4ae207a889786db285d36f666e87ae418d1, retains exact commands, names, counts, exits and logs. Artifact upload and guarded runner-target cleanup succeeded. Local evidence is retained under /tmp/cancellation-98021-39854804491. No local compilation, automatic rerun, merge, deployment or production ingress activation accompanied diagnosis.

The narrow corrective delta destructures the actual Materialized result and looks up its authoritative message ID, asserting persisted identity, conversation and sequence. It does not duplicate the namespace algorithm or remove the persistence oracle. All downstream Stop/cleanup/queue/replay/watch/exact-replay checks remain unchanged. The queued steering identity follows its separate existing drain path and must be independently checked rather than assumed to use canonical direct-turn namespacing. The unchanged-workflow successor remains gated on exact independent review and fresh host admission/no-duplicate guards under standing diagnosed-correction approval. Legacy watch misclassification and inactive production ingress remain disclosed gaps.

## 32. Qualified terminal-projection slice, not whole cancellation closure

Run [39858948853](https://github.com/scottopell/phoenix-ide/actions/runs/39858948853), exact a0053d34d6e174276c33925f572abcd79691b7fa, attempt 1/job 120104336147, completed successfully in 24m24s under the unchanged two-job/two-test-thread/30-minute Linux workflow. Four-package all-target compilation exited 0. All ten separately invoked named regressions each report the exact test name, one passed, zero failed and exit 0. The real-DB regression now reaches both no-queue and queued-steering variants rather than failing during setup.

Executed properties include exact terminal reducer projection/timestamp acceptance and replay; completion-first versus Stop-first obligation arbitration; before/after commit-cut classification; rejection of stale, unowned, storage-failed and nonterminal-drain requests without improper teardown; accepted real-DB Stop with trusted-result cleanup; durable queued steering consumption/materialization and progression; and no-queue owner/task/replay clearing with exactly one watch, no pending obligation and exact same-repository replay. The synthetic-cut, local-stale-target, empty-drain-child and same-repository-not-startup limits in section 30 remain. Helper-expiry evidence remains its separately scoped earlier run; neither run alone proves the whole original incident fixed.

Artifact 12461767573, digest sha256:45c89f1f66dab244e9ea3df4814a31cc5a50b3a8afeee5b81a2bcc9eca8ccd67, retains commands, source HEAD, logs, per-command exits and raw counts. Artifact upload and guarded runner-owned target cleanup both succeeded. Full downloaded evidence is retained at /tmp/cancellation-98021-39858948853. The historical source-parent ff9 literal is qualification lineage; head.txt records actual tested a005. Prior failures and artifacts remain preserved, not relabelled as passes.

Production ingress is not switched. Generic cancellation/sticky inference and the excluded legacy-watch override remain unfixed; targeted nonterminal drain and standalone occurrence acceptance are incomplete. Therefore this is not whole-branch required-check completion, merge readiness, activation, or deployment. The next source packet must use existing settlement/checkpoint projection ownership to carry real drain state and exact causal authority before effects, without another lifecycle/retry framework or historical outcome rewrite; shared Goals seams retain their existing owner except explicitly yielded hunks. No additional execution is inferred from this passing result.

## 33. Nonterminal direct-turn Stop source packet (unqualified)

The next source packet uses the existing conversation projection and exact owning turn/generation as durable cancellation evidence, not another cancellation table, scheduler, lifecycle, retry loop or finalizer. `UserRequestedDrainProjection` has a private field and admits only reducer-shaped user-requested CancellingTool/CancellingSubAgents states. Its separate accepted-drain receipt cannot be confused with the already-terminal Cancel receipt.

`accept_direct_turn_drain` binds conversation, turn, generation and the exact source projection used for preview. One immediate transaction admits only a live owner with no terminal obligation; it compares source state/microsecond timestamp or the exact already-committed target, then changes only state/state_kind/timestamp. It retains generation, ownership, replay, occurrence rows and watch events. A reported commit error is classified from the exact target+owner+generation+no-obligation tuple in a fresh immediate transaction; no receipt is returned on an unproven fact. A shared direction guard at the existing terminal-obligation sink and direct terminalization sink rejects Completed/Failed while user-requested drain is durable. First committed completion obligation still rejects subsequent drain acceptance.

Runtime targeted Stop selects either terminal or draining receipt, preserves the pre-transition source, and passes the committed projection through the existing apply wrapper before abort/cancel effects. StateStore/Arc forwarding/DatabaseStorage/test storage thread the distinct drain API explicitly. Final classification derives cancellation from the actual old user-requested drain even with empty process history; Timeout remains distinct. The old generic sticky flag is not removed in this packet and generic ingress/watch override is not switched.

Existing startup recovery is reused, not replaced: reset unions standalone user-requested child drains into its existing parent reconciliation set before generic reset, and no-fan-in user tool drains create the existing Cancel action rather than Resume. Existing action owner/generation capture and RuntimeManager cancellation settlement remain unchanged. These changes do not touch Goals-owned ToolRound helper moves, admitted-operation routing, Product send branch, or migration 138.

Eight added source regressions (NOT RUN) target:
- direct_stop_drain_preserves_owner_and_defers_watch_until_terminal (including replay preservation, rejection of direct Complete and Completed/Failed obligations, final ordinary Cancel/watch);
- direct_stop_drain_rejects_stale_source_target_and_completion_first;
- direct_stop_drain_commit_cuts_require_exact_durable_projection;
- direct_stop_drain_type_rejects_idle_and_timeout_direction;
- db_backed_targeted_stop_retains_drain_owner_projection_and_replay;
- durable_drain_direction_classifies_without_process_cancellation_history;
- reset_preserves_standalone_user_child_drain_as_cancel_action;
- reset_retains_user_tool_drain_cancel_direction_without_fan_in.

The real-DB runtime helper retains its prior passing model/no-queue and model/queue variants, now additionally supports a timeout-child-drain source and loads the authoritative persisted timestamp. The drain fixture has empty pending children and does not prove physical child teardown, a full live ToolAborted checkpoint, or owner-bound manager restart settlement. Reset fixtures invoke the real reset/action path but have no direct-turn owner; they are not proof of exact owner recovery. These limitations require review/stronger regression coverage before production activation.

Only rustfmt parsing and diff checks have run locally. No compile/tests/push/workflow execution has run for this packet; the unchanged workflow still selects the earlier ten tests only and cannot qualify the eight additions. Exact independent source review, declaration/callsite audit, explicit finite named test packet and host resource/no-duplicate admission precede any hosted qualification. The a005 passing evidence remains scoped to its original source and ten tests. Whole-fix, required-check, merge and deployment claims remain prohibited.

### Pre-execution declaration audit corrections

A bounded read-only declaration audit of d47 found two compile blockers before any execution was admitted: the existing transaction-cut helper accepts WorkflowTx, not raw sqlx::Transaction, and the classifier fixture referenced a private helper in a sibling test module. The correction wraps the existing immediate transaction with WorkflowTx::new at its cut/commit sink and constructs the classifier state locally with an explicitly qualified PendingSubAgent type. No transaction ordering, authority, SQL or runtime behavior changed in this audit correction. All StateStore implementations/Arc forwarding, SQL bind counts, replay table shapes, canonical fixture identity/sequence/timestamp, AssistantMessage constructor/test-support visibility, Cancel-after-Cancelled-obligation path, and reset materializer source retention were inspected. No additional deterministic assertion failure was established statically; this remains uncompiled/unexecuted source and is not review clearance.

### P1 recovery-authority correction (before qualification)

Independent full-packet review rejected b72: reset replaces the user drain state with Idle before the existing exact Cancel action is consumed, so a state-only terminal guard reopens Completed/Failed authority after a crash. No hosted execution was requested or admitted for that rejected packet.

The shared guard now consults either active user-requested drain state or an existing pending Cancel action joined to the same conversation, owning turn and generation, with the existing startup-action transcript-generation freshness fence. It still runs inside both existing terminal sinks' transactions; no shadow flag/table or recovery subsystem was introduced. Cancel itself remains allowed and uses ordinary settlement/watch publication.

The added source regression `startup_exact_cancel_action_fences_both_terminal_sinks_for_real_owner` accepts, claims and canonically materializes a Runtime turn, admits a real pending child/run/batch, commits its user-request drain, invokes real reset/reconciliation, and asserts Idle plus exact owner-qualified Cancel action and accepted child outcome. It then checks Completed and Failed rejection through both obligation and direct terminalization sinks, retained owner/no watch, exact Cancel settlement and replay, action retirement, one sole watch and owner release. This is a real DB reset/owner/child/settlement regression, not a RuntimeManager construction/startup test; live ToolAborted and physical child teardown remain separate coverage gaps. Existing tool-vs-child source-state variants remain. There are now nine added source regressions (nineteen total prospective named cases), all still NOT RUN. Fixture declarations were checked against the actual workflow claim/materialization helper signatures and replay-update enum before freezing.

A further pre-execution fixture audit rejected four inferred shapes in the new real-owner regression: Settlement::Clear requires conversation_id (unlike the unit Update::Clear); sub-agent batches require parallel_work_qualified/admitted_at_unix_micros; runs require execution_authority rather than task/mode/active fields; and parent acceptance is parent_accepted_at_unix_micros INTEGER. These fixture-only corrections now follow migrations 106 and the actual replay enum. Earlier declaration-audit language does not constitute clearance of those previously incorrect fields. No compile/SQL/test execution occurred; all findings were corrected before requesting host qualification.

### Compound-mutation ordering correction (before qualification)

Independent review rejected the corrected state/action predicate packet because the existing MessageMutation compound sink incremented transcript_generation before its inner guard, invalidating the pending Cancel action within the same transaction. The exact action freshness predicate is retained, not weakened. After an explicit bounded Goals hunk yield, `persist_sub_agent_terminal_evidence_at_cut` now invokes the same direction guard through a crate-private WorkflowRepository wrapper immediately after its existing transaction opens, before message/FTS/transcript mutation. The obligation is non-optional in this actual method; transaction mode and all later checkpoints/inner guards are unchanged.

The real-owner reset regression now seeds a real tool-result evidence carrier and invokes the public compound MessageMutation sink with both Completed and Failed obligations while Idle + exact Cancel is pending. It asserts the specific guard error, unchanged carrier content/display metadata and transcript generation, zero obligations, unchanged workflow version/generation/status, retained owner and no watch, then ordinary exact Cancel settlement/replay/action retirement and one watch. This extends the same named regression rather than increasing the proposed nineteen cases. Declarations were checked against `ToolContent::new`, `MessageContent::Tool`, the public seven-argument add_message_with_seq and compound sink signatures.

Goals retains sole ownership of its no-semantic constructor/process_outcome/ToolRound/send helper moves. Only the precise DB compound pre-guard prefix was yielded to this cancellation writer; no moved helper body, classifier-call ordering, admitted-operation/reserved-range lifetime, or Product branch was integrated or changed here. The source packet remains unqualified until exact independent full re-review and separately reviewed finite hosted packet; no local compilation or execution occurred.

### Finite nineteen-case hosted packet (not admitted or executed)

Retained independent reviewer 76f8 source-cleared the full immutable e6d..1e34 source after both P1 corrections, with no blocking finding established. That source clearance is not compile/test evidence. The separately reviewable workflow delta replaces substring filters with one nineteen-entry package/fully-qualified-name manifest, `cargo test --lib ... -- --exact --nocapture`, and strict per-command verification: exactly one case line, exactly the named passing case, exactly one summary with one passed/zero failed/zero ignored, actual command exit 0, separate verification exit and disposition. Compile failure leaves every manifested case explicitly NOT RUN, without inventing test exit codes. All four original all-target packages remain; actual binary package spelling is phoenix_ide, verified from Cargo.toml and passing artifacts.

Only static Ruby YAML parsing, bash -n, manifest uniqueness checks, diff checks and count-pattern checks against two already-existing passing artifact logs ran locally. No fixture test, Rust compilation, workflow dispatch or push ran. Runner, two Cargo jobs, two test threads, 30-minute limit, no-cache target ownership, always-upload and guarded cleanup are unchanged. Historical lineage source-parent stays ff9; head.txt will identify the actual executed source. Workflow SHA256 before: 7d486dc516f0c2d27fe86fa5a4a45acf6e982703b16d5c9cf740f67c864535d9; prospective after: 588ec9d4b41efaef3b9bbf0f8124750e7cbc70c3656d873bc17eb2a678ffa655. Exact independent workflow review and fresh host resource/head/remote/no-active-or-duplicate admission remain mandatory; no prior consumed admission authorizes this packet.

### Timing-based finite timeout adjustment (source-only admission)

Before dispatch, host independently verified baseline run 39858948853/job 120104336147 took 24m24s. Actual logs split the cost into all-target compilation 3m51s, first DB test build 2m40s, repeat DB invocations about 32s plus 1.5s tests, first IDE test build 6m03s, repeat IDE invocations 2m14s–2m18s with 0–3s tests, and roughly 1s upload/2s cleanup. Nineteen tests do not imply doubling the full baseline, but conservative recurring-build extrapolation is about 32m55s. The new --lib restriction may reduce build work; that is not measured evidence and was not used to justify a 30-minute budget.

Under standing diagnosed-correction policy, host selected a finite 40-minute job limit rather than 35 (about seven rather than two minutes estimated reserve). The only workflow delta from independently cleared 28d1 is timeout-minutes 30→40. Two build jobs/two test threads, runner, all nineteen exact cases, four all-target packages, strict verification/evidence/cleanup and no retries are unchanged. Workflow SHA256 changes from 588ec9d4b41efaef3b9bbf0f8124750e7cbc70c3656d873bc17eb2a678ffa655 to 864f38037d630ef9a69d9dc26a104c8d761ce8a8fca9993af5a5bd42909e46e8. This is source-only host approval of the bounded timeout adjustment, not execution admission: exact timeout/combined-packet peer review and fresh host head/remote/no-active-or-duplicate guards remain required. No local compilation, test, dispatch or push has occurred for this root packet.

## 34. First nineteen-case packet: compile failure, all cases NOT RUN

Run [39868601214](https://github.com/scottopell/phoenix-ide/actions/runs/39868601214), exact fb6b965563b1f3766d4a755d3799214cf1095f58, attempt 1/job 120137029285, failed in 3m41s at all-target compile exit 101. Actual diagnostics were two phoenix-db test imports: SubAgentMode imported from sm_event instead of sm_state (E0432), and unqualified ContentBlock without a local import in the new tool-reset fixture (E0433). Both source/declaration audits and independent review missed these; their clearance was never compile proof.

The strict manifest initialized all nineteen explicit NOT RUN dispositions, all verified as `NOT RUN (compile gate not passed)`. No named test executed and no fabricated test exit or verification files exist. Artifact upload and guarded runner-target cleanup succeeded. Artifact 12464508691, digest sha256:57a976d0601afc6bba161c0e7b0507afb9e6a9f8f2afeea68f1b891954f46057, retains commands, HEAD, manifest, compile log/exit and dispositions. Full downloaded run/artifact/log evidence is at /tmp/cancellation-98021-39868601214. The earlier a005 ten-case green remains unchanged; no new behavioral property was gained by this failed run.

The diagnosed correction is imports only in the two test scopes: `sm_state::{PendingSubAgent, SubAgentMode}` and `llm_types::ContentBlock`. Actual public definitions and existing valid DB test imports were read before editing. Production, all nineteen tests and assertions, strict workflow, forty-minute/two-job/two-thread budget and workflow SHA 864f38037d630ef9a69d9dc26a104c8d761ce8a8fca9993af5a5bd42909e46e8 remain unchanged. Standing diagnosed-correction work permits this narrow repair; the consumed run admission does not authorize a rerun. Exact independent review and fresh host guards/admission precede any successor. No local compilation, automatic retry, ingress activation, merge or deployment occurred.

## 35. Nineteen cases executed: seventeen PASS, two missing-watch fixture failures

Run [39869588766](https://github.com/scottopell/phoenix-ide/actions/runs/39869588766), exact 4d867ae94f539396b7db657396a1e1deb71f5561, attempt 1/job 120140698176, completed in 23m38s. Four-package all-target compilation exited 0. All nineteen exact cases executed with individual command exits, verification exits, case/name/summary counts and dispositions: seventeen PASS; `direct_stop_drain_preserves_owner_and_defers_watch_until_terminal` and `startup_exact_cancel_action_fences_both_terminal_sinks_for_real_owner` failed (command 101/verification 1) at their final expected-watch-count assertion, actual 0 versus expected 1. Neither failed case is relabelled passing merely because preceding arbitration/rollback assertions were reached.

The seven newly passing cases provide executed evidence for nonterminal drain source/target CAS and completion-first rejection, before/after commit-cut classification, typed cause restriction, real-DB runtime receipt/projection/owner/replay retention without an early watch, process-history-independent user versus timeout classification, standalone child reset discovery and user-tool Cancel versus timeout Resume reset direction. The original ten also pass on this source. Final exact-owner reset/watch closure remains unqualified because its case failed.

Upload and guarded runner cleanup succeeded. Artifact 12465745265, digest sha256:2d93e027af857890b90d9401c3876481ca7e462ca6646e8d58019dd75597e6bd, and full downloaded evidence at /tmp/cancellation-98021-39869588766 preserve all actual results. No omission, test retry or verification weakening occurred.

Source diagnosis follows `record_watch_event_tx`: terminal event insertion fans out only to an active `coordinator_watches` subscription for an open ordinary product. Both failed new DB fixtures omitted subscription enrollment; the passing runtime fixture explicitly calls `Database::watch_product_conversation` on the created conversation's returned product ID. Zero subscriptions correctly produce zero events; this is not evidence of a production terminal-settlement/watch defect. The minimal repair enrolls through that existing public API in only the two failed fixtures before acceptance. It keeps the shared repository helper unchanged, binds each returned ordinary product ID, and retains every zero-before-terminal/one-after/idempotence/owner/rollback assertion. No production source, workflow, nineteen names or forty-minute/two-job/two-thread budget changes. Exact independent repair review and new host guards/admission are required for any justified successor; the consumed run admission does not authorize rerun. No local compilation, production ingress switch, merge or deployment occurred.

## 36. Qualified nonterminal drain/recovery-authority slice

Run [39873607816](https://github.com/scottopell/phoenix-ide/actions/runs/39873607816), exact 6d4f90553d54fd9d6ec83a7a24dae027cd96e99b, attempt 1/job 120153701535, completed successfully in 23m27s. Four-package all-target compilation exited 0. All nineteen exact separate cases report exactly one case, the exact named pass, exactly one one-pass/zero-fail/zero-ignored summary, command exit 0, verification exit 0 and PASS disposition. The downloaded manifest, nineteen ledger rows, command/verification files and named logs were independently checked after the run.

Executed new properties now include deferred final watch publication with real subscription enrollment, retained live-drain owner/generation/replay/projection, exact source/target CAS and completion-first fencing, before/after commit cuts, cause typing, process-history-independent user/timeout classification, both existing reset direction paths, and real owner/child recovery through an exact pending Cancel action. The latter now passes all Completed/Failed standalone/direct and public compound MessageMutation rejection oracles: unchanged content/display/transcript/workflow, zero obligations, retained owner and no early watch, followed by normal exact Cancel settlement/replay/action retirement, owner release and exactly one subscribed watch. Prior failed runs retain their original failure status and artifacts; no assertion, case, strict gate or workflow budget was weakened.

Artifact 12467764465, digest sha256:9d8a7aea30f625b00f9d667df033d9b2e5a51edaf4ded66d9b6941dcb651c2e3, retains actual commands, names, counts, exits, dispositions, logs and source HEAD. Upload and guarded runner-owned target cleanup both succeeded. Downloaded run/artifact/log evidence is retained at /tmp/cancellation-98021-39873607816. The qualified source is 6d4f9055; later documentation-only commits do not silently become tested source. No additional qualification push or execution follows merely from recording this evidence.

This closes the bounded targeted terminal/nonterminal projection and repository recovery-authority packet, not the original whole production incident. Generic Stop ingress and sticky inference/publisher override are unchanged; the excluded legacy-watch expected-red regression remains unresolved. Standalone execution-occurrence acceptance, RuntimeManager/reopen/action-retirement retry, live ToolAborted/physical child drain, deployed-version trace verification and whole-branch required checks remain unqualified. No production ingress activation, merge or deployment is authorized by this green packet. Same writer/reviewer ownership continues into the next minimal causal-ingress/standalone-authority source packet, with no second lifecycle/retry framework or historical outcome repair; Goals-owned shared seams retain their agreed boundaries.

## 37. Unactivated standalone authority: ownership map and first typed slice

Goals integrator b477 explicitly yielded occurrence-only DB adoption/settlement, runtime startup occurrence recovery, new targeted-occurrence event handling/classification, natural occurrence settlement callsite in persist_state_effect, and authoritative Cancelled watch recording only for the new path. The current Goals source 03c65694/tree 054dd7fd is frozen in its own qualification and is not treated as a qualified integration base. Constructor/process_outcome moves, Goal terminal/round/revoked helpers, and the whole moved ToolRound helper remain sole-a52-owned. The minimum ToolRound terminal-write callsite is reserved: exact typed signature/argument/diff must be submitted before editing, and a52 handles integration if Goal commit/ordinary atomic write is affected. No source integration/cherry-pick or resource/activation authority follows from a hunk yield.

The first staged source slice exposes the existing MessageExecutionSource type (not a parallel enum) and introduces an opaque StandaloneStopTarget with conversation/message/source getters. `capture_stop_target` reads direct ownership and supported occurrence identity in one SQLite read transaction. Runtime ownership takes precedence; any other active owner returns a boundary rather than falling through to an occurrence. Only Steering, SeededFork and InteractionResponse are captureable; historical wake returns boundary. The target has no public field constructor, and public Stop does not call this API. The source regression seeds an actual FK-backed input message and checks all supported sources plus wake/missing boundaries; it is NOT RUN. Direct-owner precedence requires a contrasting real-owner fixture before qualification.

Fresh source reading corrected a trace ambiguity: existing settle_execution_occurrence accepts message ID only, re-reads source kind and deletes by conversation/message; it does not bind captured source kind. New targeted acceptance and authoritative settlement must bind both, including same-message/different-source replacement. Existing natural initial-outcome cleanup deletes by conversation and also requires exact authority migration. No acceptance, Cancelled terminal variant, effect receipt, late-terminal guard, recovery completion, marker retirement or ingress switch is implemented by this first slice. It is deliberately not standalone-path completion and has no compile/test evidence. The qualified nineteen-case 6d baseline and its artifact remain preserved; no workflow change or execution is included.

### Staged standalone drain acceptance (still unactivated and incomplete)

The next slice adds a distinct opaque accepted-drain receipt and one immediate transaction beside the snapshot API. Acceptance checks exact conversation/message/source plus absence of *any* live direct owner, compares exact source projection or exact committed target, and changes only state/state_kind/time. Occurrence, replay, markers and watch fanout are untouched. A reported commit error releases the original connection before fresh immediate read classification and returns a receipt only for the same exact retained target and projection. The Connection trait import is explicit in this new module; SQL bind counts and enum paths were checked from actual declarations.

New unrun source regressions contrast live Runtime versus Steering ownership over a supported occurrence, stale capture after owner replacement, same-message/different-source re-adoption, stale source timestamp, exact drain receipt/replay, retained occurrence and zero early watch/marker. Owner setup uses the existing accepted-turn helper; no direct owner is fabricated by raw SQL. They do not yet exercise a commit cut, terminal receipt or natural settlement/recovery. No test, compile, workflow change, push or dispatch accompanies this staging. Existing nineteen-test workflow cannot qualify new tests and remains unchanged. Standalone late-terminal guards, exact-source settlement migration, Cancelled publisher authority, runtime receipt effects and startup direction are mandatory unfinished consumers; no path/packet-complete claim is made.

Independent staged review rejected the initial owner-contrast fixture: AcceptedDisposition::Steering is non-owning (`owns_conversation=0`), not a second owner variant. The fixture now asserts the actual distinction: Runtime owns and fences the standalone target; Steering does not own, capture retains the occurrence, and its presence does not reject exact standalone drain acceptance. No corrupt non-Runtime owner is fabricated. The production owner query was already correct; its non-Runtime Boundary branch is defensive against inconsistent historical data, not a normally constructible authority. Earlier wording about a live Steering owner is superseded by this correction. No execution was requested for the rejected fixture.
