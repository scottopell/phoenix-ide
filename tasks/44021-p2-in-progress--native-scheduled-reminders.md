# Phoenix-native scheduled reminders: reassessment after wake history audit

## Status and scope

The user approved implementation, local adversarial review, push, PR creation and iteration until review clean. Implementation is in progress on current main. Deployment, recurrence and replacement wake infrastructure remain outside scope.

This task supersedes the initial draft's recommendation to introduce a reminder workflow profile and new reminder-specific runtime-acceptance/lifecycle bookkeeping. The first design inspected an old checkout without checking later main history. Current main has removed operational wake execution; dormant-looking code here is not evidence that it should be reused.

Confirmed user choices in this session:

- "Yes, pit of success" for agent-facing tool design.
- "Show reminder and wake agent" when due.
- "Start with durable reminders and agent wake; add closed-app phone alerts later."
- Subsequent direction: "do a deep dive into the repos complicated history with the wake subsystem, it was determined to be overengineered and not useful, hence the quasi retirement. But always happy to revisit".
- Delivery clarification: "I do not have a requirement that these only be delivered at idle" and "If delivery is done via standard direct turn then any alert should be queued appropriately and delivered when the conversation is ready". The earlier Idle-only restriction was an agent-added assumption and is withdrawn. Use standard input admission and its existing queueing/readiness behavior, including steering where that is the ordinary path.

The concrete use case is a user-requested reminder about Safari Web Inspector connection checks and disk-storage screenshots tonight Eastern. It is a future time trigger, not an obligation to observe a running process. No actual reminder has been scheduled by this design session.

## Investigation boundary

Repository and GitHub source/history/PR investigation only; no production database inspection, model execution, device QA, or deployed-version verification. Live GitHub main and cached origin/main both resolved to ce4b0bffdad0b11eb156d91c3584ff8767f60a02 during this audit. This checkout predates wake removal. GitHub Issue #806's generated body was consulted; no roadmap records were posted.

## History and evidence

### Product need and original contract

- May/June: agent monitoring-overpromise feedback was captured in task 35394. A running background process does not imply an agent is observing it. This is the same honesty gap illustrated by promising a future reminder without a scheduling capability.
- July: [PR #405](https://github.com/scottopell/phoenix-ide/pull/405) merged the terminal-handle contract. ADR-006 promised accountable terminal results for Bash/tmux/sub-agent waits while keeping the parent Idle. It did not establish arbitrary reminder scheduling.
- ADR-011 changed delayed original-tool results to immediate registration receipts plus durable later observations, to preserve provider-valid history across new messages and continuation. ADR-012 added a durable acceptance outbox after identifying a crash gap between consuming an observation and accepting its model turn.
- A June 20 history note, commit 612d4505b, already distinguishes wall-clock conditions: no owned handle and no meaningful Forgotten result. Clock reminders were never merely another terminal-handle evaluator.

### Generalization and its first correction

- [PR #471](https://github.com/scottopell/phoenix-ide/pull/471), closed unmerged, proposed a broad end-to-end wake plane: 104 changed files, 27,041 additions, 7,588 deletions at its recorded PR state. It was superseded by a decomposed shared-workflow stack.
- [PR #488](https://github.com/scottopell/phoenix-ide/pull/488), also closed unmerged, added 15,753 lines across 32 files. `docs/durable-workflow-grand-vision-review.md` records the expansion to 30 workflow/wake tables and overlapping authorities spanning engine, profile, conversation, tools, runtime admission and clients.
- That review did not reject shared durability. It rejected universal leased-worker semantics and permanent multi-version selector/shadow/rollback/drain machinery for a single-scheduler SQLite product. Its chosen middle path kept real durability and fencing while narrowing unsupported topology/upgrade promises.
- [PR #532](https://github.com/scottopell/phoenix-ide/pull/532), merged July 19, implemented that smaller shared model and wake delivery. It also enrolled background Bash/tmux handles automatically.

### Mechanically idempotent, semantically wrong

- [PR #555](https://github.com/scottopell/phoenix-ide/pull/555), merged July 20, removed automatic enrollment. Task 44008 records a bounded production sample in which 43 of 44 recent fired wake deliveries had an earlier terminal foreground result for the same handle. The model had already used the result and completed; wake then delivered it again and started a surprise turn. This is historical recorded evidence, not a fresh independent production measurement.
- This was not duplicated canonical rows. Row-level idempotence succeeded while two different product paths delivered the same meaning.
- [PR #557](https://github.com/scottopell/phoenix-ide/pull/557) reintroduced producers during WorkScope refactoring. [PR #603](https://github.com/scottopell/phoenix-ide/pull/603), merged July 29, removed Bash's producer again. A reported 24-hour wake remained pending even after a normal Bash wait observed successful completion, because its worker was dormant.
- [PR #666](https://github.com/scottopell/phoenix-ide/pull/666), merged August 10, removed tmux's producer again. Recorded pending wake obligations blocked cleanup after commands had been cancelled.
- Existing historical binding counts prove infrastructure enrollment, not intentional use of a successful explicit agent-facing wait tool.

### The explicit replacement hit its complexity stop gate

- [PR #621](https://github.com/scottopell/phoenix-ide/pull/621), closed unmerged, defined a bounded wake authority foundation: 6,198 additions, 13 files, 30 commits, and explicitly numbered review requests through at least round twenty. The reported 52 submitted review objects are not 52 independent review rounds.
- [PR #634](https://github.com/scottopell/phoenix-ide/pull/634), closed unmerged, investigated its first explicit Bash adapter. No adapter code was added. The foundation had no production observation/delivery/admission executor, so shipping one Bash journey would require a coordinated replacement across tools, DB, workflow execution, and runtime admission, risking a second authority.
- PR #621's closing comment explicitly rejects merging the wake-specific aggregate and points instead to existing generic durability, direct-turn materialization, and runtime admission without a compatibility bridge.
- An [August 9 deletion-first design snapshot](https://github.com/scottopell/phoenix-ide/blob/7b21a7f608f694b12649bc8f62cf9456d0eb41e0/tasks/44010-p1-brainstorming--deletion-first-durable-result-subscripti.md) attempted to shrink result subscriptions and respect ProductConversation ownership. This is unmerged planning evidence, not shipped authority or a required successor.

### Actual retirement

- [PR #894](https://github.com/scottopell/phoenix-ide/pull/894), merged October 12 at b6e0ecb744cce1b2dad3b767182d53002768d5b1, removed operational wake execution. The complete diff removes 17,138 lines and adds 1,731 across 70 files; these counts include tests/specs and are not production-code-only measurements.
- ADR-098 explicitly records removal, not a pause or promise of redesign. Current `specs/wake-contracts/requirements.md` governs retirement/history and prohibits execution from historical wake state. REQ-WAKE-001 through -018 are reserved retired identifiers.
- Removal covers registration/injection, worker startup/supervision, observations, delivery/adoption/resumption, APIs/UI/client commands, continuation transfer and wake-only Close/lifecycle coupling.
- Additive migration 126 preserves historical payloads, statuses, messages and links while marking workflow/occurrence/coordinator-event execution authority retired. It does not fabricate cancellation or revive old obligations. Already-admitted DirectTurns retain normal execution/recovery/Stop authority.
- Even removal required fixes for prompt leakage of unadopted historical observations, owned workflow deletion, and pending wake-sourced coordinator events admitting new work. Historical data and live authority had become entwined across multiple boundaries.

## What survives and is relevant

- Shared durable Direct Turn acceptance, identity, materialization, ownership, recovery and terminal settlement remain active production consumers. They were deliberately preserved by wake removal.
- Current `coordinator_watch_delivery::deliver_pass` demonstrates a factual source event admitted through `SendChatApplicationService` using a stable event/message identity and trusted origin. Discovery runs from the existing Direct Turn worker. This is a useful existing pattern, not permission to overload coordinator watch tables for reminders.
- ADR-074 uses the existing active Direct Turn as the restart baton and rejects another queue/scheduler/baton table for an already-owned turn.
- Current durable-workflow executive distinguishes active production primitives from generic observation/schedule/lease wrappers that are verification contracts without production producers. An available type such as `EffectDecl::next_eligible_at` does not prove there is a production executor ready to run reminders.
- Current SendChat admission can enqueue busy inputs as steering. This existing queueing behavior fits the user's clarified requirement; reminders need no Idle check, reminder-specific readiness policy, or admission-mode extension.

## Revised design direction to investigate

Start from one visible user reminder and one normal admitted input, not an abstract wake/result-subscription model:

1. Persist reminder identity, stable originating ProductConversation, text, due instant, display timezone, trusted source/create identity and cancellation/reschedule revision. Use normalized queryable columns, INTEGER Unix-microsecond timestamps and existing replay conventions.
2. Discover due reminders inside the existing server scheduling/discovery authority. A deadline timer/kick may reduce latency; persisted rows and startup discovery recover missed time. Do not assume an unimplemented generic schedule executor exists.
3. Record the due fact so a reminder card/list indicator can appear independently of model success and without an open client.
4. When due, submit one ordinary input with a deterministic reminder-occurrence key through standard durable Direct Turn/input admission. Let that path choose direct delivery or queueing and determine when the conversation can consume the alert. Derive acceptance/retry state from that existing owner; do not invent a reminder runtime owner, acceptance outbox, replay controller, readiness gate or transfer state machine.
5. Preserve visible reminder history when agent execution fails or is stopped. A stopped already-admitted follow-up is not a reason to recreate the input.
6. Consume current ProductConversation continuation and Close admission. Resolve pending reminder policy at that boundary; avoid making a reminder add a new settlement prerequisite or block ordinary Close by default.

The reminder-specific implementation boundary is durable time eligibility and idempotent submission into existing input admission. Verify cancellation/rescheduling races and the crash gap between due recording and standard input acceptance. Queueing, readiness and post-acceptance execution retain their ordinary owners and semantics.

## Agent-facing proposal retained

One small `reminders` surface with create/list/reschedule/cancel. Prefer a tagged time union: relative seconds, or explicit local date/time plus IANA timezone. Phoenix resolves time, rejects nonexistent/ambiguous local times with actionable choices, and supplies trusted conversation and replay identities. Confirmation is based on a successful persisted receipt with exact time/zone, reminder ID and available notification channels.

Registration returns immediately and permits a normal confirmation/final response. No provider request or tool call remains open until due, and registration does not forcibly park the entire tool round. Finish the current turn normally; zero LLM turns are spent waiting.

At due time, the event remains identifiable as a scheduled reminder tied to the original request. It is ordinary task data, not a new human message, higher-priority instruction or grant to execute mentioned actions. The reminder card appears even if the model follow-up cannot run. Closed-app phone alerts remain deferred.

## First proof and complexity stop gates

Prove one reminder for the concrete Safari/screenshots request before expanding API/UI/framework scope:

- Save and confirm an exact Eastern time, end the turn, observe no intervening model calls.
- With every client disconnected, record the reminder due and then admit one normal follow-up.
- Restart before due and between due recording and input acceptance: one visible occurrence and one admitted input identity, with no revived legacy wake records.
- Fail model startup: the reminder stays visible. Stop an accepted follow-up: no repeated automatic resurrection.
- Verify a due alert uses ordinary admission while the conversation is busy or awaiting the user, including standard queueing and eventual consumption; no reminder-specific Idle gate.
- Race cancel/reschedule, user input, continuation and Close against due submission, using existing transaction/lifecycle authority.
- Test replay after a lost create/admission acknowledgement and relevant timezone/DST boundary behavior.

Stop and re-ground if this single journey requires a new wake/profile aggregate, scheduler platform, worker lease protocol, delivery owner, acceptance outbox, continuation transfer protocol, Close settlement participant, provider-specific delayed tool result, or compatibility bridge to historical wakes. Do not add a special busy/Idle admission mode: ordinary input queueing/readiness is the selected behavior. Historical broad framework green tests do not qualify the reminder journey.

## Next design decision

Review the actual first-slice footprint on current main before committing to a profile or separate service. Inherit standard input admission, queueing, readiness and post-acceptance Stop behavior. Consume existing Close/continuation admission without inventing a new execution policy. Keep one-shot reminders, direct display and agent follow-up as the scope. Recurrence, generic scheduled commands, handle watches, Promise-like tools and phone push are separate commissions.

Implementation uses fresh reminder requirements and ADR-217 explaining the bounded difference from ADR-098; retired identifiers and old wake records remain inert. The small Allium model covers reminder time eligibility without restating Direct Turn internals.

## Implementation progress

Storage slice: normalized reminder schema migration151, validated core reminder values, replay-safe creation, revision-checked management, immutable due commitment, bounded/keyset discovery and existing Direct Turn/steering receipt lookup are implemented. Focused validation passes 11 DB tests and one core value test, including a real SQLite cancellation/due race, file-backed reopen, persistent steering receipts, owned FK cleanup and backward-clock cancellation. Reminder Allium analysis has zero diagnostics/findings with its governing import; formatting, whitespace and task metadata validate. A filtered dev.py spec run passed artifact shape but was stopped after its nested unit suite spawned an unplanned full build; its earlier four Allium surface findings have been corrected. No complete project-check pass is claimed. Tool/runtime/provenance and UI/notification slices follow serially; no full user journey, production deployment or review-clean claim is made yet.

API journey slice: the real-binary isolated `tests/e2e/run.py --scenario reminders` test passes, including native committed receipt/source identity, upcoming record, process restart before due, HTTP reschedule and revision conflict, due scheduled origin and ordinary follow-up, SSE reconnect and restart after acceptance without duplicate input, wrong-owner rejection and cancellation. All 18 mock-provider tests, 46 harness self-tests, seven panel tests, reminder CSS lint and production frontend build pass. The opt-in mock recognizes rendered scheduled-reminder provenance before scenario-marker/hash selection and emits a deterministic text follow-up without another creation.

Chrome QA observed the upcoming card and opened reschedule form. A captured desktop screenshot prompted bounded theme styling for reminder buttons and edit inputs. Later CLI capture/fill/resize operations were inconclusive, so the parent completed browser QA through the in-app browser on the transferred isolated server: real reschedule to America/New_York, 393×852 layout (panel x=16,width=361,document width=393), cancellation, immutable due card, scheduled input label and ordinary model follow-up all passed. Native date-field keyboard editing worked. Notification delivery permission/device acceptance, main integration, broad checks and independent review remain open. No production deployment is claimed.

Main integration retains Federation's migration 150 and appends reminders as 151.
The worker preserves watch delivery, Federation sync, reminder discovery and
deadline-based waits. Reminder DB and application filters pass 16 and 14 tests;
the three Federation migration regressions and exact-ledger assertions pass.
The populated 151 regression preserves every original table/column, including
provenance, Federation records and inert wake history, with clean foreign keys.
Final project checks and the integrated mock journey are assigned to parent QA.

Native integration adds scheduled-reminder decoding/encoding and a non-human
label without cross-source links. Snapshot version 3 migrates exact v1/v2
envelopes and fences older writers. Xcode build-for-testing passes for the full
app and unit-test target, including origin and cache regression sources. Test
execution is unqualified because host pseudo-terminal exhaustion blocked the
simulator runner; hosted iOS CI must execute these cases. Migration-fixture
batches pass 9 and 8 tests after the merge. No deployment or phone alerts added.
