# ADR-145: Owner gestures bind intentional Slack returns

- **Status:** Accepted
- **Date:** 2026-10-23
- **Affects:** REQ-SG-001–007, REQ-SG-009–014, REQ-SG-016

## Context

A flag for later preparation and a direct request have different publication
intent. Slack search supplies current observations rather than reaction events.
Native conversations can also contain private follow-up discussion.

## Options considered

1. Reply to every flagged message: simple but publishes unsolicited briefings.
2. Infer intent from conversation text: flexible but unpredictable.
3. Define separate owner gestures with source-bound return authority.

## Decision

Fire on another author's message requests a private briefing. A fire-prefixed
owner message or fire on the owner's own message submits an explicit request
whose answer belongs in the source thread. Verify owner and workspace before
acceptance. Untriggered replies do not invoke the agent. Source identity
deduplicates prefix and reaction observations of the same message.

Reuse the ordinary native conversation for explicit requests in one thread,
including private Phoenix follow-ups as context. Only an intentional answer
covering specified accepted requests is eligible for publication. The agent may
use the owner's broader access and must curate its answer for the destination.
An unambiguous owner instruction authorizes its requested changes; ambiguous
changes require explanation and confirmation. Quoted material supplies context
unless the owner explicitly adopts its instruction.

Initialize with a non-executing baseline, then admit new observations within
four calendar dates. Eyes acknowledges durable acceptance only. One answer may
cover multiple recorded requests. Uncertain external outcomes require evidence
inspection and must not be blindly sent again.

## Consequences

- The owner can use three predictable gestures without another permission mode.
- Initialization can absorb gestures observed before readiness; removed or
  out-of-window triggers cannot be recovered from search.
- Shared replies need deliberate audience-aware composition, including when
  private native discussion supplied useful context.

## References

- `specs/slack-gateway/slack-gateway.allium`
- `slack_record_source`, `slack_prepare_reply`, `slack_observe_return`
