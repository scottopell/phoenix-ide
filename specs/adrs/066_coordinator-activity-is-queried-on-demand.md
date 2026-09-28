# ADR-066: Coordinator activity is queried on demand

- **Status:** Accepted
- **Date:** 2026-09-27
- **Supersedes:** ADR-022's automatic activity snapshot injection and ADR-027's retention of that snapshot
- **Affects:** REQ-GR-010, REQ-GR-011, REQ-GR-013

## Context

The Global Coordinator injects a fresh activity snapshot before conversation history on every ordinary model call. The OpenAI adapter flattens the system blocks into instructions; changing activity states and timestamps therefore changes an early request prefix. The snapshot also compares a continuation root against the current Coordinator head, so after continuation it includes the Coordinator's own changing state.

Production accounting showed 2,166 Coordinator Astra calls without cache reads in a thirty-day sample, with zero-cache Sol calls also present after continuation. The user uses the Coordinator for foreground supervision, evidence queries, messaging, and authorized deployment. All eight exposed tools had usage; deleting those tools would remove used capabilities without addressing the unstable prefix.

## Options considered

1. Repair only the Coordinator self-exclusion predicate. Other changing activity rows still alter the early prefix.
2. Freeze or relocate an automatically replaced snapshot. This retains bespoke injection machinery and requires careful prefix and freshness semantics.
3. Remove automatic injection and use the existing bounded query capability when current facts are needed.

## Decision

Choose option 3. Remove snapshot structs, SQL, serialization, request insertion, and the runtime dependency used solely for injection. Keep the bounded query, history, reference, messaging, Bash, and skill capabilities. The Coordinator discovers authoritative active WorkScope identities through explicit queries before Bash use.

Retain Brief me as a normal read-only composer action requesting fresh facts, user decisions and blockers first, and progressing work next. It does not start polling or send messages. Keep normal Coordinator identity, continuation, and protected handoffs.

## Consequences

- Stable system instructions no longer depend on current work activity.
- A briefing may require an explicit query call; ordinary supervision steps no longer rebuild and prepend an unrelated global snapshot.
- Current-state claims require fresh queried evidence; retained transcript observations remain historical.
- No new persistence, watcher, background subscription, or cache abstraction is introduced.
- Prefix stability removes a demonstrated invalidation mechanism but does not guarantee provider cache hits or a particular quota reduction.

## References

- `specs/global-recall/requirements.md`
- `ConversationRuntime::dispatch_llm_request`
- `GlobalReadService::query_database`
- `COORDINATOR_BRIEFING_PROMPT`
