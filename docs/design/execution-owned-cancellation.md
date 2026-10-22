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
