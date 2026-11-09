# ADR-246: Trusted Coordinator inline research can select saved peers for read-only SQL

- **Status:** Accepted
- **Date:** 2026-11-09
- **Affects:** REQ-RQD-001/010, REQ-FED-001, REQ-GR-007

## Context

ADR-222 separated Coordinator remote SQL selection from ordinary local-only query registrations. That authority split prevented ordinary agents from querying saved peers, but it also prevented a trusted inline research child of the Coordinator from completing a delegated investigation when the evidence lived on another enrolled instance. The Coordinator could make the same remote read itself; requiring a parent round trip for every delegated query made the child tool easy to hold wrong. Remote SQLite reads are operator-level forensics, not a confidentiality sandbox, so this choice needs an explicit authority boundary.

## Decision

The Coordinator's trusted inline research children receive the same optional saved-peer `instance` selector on their one `query_database` tool as the Coordinator. Eligibility comes from the persisted Coordinator parent runtime role when constructing or reconstructing the inline registry, never from a model-supplied destination or claimed role. Other sub-agents and ordinary ProductConversations retain their local-only schema. The child uses the existing enrolled-peer resolver, typed caller/destination identity, receiver-enforced read-only/credential-column controls and budgets, and no local fallback after remote failure. Coordinator parentage grants selection of any saved peer available to that Coordinator; the parent should name the intended destination in its assignment, but the assignment text is not an enforced per-peer grant. The model cannot enroll peers, supply URLs or credentials, or mutate remote state through this tool.

## Options considered

- Keep all children local-only and have the parent relay bounded remote results. This creates avoidable parent coordination work and leaves delegated research incomplete until another round trip.
- Expose the selector to every `query_database` registration. This expands remote forensic authority beyond the Coordinator's trust domain.
- Add a separate remote-query tool to the child. This duplicates discovery and encourages accidental routing rather than explicit selection on the single read capability.

## Consequences

An authorized child can directly answer a delegated saved-peer research question and must report destination identity and untrusted evidence. Enrollment and receiver admission remain directional and destination-owned; no reciprocal owner authority, remote writes, arbitrary network access, or additional ordinary agent privilege is implied. The operator must recognize that successful SQL may expose hidden application state outside ordinary UI visibility. The older ADR-222 describes the initial decision and remains historical; this decision narrows its local-only child exclusion without altering the ordinary-agent split.

This decision changes the live tool schema for Coordinator research children; it does not promise mixed-version live-resource replacement or database downgrade behavior.
