# Add upstream-compatible Codex Responses WebSocket continuation

Upstream codex-rs prefers WebSockets for GPT-5.6 and sends incremental input with `previous_response_id` only when all cache-relevant request properties and the prior input/output prefix match. Phoenix currently sends full SSE/HTTP requests for every Codex-auth turn. Implement the WebSocket transport as a separate optimization from prompt caching, with safe HTTP fallback and exhaustive reuse-property matching.

Acceptance criteria:
- [ ] Codex-auth GPT-5.6 can use the upstream WebSocket beta contract and Responses Lite header.
- [ ] Incremental continuation is used only when model, instructions, tools, tool choice, reasoning, store/stream/include, service tier, cache key, text controls, and input/output prefix remain compatible.
- [ ] Any new request field requires an explicit continuation compatibility decision at compile time.
- [ ] Connection failure falls back to a full HTTP request without losing or duplicating conversation items.
- [ ] Retries, tool loops, model changes, compaction, and auth refresh have deterministic reset behavior.
- [ ] Tests distinguish reduced wire payload from backend cache hits; neither metric is used as a proxy for the other.
- [ ] Live measurements report payload bytes, first-token latency, cached tokens, and fallback behavior.
- [ ] `./dev.py check` passes.
