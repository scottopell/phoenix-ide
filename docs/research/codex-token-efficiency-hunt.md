# Codex token-efficiency hunt

## Executive result

Phoenix has adopted the material request-shaping and transport optimizations in current upstream Codex: a stable conversation cache key, Responses Lite, reasoning context across turns, and safe WebSocket continuation by `previous_response_id`. A fresh source audit found no additional small parity patch with demonstrated quota impact.

The remaining measured high-impact gap is system-prompt stability. Phoenix rebuilds repository guidance, task hints, skills, and mode context for every request while reusing a conversation-stable cache key. A one-line guidance mutation during a live conversation reduced the cached read from 17,152 tokens to 5,888 and increased uncached input from 796 to 19,287 tokens. Task 36009 already owns the required persisted prompt snapshot and explicit transcript-generation lifecycle; this hunt does not duplicate that schema and lifecycle work.

No token-reduction code is shipped by this hunt. Other upstream differences are either already implemented, transport-byte optimizations rather than prompt-token savings, or policies whose quota benefit has not been established without risking context loss.

## Pinned source

Source: `openai/codex` commit [`c888e8e75a9f0e90ce7d5517f8b9540832cbbf76`](https://github.com/openai/codex/tree/c888e8e75a9f0e90ce7d5517f8b9540832cbbf76), committed 2026-07-12 (`Improve composer completion target resolution (#32628)`).

Audited areas:

- `codex-rs/codex-api/src/common.rs`: canonical Responses and WebSocket request shapes, `prompt_cache_key`, Responses Lite input projection, and reasoning context.
- `codex-rs/codex-api/src/endpoint/responses_websocket.rs`: strict-prefix continuation, non-input request compatibility, and full-request fallback.
- `codex-rs/core/src/context_manager/history.rs`: raw history, deterministic prompt projection, tool-output truncation, and token estimates.
- `codex-rs/core/src/session/turn.rs`: pre-sampling compaction and pending-input ordering.
- `codex-rs/core/src/compact_remote_request.rs` and `compact_remote_v2_attempt.rs`: normalized compaction input and retained-message budgets.
- `codex-rs/core/src/session/context_window.rs` and `token_budget.rs`: prefix-aware compaction scope.
- `codex-rs/tools/src/response_history.rs`: bounded assistant output and tail retention for specialized response-history consumers.

## Reproducible live baseline

Environment: Phoenix development server, authenticated ChatGPT Codex backend, GPT-5.6 Sol, 2026-07-12. Credentials, account identifiers, response bodies beyond fixed probe strings, and raw wire payloads are not retained. The worktree database path was obtained from `./dev.py status`; measurements came from `turn_usage`.

The provider usage contract stores uncached input in `input_tokens`, cached reads in `cache_read_tokens`, and cache writes in `cache_creation_tokens`. Therefore:

```text
total prompt = input_tokens + cache_read_tokens + cache_creation_tokens
```

| Turn | Scenario | Uncached input | Cached read | Cache write | Total prompt | Output |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Cold conversation | 17,736 | 0 | 0 | 17,736 | 8 |
| 2 | Warm text turn | 609 | 17,152 | 0 | 17,761 | 8 |
| 3 | Tool request | 648 | 17,152 | 0 | 17,800 | 51 |
| 4 | Post-tool continuation | 796 | 17,152 | 0 | 17,948 | 9 |
| 5 | One-line `AGENTS.md` mutation before request | 19,287 | 5,888 | 0 | 25,175 | 9 |

The mutation was reverted immediately after the probe. Latency is intentionally not used as cache-hit evidence.

### Interpretation

- Normal warm and tool-loop turns reuse a 17,152-token prefix and add only 609–796 uncached tokens.
- Cache writes remained zero, matching earlier Codex-auth observations; automatic cache reads are the relevant quota signal.
- A tiny mutation in a leading guidance component invalidated at least 11,264 cached tokens and caused an 18,491-token increase in uncached input compared with the preceding turn.
- The total prompt also grew because Phoenix rebuilt the effective prompt from mutable repository state. The experiment demonstrates the risk but does not claim that every token difference is attributable to the appended line alone.

## Upstream comparison

| Technique | Current upstream Codex | Phoenix | Quota classification |
| --- | --- | --- | --- |
| Stable cache cohort | Thread/session-derived `prompt_cache_key` | Conversation-derived `PromptCacheKey::stable` | Adopted |
| Responses Lite prefix | Tools and base instructions projected into stable developer input | Implemented for Codex-auth models | Adopted |
| Reasoning context | `all_turns` in Responses Lite | Implemented | Adopted |
| WebSocket continuation | Incremental suffix plus `previous_response_id` after strict compatibility checks | Implemented with conservative fallback | Transport-byte saving; does not independently prove token saving |
| Raw history vs prompt projection | Deterministic `ContextManager::for_prompt` projection | Stored messages projected and stale tool results cleared at dispatch | Different architecture; no measured gap |
| Tool-output truncation | Per-tool truncation policy at history record time | Monotonic stale-result clearing under context pressure | Different policy; no evidence to replace it |
| Compaction timing | Pre-sampling local/remote compaction with prefix-aware scopes | Context-pressure stale-result sweep and continuation summary paths | Candidate for future controlled measurement, not a safe parity copy |
| System instruction lifetime | Step/session context supplies request instructions | Mutable system prompt rebuilt each request | Measured material gap; task 36009 |
| Tool definition order | Model-visible specs originate from deterministic tool configuration | Registry definitions retain deterministic registration order | No observed per-turn mutation |
| Usage accounting | Cached reads parsed; cache-write detail is backend-dependent | Reads and writes parsed and persisted separately | Phoenix is at least equivalent |

## Candidate ranking

| Rank | Candidate | Expected quota impact | Risk / cost | Decision |
| ---: | --- | --- | --- | --- |
| 1 | Persist system-prompt snapshot with explicit refresh/generation boundary | High; live mutation invalidated a large prefix | High: schema, migration, recovery, mode transitions, refresh semantics | Implement in task 36009 |
| 2 | Measure compaction timing and cache rewarm at a real context boundary | Potentially high in long sessions | High live quota cost; semantic continuity risk | Do not change without a dedicated long-context fixture |
| 3 | Canonically sort tool definitions | Low unless registry assembly becomes nondeterministic | Sorting can alter established prefix and tool priority assumptions | No change; current registry order is deterministic |
| 4 | Retain only the last N user turns | Superficially high | Unacceptable context loss for general conversations | Reject |
| 5 | Narrow reasoning context from `all_turns` | Unknown | Can remove model-relevant reasoning continuity and diverges from Codex contract | Reject without provider evidence |
| 6 | Treat WebSocket continuation as token optimization | None demonstrated | Misleading accounting | Report only as transport optimization |

## Cold, warm, tool-loop, and compaction coverage

Cold, warm, tool-loop, and post-tool behavior were measured directly. A real post-compaction probe was not forced: the controlled conversation was far below the compaction threshold, and manufacturing a near-window transcript would spend substantial live quota while testing a different policy from upstream remote compaction. Existing stale-result clearing tests cover deterministic semantic behavior; a cache-rewarm measurement belongs with any future compaction-policy change.

## Conclusions

1. Preserve the existing stable cache key, Responses Lite shape, reasoning context, and WebSocket continuation.
2. Keep prompt-cache accounting separate from transport bytes. `previous_response_id` reduces retransmission but is not evidence of fewer uncached tokens.
3. Do not prune general history, narrow reasoning context, or replace stale-result clearing based only on source similarity.
4. Prioritize task 36009. Its persisted snapshot must make refresh/restart a visible transcript-generation boundary and preserve mode semantics and recovery.
5. Re-run the same mutation matrix after task 36009; success means the active conversation retains a byte-identical prompt and a warm-sized uncached suffix until an explicit generation refresh.
