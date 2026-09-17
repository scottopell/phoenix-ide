# Add opt-in durable automatic context continuation

## Commission and base

Implement from refreshed live `origin/main`, with the commissioned investigation anchored at `a56928576440ccfb39344d8e16964aaf4c6a2d8e`. This task replaces its former UI-only auto-navigation proposal: automatic continuation crosses consent, authority, persistence, recovery, and aggregate-identity boundaries and cannot safely be implemented by a React `useEffect` plus two API calls.

This is Phoenix-only user-authorized work. Do not merge, deploy, continue any production conversation, edit lifecycle rows manually, or perform destructive repair. Until the coordinator releases devmbp’s sole heavy slot, perform only source/spec analysis and lightweight spec work; do not run Cargo/rustc, `./dev.py check`, Playwright, Vitest, or `xcodebuild`.

The GitHub #651 roadmap was unavailable in network-blocked Explore mode. Before implementation, use `phoenix-development` to refresh main and inspect the generated roadmap plus #764/#765 current state. Specs/Allium, ADRs, task filenames, merged main, and explicit dependency ownership remain the domain authorities.

## Observed journey

- An idle parent can manually request a continuation summary. At the context threshold Phoenix triggers the same summary flow automatically, rejects summary-response tools, freezes that transcript member’s prompt projection, and atomically commits one exact generated handoff plus `ContextExhausted` state.
- Today a context-exhausted latest row waits for user review: Continue unchanged, edit-first Continue, or Copy. Continue calls the existing durable continuation endpoint with selected text and a client message id.
- The existing operation atomically creates one linked successor and opening dispatch intent, transfers aggregate/environment ownership, materializes the runtime, and routes the opening text through `SendChatApplicationService`; durable message insertion settles the intent. Concurrent calls converge on the topology winner.
- Add an explicit per-user-visible-conversation preference, default OFF: “Always accept generated handoffs and continue.” It applies to both ordinary ProductConversations and the single stable Global Coordinator conversation identity. OFF preserves the journey above exactly. ON is sampled only as the owning stable aggregate enters `ContextExhausted`; a successful sample durably admits automatic continuation.

## Verified current-code semantics

### Normative behavior

- `specs/bedrock/requirements.md` REQ-BED-019 through REQ-BED-023 and `specs/bedrock/bedrock.allium` own threshold detection, identified summary generation/recovery, immutable context-exhausted handoff review, and manual successor creation.
- REQ-BED-021 requires successor creation and its opening intent to commit atomically; retries reuse original text/message identity, avoid a second send path, and consume the intent only when acceptance is durable elsewhere.
- `UserStartsContinuationConversation` requires a context-exhausted latest row, no successor, an Open ordinary ProductConversation or Coordinator, and no active Close obligation. It preserves stable ProductConversation identity, mode, working directory, and exactly one topology successor.
- ADR-025 establishes summary compaction as an idempotent durable operation, with at-least-once provider calls but exactly-once Phoenix summary commit and handoff. ADR-026, ADR-031, and ADR-046 establish ProductConversation as stable aggregate identity while transcript rows retain runtime/message authority.

### Implementation path

- `phoenix-state-machine::transition` emits `Effect::ContinuationCommit`; `ConversationExecutor` creates deterministic message id `continuation-{conversation_id}-{operation_id}` and calls `commit_continuation` or direct-turn settlement. Applied/duplicate/stale outcomes reconcile summary persistence and state publication.
- `Database::continue_conversation_with_intent` / `continue_conversation_inner` owns successor reservation. It checks `ContextExhausted`, rejects sub-agents, reserves the predecessor edge, creates one Idle successor, sets `continued_in_conv_id`, and inserts the opening intent in one transaction. The guarded predecessor update makes concurrent calls return `AlreadyContinued`.
- The successor keeps `product_conversation_id`, model/effort/service tier/language, cwd, legacy mode/task identity, and mapped attached WorkScope (Direct intentionally receives a new Direct scope). Existing continuation transfer/reconciliation moves wake bindings.
- `dispatch_continuation_handoff` sends `LiteralText` through `SendChatApplicationService`. Delivered/queued outcomes leave the intent until durable message insertion; `AlreadyPersisted` cleans stale intent; rejection returns `dispatch_failed`. Pending continuation intents currently lack an autonomous recovery owner.
- Pending steering is normalized under `steering_messages` and keyed to transcript conversation id. Current successor creation does not move it, so queued steering on the predecessor does not follow today. Deferred wake/runtime work likewise needs explicit transfer through existing typed seams.
- Ordinary ProductConversations and the Global Coordinator each have stable aggregate/identity authority across transcript successors. Those existing stable authorities—not historical `conversations` rows and not a global application setting—are the nonredundant persistence owners for their respective preference.
- `ProductConversationSnapshotView` is the canonical ordinary-aggregate projection consumed by `ProductConversationPage`, which embeds `ConversationPage` for the latest row. The Coordinator’s existing stable aggregate projection/routing must expose the same contract through its own canonical surface. `ContextExhaustedHandoff` owns unchanged/edit/copy controls and browser-local drafts.
- Today the generated handoff becomes an ordinary user message after manual acceptance. Automatic acceptance needs typed provenance/prompt treatment so exact generated text is predecessor context, not newly asserted user authority, without storing the text twice.

## Dependency and portfolio seams

- #764 owns ADR-052 and migrations 097–098. #765 owns ADR-053 and subsequent capability migrations. This task must not reserve, create, rename, or duplicate any of those ledger slots.
- Read-only analysis and edits to non-conflicting normative text can proceed while those workstreams are active. Before any migration or ADR is added, refresh live main and the ADR/migration ledgers after dependencies land, inspect #764/#765 changes, and allocate only the next genuinely free slots then. This task intentionally names no future migration or ADR number.
- Reconcile likely conflicts in `specs/adrs/README.md`, `specs/bedrock/*`, migration registration/schema code, ProductConversation schema/projections, direct-turn/continuation persistence, and generated API types. Preserve the dependency work’s invariants and extend its canonical types/tables rather than introducing parallel representations.
- If #764/#765 alter continuation foundation, capability persistence, migration ordering, or aggregate projections, re-ground this plan against merged code before implementation. A semantic conflict is resolved in specs/ADR and shared types—not by retaining both implementations.

## Owning invariants

1. **Explicit aggregate consent:** only an explicit supported API mutation enables one user-visible conversation: an ordinary ProductConversation or the Global Coordinator. Absence and every migrated aggregate mean OFF. No global application default, inferred consent, or mass activation.
2. **Stable single authority:** store the preference once on the existing stable ordinary ProductConversation identity or the existing single Global Coordinator aggregate identity, and project it from every transcript successor. Never copy it onto historical transcript rows or introduce a second Coordinator/global-settings authority.
3. **Prospective admission only:** ON must be observed in the atomic transition/admission associated with reaching `ContextExhausted`. Enabling an already-exhausted current or historical row MUST NOT create, schedule, claim, dispatch, or wake a successor. It affects only a future exhaustion after the user manually continues within the same stable ordinary or Coordinator aggregate.
4. **Durably admitted recovery is distinct from retroactivity:** once the setting was ON at exhaustion and the automatic operation was durably admitted, disabling later does not revoke it; retries/restart recovery may complete that exact admitted operation. Recovery can never manufacture admission from an exhausted row that lacks the durable admission fact.
5. **OFF parity:** OFF ends in today’s manual `ContextExhausted` surface with unchanged edit/copy/Continue behavior, API semantics, close eligibility, and no automatic effects.
6. **One shared continuation operation:** automatic work invokes the existing manual successor reservation, dispatch, settlement, WorkScope/wake transfer, and idempotent send path. No second successor constructor, sender, state machine, or parallel workflow.
7. **Exact non-authoritative context:** dispatch exactly the committed summary bytes—no trim, interpolation, draft, regeneration, or replacement—with typed origin/projection semantics that make it untrusted predecessor context rather than a new user instruction. Manual unchanged/edited acceptance retains current user-authorized semantics.
8. **Successor inheritance:** one linked successor remains in the same ordinary ProductConversation or Global Coordinator aggregate and inherits execution mode, WorkScope/environment, queued steering, and eligible deferred durable work. Transfer normalized children/typed owners atomically or through existing durable transfer protocols; never strand or duplicate them.
9. **Exactly one winner:** concurrent auto attempts, manual Continue, retries, and restarts converge on one predecessor edge, one persisted opening identity, one accepted opening handoff, and one set of transfer effects. Manual-vs-auto races expose the durable winner and never claim the losing text was accepted.
10. **Durable bounded liveness:** crashes around admission/reservation/dispatch/settlement recover from persisted facts. Retry only an already-admitted operation while progress remains owed; bound repeated no-progress attempts, stop without a hot loop, persist an actionable failure, and provide safe explicit recovery without changing the accepted payload.
11. **Eligibility and lifecycle fences:** ordinary ProductConversations and the Global Coordinator are eligible. Sub-agents and all other ineligible modes, History, active Close obligations, nonlatest rows, and non-context-exhausted rows cannot auto-continue. Preference mutation alone never changes lifecycle or wakes work.

## Normative specification and decision work

Before code, follow `specs/AUTHORING.md` and leave timeless artifacts free of issue/task/status language:

- Extend `specs/bedrock/requirements.md` with named requirements applying the same aggregate opt-in/default OFF, prospective exhaustion-time admission, non-retroactive enablement, admitted-operation recovery, exact non-authoritative context, reuse of manual durable continuation, inheritance, exactly-once convergence, and bounded actionable failure to ordinary ProductConversations and the Global Coordinator.
- Extend `specs/bedrock/bedrock.allium` entities, surfaces, continuation rules, crash/retry rules, and invariants. Model preference mutation, exhaustion-time admission, post-exhaustion enable as no-op for that exhaustion, disable/manual races, durable phases, ownership transfer, circuit-open failure, and explicit retry. Leave no open questions.
- Update `specs/api/requirements.md` and `specs/conversation-ui/requirements.md` where supported aggregate preference read/write and explicit controls are owned; update `specs/bedrock/executive.md` only after implementation with current reality and verification coverage.
- After refreshing merged dependency work, add an ADR only if the resulting decision is not already owned by #764/#765 or an existing ADR. If required, allocate the next free live-main number then and record aggregate ownership, non-authoritative generated context, the admission linearization point, shared operation reuse, and durable breaker design. Do not rewrite historical ADRs or pre-claim a number.

## Implementation plan

1. **Persistence and types**
   - After refreshing dependencies, allocate the next free migration slot(s). Add schema-constrained preferences, defaulting false, to the existing stable ordinary ProductConversation and Global Coordinator aggregate authorities; migration changes no aggregate to ON and creates no application-global setting.
   - Persist the minimum normalized automatic-admission/failure facts needed to distinguish: never admitted, admitted while ON, in progress, settled, and breaker-open. Do not infer admission by scanning `ContextExhausted`, put child records in JSON, copy preference to transcript rows, or duplicate handoff text.
   - Add typed preference, handoff origin, phase/outcome, and breaker states. Make exhaustion commit/admission atomic where possible; otherwise use an existing transactionally coupled durable effect whose existence is committed with exhaustion and whose recovery cannot be synthesized afterward.
   - Extend the existing dispatch intent only as needed for automatic provenance and no-progress accounting while preserving manual intent semantics.

2. **Shared application operation and recovery**
   - Extract current handler orchestration into one application service for manual endpoint and automatic reconciler: existing reservation → existing typed ownership transfer → runtime materialization → existing `SendChatApplicationService` dispatch → trigger-based settlement.
   - Kick automatic work only after the durable admission exists. Startup/steady reconciliation lists only admitted unsettled operations; preference enablement never scans/admits already-exhausted rows.
   - Count completed no-progress attempts, use bounded backoff, and persist circuit-open actionable failure. Proof of prior progress/settlement reconciles without incrementing or redispatching.
   - Reuse topology uniqueness, deterministic opening id, steering acceptance receipts, wake transfer reconciliation, and intent-consumption trigger for idempotence.

3. **Authority and inheritance**
   - Preserve committed summary bytes in one accepted opening payload while carrying typed automatic-generated-context origin into prompt projection. Provider rendering must delimit it as predecessor context unable to confer instructions or authorization.
   - Move/rebind predecessor `steering_messages`, attachments, and receipts as one ordered set without changing ids/FIFO order. Transfer continuation-eligible deferred workflow/wake ownership through current typed APIs and crash reconciliation. Preserve mode/task identity and ordinary ProductConversation- or Coordinator-attached WorkScope through existing continuation code.

4. **Supported API and UI**
   - Add aggregate-addressed, idempotent explicit-boolean preference API support for both ordinary ProductConversation and Global Coordinator stable identities. Include preference and any admitted automatic operation’s progress/actionable failure in each canonical aggregate projection; do not route Coordinator consent through application-global settings.
   - Add generated TS types and API methods; regenerate files rather than hand-editing.
   - Add the same simple explicit checkbox/switch on the live ordinary ProductConversation and Global Coordinator surfaces: “Always accept generated handoffs and continue.” Default visually OFF, persist immediately, and show save/error feedback. Enabling while already exhausted must clearly apply to the next exhaustion only and must leave manual controls untouched.
   - During a durably admitted automatic operation show compact progress. On breaker-open failure show actionable reason and safe retry/open-successor/manual fallback consistent with the single durable winner. Render one historical continuation boundary after success.

## Verification matrix

### Persistence/API and prospective consent

- Fresh and migrated ordinary ProductConversations and the Global Coordinator read OFF; no migration mass-activates either kind.
- Preference is scoped to one stable user-visible conversation identity, idempotent, survives restart/successors, and cannot leak between ordinary aggregates or between an ordinary aggregate and Coordinator.
- ON before the exhaustion admission creates one durable automatic obligation. OFF at admission creates none.
- Enabling any already-exhausted latest or historical row creates/wakes nothing, including after restart or reconciler scans. After manual continuation, ON applies when that same stable ordinary ProductConversation or Global Coordinator reaches a later exhaustion.
- Disabling before exhaustion/admission yields manual flow. Disabling after durable admission does not revoke that exact operation; recovery still converges.

### OFF parity and manual compatibility

- OFF threshold summary commit produces only today’s exact continuation message and `ContextExhausted`; no auto obligation, successor, dispatch, queue move, or new effect.
- Existing manual unchanged/edit/copy flow, endpoint responses, and double-Continue topology race remain unchanged.

### ON, authority, and inheritance

- ON has no effect during ordinary turns or before `ContextExhausted` commits.
- Generated text with leading/trailing whitespace and instruction-like content becomes exactly one typed predecessor-context handoff, not user authority; one successor starts without user action.
- Ordinary and Coordinator successors each retain their respective stable aggregate identity, mode, model/effort/service tier/language, WorkScope/environment, queued steering FIFO/attachments/receipts, and eligible deferred wake/work ownership.

### Races and exactly-once effects

- Exercise ON→OFF versus exhaustion admission in both transaction orderings.
- Exercise manual Continue versus admitted automatic continuation in both orderings: one successor and one winning text; loser observes a conflict/existing successor without false acceptance.
- Concurrent reconcilers, repeated kicks, API retries, reconnects, and preference writes yield one reservation, one ownership transfer, one runtime obligation, one opening acceptance, and one settlement.

### Crash boundaries and breaker

Inject/reconstruct crashes: summary/exhaustion before admission commit (if not atomic), admitted obligation before claim, claim before successor reservation, successor/intent commit before ownership transfer, before runtime, before send, after delivery before message persistence, and after persistence before cleanup/publication. Every restart either remains manual because no admission exists or completes exactly one admitted operation.

- Transient failures retry original persisted bytes/id with bounded backoff.
- Repeated no-progress reaches the configured cap, stops autonomous retry/hot-looping, exposes durable actionable failure, and supports safe explicit recovery.
- Duplicate/already-persisted/progress reconciliation does not increment no-progress or redispatch after settlement.

## Test homes and validation timing

- DB/migration/transaction/race tests: migration registry plus continuation and wake persistence modules selected after #764/#765 merge.
- Runtime/application/crash tests: `ConversationExecutor`, shared continuation service/reconciler, runtime test storage/traits, and ordinary ProductConversation plus Global Coordinator API/handler tests.
- Pure transition/property tests in `phoenix-state-machine` if it owns the new admission transition.
- UI/API tests in `ui/src/api.test.ts`, `ContextExhaustedHandoff.test.tsx`, and ordinary ProductConversation, Global Coordinator, and Conversation page tests/fixtures.
- Lightweight spec/Allium checks may run before heavy-slot release. Only afterward run focused Rust/UI tests, codegen, and `./dev.py check`; report anything unavailable rather than claiming it ran.

## Explicit non-goals

Do not auto-continue sub-agents or other ineligible modes; exclude the Global Coordinator; create an application-global setting; globally enable; retroactively admit exhausted rows; infer consent from manual Continue; copy flags to historical rows; duplicate handoff text; flatten transcript history into the successor prompt; add a second continuation workflow; reserve dependency-owned ADR/migration slots; redesign generic creation; merge/deploy; or broaden compatibility guarantees without normative policy and an allocated live-ledger ADR.
