# ADR-261: Completion events capture occurrence-bound ending evidence

- **Status:** Accepted
- **Date:** 2026-11-16
- **Affects:** REQ-CEVT-006–009, REQ-RET-012

## Context

Ember commonly reads the latest worker messages after a completion notification.
Delayed notifications can point at a newer response, and an exact message ID alone
does not protect an omitted conclusion from later edits. The user requested useful
existing events without more notifications or a separate supervision system.

## Options considered

1. Fetch the latest response at delivery: small change, incorrect attribution.
2. Store every full historical response: reliable history but unnecessary duplication.
3. Capture one bounded quotation and a full-response version expectation atomically
   with the terminal occurrence; recover omitted text only while that version matches.

## Decision

Choose option 3. Capture from the exact terminal obligation before settlement deletes
it, threading the same typed response binding into direct and adopted-input events.
No exact carrier means explicit absence. Replays reuse original evidence and cannot
borrow later response text. Keep settled-input origin and sender facts distinct from
the response; multiple inputs may have contributed.

Normalize event-owned evidence in a child table. Retain the quotation with the event
even after source edits/deletion; source removal makes further reads unavailable.
The version-1 canonical SHA-256 covers the complete typed agent content, including
non-prose blocks. Capture at most 1,024 UTF-8 bytes of verbatim text, with explicit
clipping and tool-only/empty distinction. Current-source navigation is not historical
recovery. An event-bound read validates the full digest and returns one bounded prose
page from that same database snapshot; repeat validation on every page.

Existing private event details and model input consume the same typed authority.
Shared/redacted views acquire no private evidence. Remote terminal-fact v1 remains
unchanged and explicitly lacks this capability. Delivery, waits, suppression and
continuation semantics remain unchanged. No model summary is generated.

Before activating compact model projection, compare baseline/excerpt/compact variants
on a pinned synthetic corpus: zero attribution or authority violations, at least 25%
fewer targeted source-message retrievals, at most 10% total input-token regression
and 20% median latency regression in no-useful-excerpt controls. Failed gates retain
capture and honest evidence inspection without claiming prompt optimization.

## Consequences

Small immutable quotations survive source edits without a historical transcript
store. Later reads may honestly refuse changed or deleted content. Existing events
have historical-not-captured evidence; no historical inference/backfill is performed.
Captured evidence is retained until its owning event is deleted. Scanner, tracking,
remote protocol evolution and event-frequency policy remain separate concerns.
