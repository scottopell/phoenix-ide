# ADR-070: Trusted input provenance and explicit Global conversation watches

- **Status:** Accepted
- **Date:** 2026-09-29
- **Affects:** REQ-GR-007, REQ-GR-014, REQ-GR-015

## Context

Cross-conversation messages entered the ordinary chat path without persisted sender attribution. The user approved explicit Global Coordinator watches to report execution facts, including normal endings, without asking infrastructure to judge task completion. The same design session required factual packets rather than recovery instructions, future occurrences only, and reuse of ordinary input admission.

## Decision

Trusted server boundaries assign input origin. API channel origin is not proof of biological human authorship. Internal sends preserve stable sender conversation and exact transcript identity. Origin is constrained, queryable persisted data threaded through steering, history, wire transport, UI, and model rendering. Historical rows without recorded evidence remain unknown; migration does not invent human attribution.

Explicit Global watches follow stable conversation identity across continuation. An active watch observes future source occurrences, not history. Source settlement records an owed notification in its transaction. Admission of the notification uses the ordinary durable input path; acceptance and watch removal/Close serialize in SQLite. Accepted input is never retracted or redelivered because the recipient was stopped. Stop is not unsubscribe.

Packets report outcome and identity facts only. Natural-language facts are allowed; behavioral guidance is not appended. Automatic continuation authority decides whether execution handed onward; subscription delivery does not infer continuation from elapsed time.

Delivery discovery reuses the direct-turn worker rather than adding a subscription scheduler. Source and recipient lifecycle authority remain with their existing owners. No Project Coordinator extension or identity-grammar replacement is introduced.

## Alternatives rejected

- Unattributed user-role text prefixes as the only stored representation: loses trustworthy provenance on retrieval and replay.
- Historical-human default: fabricates attribution that was not recorded.
- Generic event bus, predicates, and separate execution scheduling: unnecessary for a fixed bounded event set.
- Pause-on-Stop and notification replay on recipient cancellation: adds persistent control modes and can create restart loops.
- Catch-up mode: duplicates an ordinary current-state read at enrollment.

## Consequences

Input-origin fields cross persistence and transport boundaries, so regression coverage must include both direct and queued acceptance and source attribution in model-bound text. Source failure coverage and continuation ordering need explicit tests. The implementation's executive status records verification limits rather than treating type compilation as proof of crash/race behavior.

The feature establishes no downgrade, live-resource replacement, or cross-version replay guarantee beyond the existing compatibility policy. Unshipped migrations in this change are published as one final schema contract.
