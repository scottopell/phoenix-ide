# ADR-038: Commission review is retired with forward history recovery

- **Status:** Accepted
- **Date:** 2026-08-17
- **Affects:** REQ-CR-001 through REQ-CR-018; REQ-COMP-001 through REQ-COMP-005; REQ-VS-006, REQ-VS-016

## Context

Phoenix had a second code-review workflow in which an agent requested `commission_review`, a conversation entered a dedicated approval state, and an internal approved tool performed an LLM review with a specialized result viewer. The workflow duplicated product attention already served by ordinary pull-request review, could spend substantial tokens, and could strand a persisted conversation behind feature-specific approval authority.

Retirement has two different data concerns. Pending approval state contains an assistant tool-use message that has not yet been copied to the transcript. Historical transcripts contain stored tool-use and tool-result blocks that are user-visible records. Removing the Rust state and tool types without transforming the first loses or strands data; retaining specialized execution/viewer types merely to read the second preserves unwanted authority.

ADR-034 permits an explicit one-way forward migration and does not require downgrade, mixed-version, or old-link compatibility.

## Options considered

1. **Repair and harden commission review** — make approval and execution resumable. This retains duplicate review authority and its token, lifecycle, prompt, provider, and viewer complexity.
2. **Hide execution but retain specialized state and viewer types indefinitely** — avoids new discovery but leaves stale approval endpoints, writable lifecycle concepts, and a permanent parallel result representation.
3. **Retire authority, migrate pending state, and preserve history generically** — remove all new discovery/execution and specialized lifecycle/viewer machinery; transform pending approvals into a valid error-paired transcript; render existing blocks through generic read-only tool history.

## Decision

Choose option 3.

A one-way migration materializes the carried assistant message for every persisted commission-review approval, appends exactly one generic error tool result for the same tool-use identifier, and moves the conversation to idle. The migration performs no provider dispatch or tool execution and is idempotent.

The tool name, internal approved alias, approval state/events/endpoints, specialized result model, and viewer-slot variant are not live application types. Historical message content remains unchanged and is consumed only by generic read-only transcript and LLM-history sinks. Old approval endpoints and specialized viewer URLs receive no compatibility guarantee.

REQ-CR-001 through REQ-CR-015 are deprecated because they require the retired execution and specialized result capability. REQ-CR-016 through REQ-CR-018 define the retirement boundary.

## Consequences

- **Positive:** Agents cannot discover or execute the retired reviewer, and stale calls receive the ordinary bounded unknown-tool error.
- **Positive:** Pending approvals recover to an interactive non-success state without duplicate dispatch or fabricated findings.
- **Positive:** User transcript history remains readable without retaining specialized writable authority or parallel result parsing.
- **Negative:** Specialized commission-review URLs, approval API clients, and older binaries are not supported after migration.
- **Negative:** Historical result payloads lose their dedicated presentation and appear as generic tool history.
- **Neutral:** Ordinary pull-request review, task approval, message/prose review, and diff viewing remain separate and unchanged.

## References

- ADR-034: compatibility guarantees are explicit and data-aware
- `specs/compatibility/requirements.md`
- `specs/commission-review/requirements.md`
- Migrations 68 and 69
- `strip_unavailable_tool_blocks`
- `AgentMessage`
