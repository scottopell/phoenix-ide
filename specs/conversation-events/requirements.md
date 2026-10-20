# Conversation event presentation

## User need

Readers need to distinguish a conversation's reported outcome from task completion, scan events without diagnostic identifiers dominating the transcript, and inspect original evidence and source provenance when needed.

## REQ-CEVT-001 — Durable typed projection

WHEN a transcript message originates from a durable subscription event, THE SYSTEM SHALL derive its presentation from the persisted event identity and typed outcome, not by parsing diagnostic prose. The lookup SHALL consider every requested identity, including historical events whose delivery or watch enrollment is no longer active. Presentation SHALL NOT mutate persisted event or provider evidence.

## REQ-CEVT-002 — Truthful compact summary

WHEN event metadata is available, THE SYSTEM SHALL display a linked conversation title (or an honest identity fallback when the title is absent), the recorded outcome and local occurrence time. A completed turn SHALL be labelled “Turn finished” and SHALL NOT imply task completion. Failure, cleanup failure, cancellation and awaiting-user outcomes SHALL remain distinguishable. WHEN metadata is missing or cannot be loaded, THE SYSTEM SHALL retain the message with an unavailable-details fallback rather than inventing its source title or outcome.

## REQ-CEVT-003 — Inspectable evidence

THE SYSTEM SHALL retain expandable exact event and source identifiers, occurrence and generation provenance, original diagnostic text, and copy controls. Expansion SHALL be keyboard-operable. Titles SHALL wrap on narrow viewports and use readable theme colors without inherited opacity reducing title contrast below 4.5:1. Find SHALL include both summary and hidden evidence and reveal matching collapsed details.

## REQ-CEVT-004 — Read-path and privacy consistency

THE SYSTEM SHALL retain the typed presentation through authenticated initial, live, accepted, historical, paginated and continued transcript reads, including pending message projections. Non-event messages SHALL carry an explicit null presentation. Shared reads SHALL use an explicit redacted presentation and SHALL NOT perform a private title lookup or add private provenance; this does not authorize alteration of diagnostic content already included by the share contract.

## REQ-CEVT-005 — Navigation and noninterference

WHEN a reader follows an event's conversation link, THE SYSTEM SHALL use the canonical conversation route and preserve the originating Coordinator return context where applicable. Events SHALL remain excluded from human chapter navigation. Presentation SHALL NOT change event delivery, message ordering, grouping, deduplication, history boundaries, scroll policy, or conversation lifecycle semantics. Replay accounting SHALL consume a size-only projection rather than expose a presentation-bypassing wire conversion.

## REQ-CEVT-006 — Saved goal facts without outcome reinterpretation

WHEN a local subscription event carries goal facts under `specs/conversation-goals/requirements.md` REQ-GOAL-011 through REQ-GOAL-014, THE SYSTEM SHALL project the event-owned saved facts, not a later current goal or an admission-time execution binding. The presentation SHALL distinguish a captured goal (owner, immutable identity/objective, owner/status version, state and exact reason), captured absence of a goal, historical facts not captured, and explicitly unavailable usage.

THE SYSTEM SHALL keep goal facts separate from the event's execution, wait or goal-change outcome. A completed execution SHALL remain “Turn finished” regardless of saved goal state. A standalone goal-change event SHALL be identified as a goal change, not execution completion, permission to execute, or an instruction to recover or continue.

THE SYSTEM SHALL preserve these distinctions through the read paths and shared-read redaction of REQ-CEVT-004, with inspectable saved evidence under REQ-CEVT-003. A missing or invalid event projection SHALL use the existing unavailable-details fallback rather than reconstruct facts from live goal storage. Mandatory Close and federation terminal-fact routes, source provenance, message acceptance, navigation and ordering SHALL remain unchanged.

Related contracts: [Conversation UI](../conversation-ui/requirements.md), [Conversation retrieval](../conversation-retrieval/requirements.md).
