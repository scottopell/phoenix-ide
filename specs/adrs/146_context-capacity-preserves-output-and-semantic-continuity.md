# ADR-146: Shared capacity planning preserves output headroom and semantic continuity

- **Status:** Accepted
- **Date:** 2026-10-25
- **Affects:** REQ-LLM-004f/004j/004k/004l/004m, REQ-BED-020a, REQ-STR-012, REQ-AN-011

## Context

The declared-output policy in ADR-144 removed effort-dependent output starvation. Production preflight pressure nevertheless reached a generic context-exhausted notice before the ninety-percent response threshold, bypassing handoff generation. The user chooses to preserve full output headroom and reduce input, values model switching, rejects a second memory authority, and wants cache-friendly behavior without frozen stale instructions. User decisions on October 18 favored provider recommendations and accepting necessary cache misses; task 36009 records the superseded manual-refresh design and accepted turn-boundary refresh.

## Options considered

1. Shrink output to retain more history. This undermines the chosen reasoning-headroom default.
2. Preserve full output, sharing capacity accounting and using existing clearing and durable compaction. This accepts more deliberate cache boundaries but preserves one lifecycle and transcript authority.
3. Maintain extracted obligation memory, a separate Coordinator strategy, immutable instruction bundles and counting jobs. This adds synchronization and lifecycle concerns without demonstrated need.

## Decision

Choose option 2. Clarify ADR-144's ambiguous context clamp: input reduction precedes dispatch; irreducible conflicts are visible rather than silently shrinking ordinary or handoff output. Keep bounded utility budgets. Common accounting does not require common provider serialization or identical ordinary/summary retention.

Use route-supported counting only when it can change a reduction/refusal decision; bound the entire logical planning operation, across clearing and handoff candidates, to one remote counting attempt and two seconds, with visible local fallback. The timeout is an engineering default for this proposal rather than a measured provider latency result. No mandatory remote counting, endpoint probes, model downloads, or counting recovery service.

Preserve one transcript and existing handoff lifecycle. Keep ordinary project guidance stable within a user turn and refreshed at the next turn; live authority remains independent. Prefer worthwhile monotonic clearing and stable provider serialization, accepting necessary rewarm at semantic boundaries. Extend existing content-free metrics and usage views rather than create new analytics or memory infrastructure.

## Consequences

- **Positive:** reasoning headroom, model switches, clearing and compaction use one capacity contract; errors cannot masquerade as handoffs.
- **Negative:** full output reservation reduces available history; compaction and necessary prefix changes can increase rewarm cost. Complete provider accounting is not uniformly available. Counting adds bounded latency near pressure.
- **Neutral:** no provider cache-hit guarantee, automatic model fallback or production repair is authorized by this documentation. Existing continuation consent, tool authority and Close fences remain authoritative.

## References

- [ADR-144](144_output-budget-is-declared-model-maximum-and-exhaustion-is-terminal.md)
- [LLM requirements](../llm/requirements.md)
- [Bedrock requirements](../bedrock/requirements.md)
- [Stale-result requirements](../stale-tool-results/requirements.md)
- [Analytics requirements](../analytics/requirements.md)
