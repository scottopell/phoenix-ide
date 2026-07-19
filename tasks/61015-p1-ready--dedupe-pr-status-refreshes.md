Suppress redundant frontend PR-status refreshes while preserving the existing endpoint and status semantics.

Production traces show GET /api/conversations/:id/pr-status at p50 2.97s, p95 4.09s, and max 11.07s in a capped 1,000-result sample. The UI can trigger refresh from initial load, a 60-second schedule, visibility changes, and mutation follow-ups.

Implement scope-keyed in-flight request coalescing and a small freshness window in useConversationPrStatus so poll and visibility triggers do not duplicate work. Preserve explicit user refresh semantics where freshness is required, stale-response guards, cached seed behavior, and all current PR-selection states. Add hook tests for concurrent refresh deduplication, visibility-plus-timer suppression, scope changes, errors, and explicit refresh behavior.

No backend endpoint changes, durable workflow changes, or PR status model redesign.
