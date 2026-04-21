---
created: 2026-04-21
priority: p2
status: done
artifact: ui/src/conversation/atom.ts
---

# Derive context window total from model, drop atom.contextWindow.total

## Summary

`atom.contextWindow` currently stores `{ used, total }`. `used` is real
state (changes over time as tokens accumulate); `total` is a pure
function of the current model's spec. Storing both creates a parallel
representation and forces hand-sync on every model change.

Drop `total` from the atom. Derive it at read time from
`availableModels.find(m => m.id === conversation.model)?.context_window`.

## Context

This violates AGENTS.md "No parallel representations of the same
semantic value." The denormalization is load-bearing for the current
`handleUpgradeModel` reload hack -- after a model switch,
`atom.contextWindow.total` is stale until SSE reinitializes, so the
code calls `window.location.reload()` as a sledgehammer.

Once `total` is derived, the no-reload switch is a single dispatch:
`{ type: 'sse_conversation_update', updates: { model: newModelId } }`.

## Acceptance Criteria

- [x] `atom.contextWindow` is typed as `{ used: number }` (no `total`).
- [x] `StateBar`'s `modelContextWindow` prop is computed in
      `ConversationPage` from `availableModels` + `atom.conversation.model`,
      with a 200_000 fallback matching today's behaviour when
      availableModels hasn't loaded yet.
- [x] `handleUpgradeModel` in `ConversationPage.tsx` no longer calls
      `window.location.reload()`. Instead it dispatches
      `{ type: 'sse_conversation_update', updates: { model: newModelId } }`
      after the API call succeeds. The `showInfo` toast stays.
- [x] All three reducer cases that populate `contextWindow` (sse_init,
      set_initial_data, and any SSE event that updates usage) only set
      `used` -- no `total`.
- [x] Tests in `ui/src/conversation/atom.test.ts` updated to match.
- [x] `cd ui && npx tsc --noEmit` passes.
- [x] `cd ui && npx eslint src` passes.

## Files likely touched

- `ui/src/conversation/atom.ts` -- type + reducer cases
- `ui/src/conversation/atom.test.ts` -- fix any fixtures that set `total`
- `ui/src/pages/ConversationPage.tsx` -- compute `modelContextWindow`,
  drop reload from `handleUpgradeModel`
- `ui/src/components/StateBar.tsx` -- no logic change; `modelContextWindow`
  prop stays a `number`
- Anywhere else that reads `atom.contextWindow.total` -- grep to find

## Notes

- Verify manually that after a model switch (e.g. 200k -> 1M variant)
  the context bar's denominator updates without a page reload.
- `used` continues to be backend-sourced. The field name `context_window_size`
  from the API is a *usage* value, not a spec -- matches `used`.
