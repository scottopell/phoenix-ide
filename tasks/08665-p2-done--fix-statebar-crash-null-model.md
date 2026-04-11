---
created: 2026-04-10
priority: p2
status: done
artifact: ui/src/components/StateBar.tsx
---

# Fix StateBar crash when conversation.model is null

`abbreviateModel(conversation.model)` crashes with `TypeError: Cannot read properties of null (reading 'startsWith')` when a conversation is created without specifying a model (model field is null in the API response).

Found during QA of terminal feature. Fixed inline: added `?? ''` guard.

## Fix

```ts
// Before (crashes if conversation.model is null):
const modelAbbrev = conversation ? abbreviateModel(conversation.model) : '';

// After:
const modelAbbrev = conversation ? abbreviateModel(conversation.model ?? '') : '';
```
