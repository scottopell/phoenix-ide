# Seed PR badge from cached association before gh refresh

## Problem

When switching to a Work/Branch conversation that already has an associated PR, the conversation state bar initially renders without the PR badge. The badge appears only after `GET /api/conversations/:id/pr-status` completes a fresh `gh`-backed load. This makes the GitHub link feel unnecessarily delayed, especially when the user only wants to click through to the PR.

The code already stores a compact work-scope PR association and already uses it for sidebar `cached_pr` badges. The state bar ignores that cached association and waits for the richer PR status refresh.

## Plan

1. Extend the single-conversation serialization path so `conversation.cached_pr` is included consistently, not just in list/sidebar responses.
   - Reuse the existing `conversation_work_scope`, `sidebar_cached_pr_summary`, and `primary_work_scope_pr_association` path.
   - Keep this as a compact, DB-backed snapshot; do not run `gh` during conversation load.

2. Seed `useConversationPrStatus` from `conversation.cached_pr`.
   - Add an optional cached PR input to the hook.
   - On a valid Work/Branch scope with cached PR, initialize/render a `ready` `PrStatusResponse` immediately with `found: true`, link/number/title/display state/base/head populated, and explicit refresh/work-change metadata indicating the rich status is still loading or stale.
   - Continue the existing immediate `api.getPrStatus(conversationId)` call in the background so checks, draft/open state, freshness, and work-change status update when `gh` returns.
   - Preserve stale-result protection across conversation switches.

3. Wire `ConversationPage` to pass `conversation.cached_pr` into the hook.

4. Tests
   - Backend: single-conversation response includes `cached_pr` when a primary work-scope PR association exists.
   - Hook/UI: with cached PR, state is ready on first render and `getPrStatus` is still called; when the fresh response resolves, it replaces the cached seed.
   - Regression: switching conversations must not allow a previous conversation’s cached seed or late fresh response to appear in the new state bar.

## Acceptance criteria

- Navigating to a conversation with a stored PR association shows the PR badge/link in the state bar on first paint after the conversation payload arrives.
- No `gh` call is required before the clickable PR link appears.
- The richer existing PR status refresh still happens and updates the badge/status when complete.
- Conversations without a cached PR behave as they do today.
