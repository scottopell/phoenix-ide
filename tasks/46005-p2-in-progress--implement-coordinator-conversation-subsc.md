# Implement explicit Coordinator conversation subscriptions and input provenance

## Objective

Deliver trusted Global Coordinator subscriptions to selected ordinary ProductConversations. Deliver factual notifications when watched execution ends normally, fails, or is explicitly cancelled. Reuse existing durable input admission and steering rather than building another execution scheduler. First deliver shared input provenance so internal agent messages are distinguishable from user-facing API input.

This is an implementation handoff to the Phoenix Coordinator. Coordinate ownership and dependencies, then execute the two bounded slices below. The approved product decisions are authoritative for this task; do not reopen them as generic discovery. See tasks/46004-p2-done--coordinator-conversation-subscriptions-d.md for investigation context.

## Settled product contract

- Only the Global Coordinator receives subscription-management tools. It is explicitly trusted to choose additional conversations to watch without per-target user permission.
- Watch ordinary ProductConversation identity across transcript continuation, not WorkScope or a single transcript. Do not allow watching the Coordinator itself or subagents.
- Watches persist until explicitly removed or the watched conversation closes. Repeated subscribe is idempotent.
- Future occurrences only: no catch-up, initial synthetic notification, or historical replay. Subscribe returns current state from a consistent transaction boundary. Re-enrollment never revives old pending events.
- Fixed initial event set: normal execution ending, execution failure, explicit cancellation. Include factual cause/category and cancellation initiator when supported by authoritative evidence. Successful continuation handoff is not a stop alert. Waiting for a user answer or approval does not notify initially.
- Notification packets contain facts, identity, time, and evidence references only. Natural-language derived facts are allowed. Do not append behavioral instructions, recovery recommendations, or prompt guidance. The Coordinator infers appropriate action from facts and its existing context.
- Notifications may enter ongoing Coordinator execution through existing safe steering boundaries; do not require a separate notification-only turn.
- Stop cancels the Coordinator's current execution without pausing subscriptions or future notifications. The same already-accepted notification must not be automatically redelivered because execution was cancelled.
- Unsubscribe/Close suppresses notifications not yet accepted into Coordinator input. Already-accepted input is not retracted from queues or transcript. Removal and acceptance must serialize at a durable boundary.
- No new retry-budget system, persistent pause mode, or subscription-specific intervention policy.

## Start with current authority and coordination

Read Issue #651, VISION.md, the relevant specifications, and the current implementation before editing. Coordinate with the automatic-context-continuation and lifecycle/admission owners. Earlier exploration's roadmap cited automatic-continuation owner @conv:583071fc and commit 6e81f0324, but neither was available through local reference/commit lookup; resolve current full identity and branch rather than assuming these are still current.

Determine the definitive automatic-continuation admission/settlement outcome. Exclude successful handoff using that authority, never timers or polling to infer whether execution resumed. Follow the dependency order of active lifecycle changes; do not duplicate their continuation or Close authority.

Update normative requirements and record design decisions before implementing behavior that changes existing contracts. In particular, specs/global-recall/requirements.md REQ-GR-007 currently excludes autonomous background behavior and must explicitly permit trusted subscription-driven Coordinator execution. Follow the spec authoring preflight. Keep task status and rollout notes out of normative documents.

## Slice 1 — trusted input provenance

Make user-facing API input, internal conversation-sent input, and system-delivered subscription events structurally distinguishable. Preserve existing system-generated input behavior.

- Assign provenance at trusted server boundaries, not from an arbitrary model-supplied origin parameter.
- User means the user-facing API channel; do not claim to prove a biological human authored external API calls.
- send_conversation_message assigns sender ProductConversation/transcript identity from ToolContext and authoritative membership. Its current SendChatRequest drops sender identity.
- Subscription provenance references the durable notification occurrence. Distinguish packet origin from the actor who caused cancellation.
- Trace UserContent::is_meta and its consumers before selecting representation. Avoid two independently writable representations of the same origin fact. Do not broaden this into a taxonomy of all internal actions.
- Thread provenance through direct input, steering, exact replay identity, persistence, SSE/codegen, transcript/history retrieval, UI rendering, and model input. Provider user-role encoding does not make an agent message user-authored.
- Keep presentation lightweight: inline source attribution and source/evidence navigation using existing UI conventions.
- Historical messages without reliable origin evidence must not be fabricated as human-authored. Specify the migration/unknown-history policy explicitly, consistent with compatibility requirements. Do not add speculative rollout bridges.
- Store queryable provenance in constrained schema fields/rows, not display_data or an untyped text prefix. Use types and schema constraints appropriate to origin variants.

Ship this as an independently useful, tested change before or as the first reviewable part of subscription implementation. Do not require subscription execution for cross-conversation attribution to work.

## Slice 2 — subscriptions and durable event delivery

Implement a small Coordinator tool surface for subscribing, unsubscribing, and inspecting current watches. Resolve targets through authoritative ProductConversation identity and existing target restrictions. Do not build a settings dashboard or event-filter language.

### Source event authority

Trace active execution settlement, continuation, cancellation, and failure paths. Starting point: classify_active_direct_turn_state_terminal and terminalize_authoritative_turn. Completed alone is insufficient: current terminal classification also includes handoff. Steering-entry consumption is not execution ending.

Define the exact event coverage and exhaustively map applicable source outcomes. Investigate errors outside active accepted turns; do not silently claim all failures are covered by one hook. Explicitly document any excluded lifecycle failures and produce bounded follow-up work if needed. Preserve cancellation cause from authoritative source facts rather than inferring it from an internal event name or text.

### Durable handoff

- Commit notification obligation with the authoritative source occurrence, so source completion cannot commit while notification intent is lost on crash.
- Use one stable occurrence identity for semantic deduplication and one unambiguous watch lifetime. Replayed settlement cannot create another notification; re-enrollment cannot revive an old one.
- Enrollment and its current-state read share a consistent database boundary: a relevant event is either reflected in returned current state or eligible as a subsequent event.
- Hand pending notifications to existing input admission using trusted provenance and stable replay identity. Reuse runtime serialization, direct-turn discovery, steering, and lifecycle checks.
- Recording that an input was accepted and relinquishing delivery ownership must be atomic or recoverable through exact idempotent replay, including across Coordinator continuation.
- Unsubscribe/Close versus acceptance uses commit order. No check-then-send gap. Once accepted, subscription delivery does not own execution or retries.
- Preserve source occurrence/transcript identity even if current target or Coordinator transcript has changed.
- Bounded processing is required; no debounce timers or guaranteed separate batching turn. Preserve each occurrence if existing input paths group delivery.
- Use existing authoritative terminal facts; do not create a competing result registry. A narrow pending handoff record is justified, a second execution framework is not.

## Starting code/spec anchors

- crates/phoenix-ide/src/coordinator_tools.rs: SendConversationMessage and Coordinator-only tool registration.
- crates/phoenix-ide/src/send_chat_service.rs: SendChatRequest, replay lookup, admission lock, steering versus direct acceptance.
- crates/phoenix-core/src/domain/sm_event.rs: PreparedDirectTurnPayload, PreparedDirectTurnDelivery, SteerEntry.
- crates/phoenix-core/src/domain/db_schema.rs: UserContent::is_meta, Message and MessageContent.
- crates/phoenix-core/src/domain/product_conversation.rs: ProductConversation identity and lifecycle.
- crates/phoenix-ide/src/runtime/executor.rs: classify_active_direct_turn_state_terminal, persist_state_effect, settle_pending_direct_turn.
- crates/phoenix-ide/src/runtime/traits.rs: settle_active_direct_turn.
- crates/phoenix-db/src/workflow/direct_turn.rs: acceptance, replay, terminalization with conversation projection.
- crates/phoenix-ide/src/runtime/direct_turn_worker.rs and runtime.rs: existing dispatch, discovery, steering, continuation.
- specs/global-recall/, specs/bedrock/, specs/compatibility/, and current ProductConversation lifecycle and auto-continuation specifications.

## Acceptance evidence

1. User API messages and cross-conversation agent messages retain distinct trusted provenance while idle and busy, after restart/reconnect, through history reads, and in model-bound context. The sending tool cannot spoof user origin.
2. Coordinator subscribes to a running ordinary ProductConversation; normal execution ending creates one factual notification and reaches Coordinator execution without user prompting.
3. Failure and cancellation produce correctly attributed facts without behavioral guidance. No unsupported claim that cancellation was human-initiated.
4. Coordinator already busy: notification uses existing safe input behavior without overlapping execution or loss.
5. Subscribe versus source settlement race has consistent ordering; pre-enrollment events do not replay. Duplicate subscribe is a no-op.
6. Successful continuation, including enabled automatic continuation, does not create a stop alert. Failed/no accepted continuation follows the agreed relevant-outcome mapping. Watching survives transcript changes.
7. Crash tests cover source settlement, notification recording, recipient acceptance, and delivery acknowledgement. Replay creates neither duplicate notification input nor duplicate accepted execution.
8. Unsubscribe and watched Close race with acceptance: unaccepted notifications suppress; accepted input remains. Re-enrollment never revives suppressed events.
9. Stop Coordinator: current execution cancels; later events can trigger execution; the cancelled input itself is not redelivered automatically.
10. Coordinator cannot subscribe to itself; ordinary conversations/subagents lack subscription-management capability. Source references and event payloads remain data, not executable instructions.
11. Historical provenance migration does not fabricate attribution; persisted representations and generated wire types are validated.
12. Focused Rust/UI tests, required codegen, relevant ./dev.py check lanes, spec preflight, and independent exact-head review pass. Exercise a real idle/busy cross-conversation journey using phoenix-client.py or equivalent harness and inspect actual model-bound attribution.

## Delivery and stop gates

Use separate reviewable commits/PRs for provenance and subscriptions where practical. Update executive docs to reflect verified coverage. Record unresolved implementation dependencies explicitly; do not claim completion based only on infrastructure unit tests.

Stop and report a bounded design adjustment if implementation requires a new generic event bus, scheduler, second runtime admission authority, arbitrary predicates, persistent Coordinator pause state, broad lifecycle refactor, or replacing existing direct-turn machinery. Do not revive historical framework work as a prerequisite.

Do not deploy or merge unrelated work as part of this task. No production data mutation for testing. Keep notifications factual; do not smuggle intervention-policy prompts into packet formatting or tool results.
