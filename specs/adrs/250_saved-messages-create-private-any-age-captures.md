# ADR-250: Saved messages create private captures of any age

- **Status:** Accepted
- **Date:** 2026-11-12
- **Affects:** REQ-SG-001, REQ-SG-003–007, REQ-SG-017, REQ-SG-019

## Context

Emoji reactions are visible and can notify the source author. The owner wants a
private capture action and explicitly wants saving an older message to work
regardless of its age. Controlled MCP reads demonstrate Saved membership changes,
two distinct paged results, and exact reply/root identities. Search returns source
timestamps, not save timestamps. The retired stars API does not track new saves.

## Options considered

1. Reuse emoji eligibility and handled identities. This is small but rejects old
   messages, can turn the owner's saved text into an instruction, posts eyes,
   and silently suppresses sources previously baselined for emoji discovery.
2. Add a Saved capture scope to the existing gateway and native briefing flow.
   This adds one discovery stream and independent readiness, but preserves one
   call allowance, serial service gate, and accepted-work lifecycle.
3. Add a Slack app shortcut or desktop capture service. This gives dedicated
   invocation semantics but requires another integration and deployment surface.

## Decision

Use the Saved search modifier through the existing owner-authenticated MCP
connection. All source ages and authors are eligible for a private briefing;
quoted source text never becomes an explicit instruction. There is no source
reaction or thread return, including on clarification, failure, or interruption.

Keep Saved readiness, scan progress and source scope independent of emoji
discovery. First enablement establishes a Saved baseline; subsequent retained-state
restarts and toggles preserve it. An emoji baseline/request cannot consume the
Saved identity. Repeated saves cannot repeat an accepted Saved capture. Unsaving
does not cancel accepted work or automatically delete its conversation.

Share serial service, the existing fixed-window call allowance, provider backoff,
soft burst deadline and accepted-work reserve. Rotate page opportunities across
enabled streams. Saved sweeps finish independently of paired emoji sweeps.
Preserve a failed Saved scan as incomplete, including a connector page ceiling;
never convert it into end-of-collection evidence. No independent retry worker,
automatic unsaving, larger quota or new permission mode is introduced.

## Consequences

Saved discovery scans current collection state rather than save-time events.
Polling cost grows with the collection, and a newly saved old message may be found
late in a sweep. Provider ceilings can prevent a full scan; visible incompleteness
is part of the contract. Supporting all ages does not promise unlimited collection
size, complete observation during changing pagination, or a discovery deadline.

Qualification must exercise the Phoenix-owned transport, source-age exceptions,
privacy on every return path, independent baselines, shared scheduling, retained
state, and a collection that crosses the provider paging boundary. Live capability
inspection alone does not qualify production activation.

## References

- [Saved capture requirements](../slack-gateway/requirements.md#req-sg-019--capture-a-discussion-privately-with-save-for-later).
- [Saved behavior](../slack-gateway/saved-capture.allium).
- [Shared discovery allowance](../slack-gateway/discovery.allium).
- [Retired stars API](https://docs.slack.dev/reference/methods/stars.list/).
