# Bound continuation summaries by provider input-item count

## Problem

Codex-routed OpenAI conversations continue to fail continuation compaction with `context_length_exceeded` after the minimum-token-headroom fix in PR #439 (`bf30e658`). The existing planner bounds estimated tokens but not the number of OpenAI Responses input items.

Read-only production evidence shows two failures after the headroom fix:

| Conversation | Failure time | Persisted messages before failure | Approx. flattened messages retained by current planner |
|---|---:|---:|---:|
| `phoenix-ux-async-conversion` | 2026-07-08 | 1,155 | 1,064 |
| `redesign-conversation-sync-around-separate-event-and-message-sequences` | 2026-07-09 | 1,367 | 1,096 |

The original pre-fix failure had 966 persisted messages and would retain approximately 927 under the new headroom planner. The post-fix failures therefore cross a new boundary near 1,000 retained messages even though the planner preserves 8,192 estimated tokens of context headroom.

`request_continuation` flattens tool blocks and caps each flattened block at 2,000 characters. `translate_to_responses_request` then emits one `ResponsesApiInputItem::Message` for each flattened text/image message. No code limits that item count. The observed failures are consistent with a Codex/Responses input-item ceiling around 1,000 that surfaces through the provider's broad `context_length_exceeded` classification.

## Goal

Make continuation-summary requests structurally fit both the model token window and the routed provider's request-shape limits, while preserving the newest contiguous history and a user-first opening message.

## Implementation plan

1. Add a continuation history item-count constraint alongside the existing token/headroom constraint.
2. Represent the applicable continuation request limits as typed route/provider capabilities rather than inferring them from model names or checking `uses_codex_bridge` ad hoc in the executor.
3. For the Codex bridge, set a conservative maximum history-message count below the backend ceiling, reserving one item for the appended continuation prompt. If the exact backend ceiling cannot be verified locally, use the production boundary evidence to choose and document a conservative value (for example, 900 history messages) rather than claiming an unverified exact limit.
4. Apply token and item constraints in one planner pass or a clearly ordered pair of passes. Drop oldest messages, then restore the user-first invariant. Re-check both constraints after user-first trimming.
5. Preserve existing behavior for routes without a known item cap; do not unnecessarily reduce Anthropic or direct platform-API continuation history.
6. Extend continuation diagnostics to log rendered count, final retained count, item cap, items dropped for the cap, token estimate, and headroom. Do not log message contents.
7. Add a regression fixture matching the production shape: more than 1,100 small tool-heavy messages whose token total fits below the current history budget. Assert that the Codex-routed plan retains no more than the configured history-item cap, remains user-first, preserves a contiguous newest suffix, and still satisfies token headroom.
8. Add a control test proving a route without an item cap is governed only by token/headroom constraints.
9. Verify that the translated OpenAI Responses request, including the final continuation prompt, stays under the configured total input-item limit.

## Resilience follow-up within scope

Inspect the provider error payloads captured by the existing Responses error logging for these two failures if available. If the backend supplies a distinct item-count code/message, classify and test it explicitly. If it only reports `context_length_exceeded`, keep the proactive route limit as the primary fix.

Do not add blind retries that resend the identical request. A single shrink-and-retry on context rejection may be added only if it uses a strictly smaller typed continuation plan, is bounded to one attempt, and has a regression test; otherwise leave adaptive retry as a separate follow-up.

## Acceptance criteria

- Codex-routed continuation history is bounded by both estimated token budget and a typed input-item limit.
- The final translated Responses request includes the continuation prompt without exceeding the configured total item limit.
- More-than-1,100-message tool-heavy regression coverage reproduces the previously unmodeled boundary and passes with the fix.
- The retained history remains the newest contiguous suffix and begins with a user-role message when non-empty.
- Non-Codex routes retain their existing behavior unless they declare their own typed item limit.
- Debug logs expose enough counts to diagnose future token-vs-item-limit failures without exposing content.
- Targeted continuation and OpenAI translation tests pass, followed by `./dev.py check`.
