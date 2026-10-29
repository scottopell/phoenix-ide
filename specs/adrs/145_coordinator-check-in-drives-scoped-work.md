# ADR-145: Coordinator check-ins drive scoped work and report outcomes

- **Status:** Accepted
- **Date:** 2026-10-24
- **Affects:** REQ-GR-010, REQ-GR-011, REQ-GR-013

## Context

The Coordinator's compact composer action requests a read-only briefing even when the user expects existing commissioned work to move. A delivery-result reporting instruction also makes quiet routine coordination sound like a series of tool receipts. A small synthetic evaluation finds that an active request can induce useful owner follow-ups, but the sampled responses remain opaque and procedural to the user. Neither model sends nor synthetic trials prove that an owner executed the request.

## Options considered

1. **Keep read-only briefing** — easy to understand as a query, but leaves unfinished work idle and repeatedly returns decisions already settled by standing authority.
2. **Adopt the evaluated C2 prose verbatim** — drives synthetic follow-ups and reduces compulsory receipts, but its reports still foreground machinery over the work and were rejected by the user.
3. **Request bounded coordination and a work-focused report** — use verified existing scope to advance routine next steps through established owners, while reporting the problem, intervention and direction. Retain genuine decisions, later pauses and delivery limits.

## Decision

Choose bounded active check-ins, not broad autonomous authority. Inspect current work including idle owners, recover relevant primary authorization, and move routine already-approved next steps through their existing owners. Advance independent work before asking a genuine user decision. A delivered or queued message is an intervention, not completion. Report material outcomes and decisions in plain English, with grouped links and optional grounded status/progress; do not make every tool receipt a user-facing update or poll indefinitely. Preserve reporting preferences through continuation without changing handoff generation or trust boundaries.

## Consequences

- **Positive:** A single check-in can resume commissioned work and explain its direction to a context-switching reader.
- **Negative:** The model must judge which routine interventions to make and when evidence limits materially affect the report; prompt wording alone does not guarantee execution or reporting quality.
- **Neutral:** Existing message acceptance, WorkScope targeting, primary-authority verification and approval gates remain unchanged. The selected product wording is informed by, but not live-qualified by, the synthetic comparison.

## References

- ADR-021 (chat-only surface), ADR-066 (on-demand current facts).
- `specs/global-recall/requirements.md` and `scripts/qa/coordinator-drive-eval/RESULTS.md`.
- `coordinator_prompt`, `COORDINATOR_CONTINUATION_SYSTEM_PROMPT`, `COORDINATOR_QUICK_ACTION`.
