# ADR-260: Retain Coordinator intent independently of continuations

- **Status:** Accepted
- **Date:** 2026-11-15
- **Affects:** REQ-TRACK-001–007, REQ-GR-011

## Context

User requests and useful questions disappear from generated handoffs despite surviving in history. Existing Goals and subscriptions describe known worker execution, rather than retaining forgotten intent. The user authorized one singleton list, explicit retirement and eventual automatic capture, and required immutable conversation prefixes.

## Decision

Retain normalized items and ordered evidence under stable Coordinator identity. One closed native operation union reads and mutates through one service. Trusted call identity supplies retry identity; revision checks prevent stale updates. Evidence append advances an ordered watermark; explicit review acknowledges exactly that watermark. Historical notes cannot appear as ready steps when new evidence exists.

Append bounded, revision-labelled observations at existing admitted foreground boundaries. Never inject changing state into system instructions or replace prior observations. Keep full notes behind paginated reads. Static guidance asks Ember to connect older intent to current work and use one due requested item at most, with a 24-hour review interval; no new wakeups. First-page compact context is capped at eight entries and 8 KiB; detailed reads have separate evidence pages.

Mutation receipts retain only call fingerprint, item and committed revisions, not a second mutable item copy. Exact replay reports the original committed revision; callers read current state separately. Reusing a call with different intent conflicts. Closed items and reasons persist without age expiry; source references deliberately do not cascade on source deletion.

## Consequences

Manual memory can ship before extraction. Goals/events/tracking remain separate authorities. Prompt growth follows existing continuation policies, and compact pages explicitly omit entries. Stored text remains evidence rather than executable instruction. More elaborate ranking, status checkers and dashboards require demonstrated need.
