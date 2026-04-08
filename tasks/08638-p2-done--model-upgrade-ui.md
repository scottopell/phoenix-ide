---
created: 2026-04-07
priority: p2
status: done
artifact: ui/src/components/StateBar.tsx
---

# Frontend UI for model upgrade flow

## Summary

The backend supports upgrading a conversation's model via
`POST /api/conversations/{id}/upgrade-model` and the frontend API client
has `api.upgradeModel()`. But there's no UI to trigger it.

## What to build

1. **Upgrade button in StateBar or model display**: When the current model
   has a 1M variant available (e.g., user is on `claude-sonnet-4-6`, and
   `claude-sonnet-4-6-1m` exists), show an upgrade affordance.

2. **Confirmation**: Brief confirmation since this changes model mid-conversation.
   "Upgrade to 1M context? The conversation will use the extended context
   window for all future messages."

3. **Model selector in conversation settings**: Allow changing model on an
   idle conversation (not just at creation time). The upgrade endpoint
   already handles validation.

4. **Context window indicator**: The StateBar already shows context usage.
   When on a 1M model, the bar should reflect the larger window. This
   already works (context_window comes from the model spec), but verify
   the visual scales correctly for 1M.

## Done when

- [x] User can upgrade from 200k to 1M variant from the UI
- [x] User can switch models on an idle conversation
- [x] Context window display reflects the upgraded model
- [x] Upgrade is disabled when conversation is not idle
