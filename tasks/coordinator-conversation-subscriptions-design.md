# Design explicit conversation subscriptions for the Coordinator

## Goal

Collaboratively design how the Global Coordinator can subscribe to selected ProductConversations, receive durable notifications when relevant execution ends or errors occur, inspect the evidence, and optionally send guidance to re-drive unfinished work.

This is an architecture exploration and teaching task, not approval to implement the feature. Walk the user through Phoenix internals as they become relevant, explain interesting trade-offs, and ask focused product questions along the way. Prefer short incremental explanations over one large final architecture document.

## Intended user journey

The user authorizes the Coordinator to watch selected conversations. A relevant committed event makes a notification owed to the Coordinator. Existing runtime admission delivers that notification safely, including when the Coordinator is busy. The Coordinator reads current state and relevant transcript evidence before deciding whether to send guidance through the existing cross-conversation messaging service. A notification does not itself authorize blindly restarting its subject.

## Starting recommendation

Use a narrow typed set of conversation events and reuse existing durable direct-turn admission, conversation lifecycle authority, and cross-conversation messaging. Do not begin with a generic event bus, arbitrary predicates, a new scheduler, or another runtime-admission authority. Validate reuse against actual code rather than assuming an existing API already supplies all needed semantics.

Separate facts from interpretation: a completed turn does not imply completed work; an idle state does not imply abandonment. Preserve cancellation, waiting for user input, continuation, error recovery, and Close/History distinctions.

## Investigation and guided design sequence

1. Recheck the delivery roadmap, current checkout, relevant specifications, and implementation. Read VISION.md. Clearly distinguish current behavior, normative requirements, proposals, and deployment evidence.
2. Teach the identity model: ProductConversation versus transcript, continuation, WorkScope, and accepted turn identity. Show which identity a subscription follows and which identity identifies an event occurrence.
3. Trace one existing user message from chat acceptance through durable direct-turn admission, runtime execution, terminal persistence, and post-commit publication. Explain where busy serialization and restart recovery actually live.
4. Trace the Coordinator's current snapshot, read tools, and send_conversation_message path. Explain delivered versus queued versus rejected outcomes and why acceptance does not mean recipient understanding.
5. Compare the smallest event-production seams: persisted turn terminal outcomes and committed conversation state transitions. Identify coverage gaps, including errors outside an ordinary accepted turn, continuation, cancellation, and recovery. Do not treat SSE delivery or transient in-memory notifications as durable event truth.
6. Design the minimum subscription and notification persistence. Identify one authority for enrollment, source occurrence identity, owed delivery, and accepted execution. Show the transaction/crash boundary that prevents lost or duplicate semantic notifications. Use existing terminal truth rather than creating a competing result store.
7. Walk through busy Coordinator, duplicate delivery, restart, already-resumed target, subscription cancellation, continuation, and Close races. Keep source lifecycle separate from recipient admission.
8. Discuss feedback-loop and cost controls: no blind re-drive, explicit cancellation respected, current-state recheck, and bounded intervention. Ask the user to choose policy where preference matters.
9. Produce a concise agreed architecture, interaction diagram, settled decisions, remaining code-verification work, and one smallest implementation slice with testable acceptance criteria. Propose implementation separately; do not silently expand this task into building it.

## Questions to present incrementally

Present concrete options with a recommendation and explain the consequence of each. Do not ask the user to decide facts that code inspection can settle.

- Is a subscription one-shot for the current/next turn, or persistent until explicitly removed?
- Which events should notify: completed turns, terminal errors, explicit cancellations, or waiting-for-user states? Which merely provide context?
- What is the authorization boundary for enrollment and proactive Coordinator messages?
- May the Coordinator re-drive automatically within explicit limits, or should some interventions require user confirmation?
- What should happen to accumulated notifications while the Coordinator is busy, and what audit/status visibility is actually useful?
- What stops repeated intervention when a target continues to end without satisfying its assignment?

## Evidence anchors to verify

- crates/phoenix-workflow/src/direct_turn.rs: TurnTerminal and turn lifecycle.
- crates/phoenix-db/src/workflow/direct_turn.rs: accept_authoritative_turn, terminalization, replay, and materialization.
- crates/phoenix-ide/src/runtime/executor.rs: persist_state_effect and terminal settlement.
- crates/phoenix-ide/src/send_chat_service.rs: ordinary chat acceptance and steering.
- crates/phoenix-ide/src/coordinator_tools.rs: send_conversation_message and Coordinator tool boundaries.
- ProductConversation lifecycle/Close specifications and their current admission implementation.
- specs/global-recall/requirements.md: REQ-GR-007 currently excludes autonomous background behavior; explicit subscription-driven execution needs a deliberate requirements change and decision record before implementation.

## Acceptance for this design task

- The user has been walked through the relevant existing architecture and its authority boundaries.
- Proposed behavior is distinguished from current capability, with concrete code/spec evidence.
- Product choices are resolved through conversation rather than hidden assumptions.
- The proposed design identifies enrollment, occurrence identity, durable delivery, runtime admission, lifecycle suppression, and intervention policy without overlapping authorities.
- A bounded implementation plan lists producer/consumer seams, required specification changes, and tests for idle/busy delivery, duplicate/restart handling, continuation, cancellation, Close races, stale notifications, and feedback-loop prevention.
- No production mutation or feature implementation occurs as part of this design approval.

## Non-goals

General event infrastructure, arbitrary event filters, external-process subscriptions, universal exactly-once execution claims, redesigning direct-turn execution, or background monitoring of every conversation by default.

*Historical footnote: earlier wake-contract work contains lessons about duplicate semantic delivery and overlapping admission authorities. It is not the proposed API, implementation program, or a prerequisite for this feature.*
