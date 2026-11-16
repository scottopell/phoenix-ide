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

Related contracts: [Conversation UI](../conversation-ui/requirements.md), [Conversation retrieval](../conversation-retrieval/requirements.md).

## REQ-CEVT-006 — Occurrence-bound ending evidence

WHEN a local execution settles with an exact authoritative response carrier, THE
SYSTEM SHALL atomically capture its exact transcript/message identity, full canonical
content digest/version, bounded verbatim ending prose and truncation provenance with
the completion event. Direct and adopted-input occurrences SHALL use the same captured
response binding before obligation deletion. Replay SHALL retain original evidence or
explicit absence, never borrow a later response. Settled-input identity/origin/sender
facts SHALL remain distinguishable from a sole-cause attribution.

## REQ-CEVT-007 — Honest absence and retention

THE SYSTEM SHALL distinguish captured prose, exact response without prose, unavailable
response, no exact carrier, historical-not-captured and unsupported remote enrichment.
Captured quotations SHALL remain immutable with their owning event after source edit
or deletion. Increasing capture bounds SHALL NOT rehydrate old excerpts. Historical
events SHALL NOT acquire inferred evidence. Source deletion SHALL make further source
recovery unavailable without altering the recorded outcome.

## REQ-CEVT-008 — Evidence visibility

THE SYSTEM SHALL expose occurrence-bound evidence through existing private event
inspection and event-bound reads, preserving authentication and redaction. Shared reads
SHALL NOT acquire private evidence or sender facts. Worker quotations SHALL remain
untrusted factual evidence, not user requests, authority or proof that a broader
tracked user outcome is resolved. Remote terminal-fact v1 SHALL remain unchanged.

## REQ-CEVT-009 — Measured noninterference

THE SYSTEM SHALL preserve existing notification frequency, input admission, wait,
deduplication, suppression and continuation behavior. Compact model projection SHALL
be qualified against declared source-correctness, authority, retrieval, total-context
and latency thresholds before activation; packet length alone SHALL NOT establish
effectiveness.
