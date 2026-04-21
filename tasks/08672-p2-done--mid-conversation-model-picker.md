---
created: 2026-04-21
priority: p2
status: done
artifact: ui/src/components/StateBar.tsx
---

# Mid-conversation model picker (cross-family / cross-tier)

## Summary

Surface a general model picker on idle conversations so users can switch
between any available models (e.g. sonnet-4-6 -> opus-4-7), not just the
1M upgrade variant of whatever they started with.

## Context

The backend has supported arbitrary model switching since task 08638:
`POST /api/conversations/:id/upgrade-model` accepts any model ID the
registry knows about, validates it, evicts the runtime, and the next
turn uses the new model (handlers.rs:1470-1516). The only precondition
is `ConvState::Idle`.

The frontend only wires up *one* case of this API: the
`currentModel + '-1m'` suffix check in `StateBar.tsx:225-234`. If the
current model doesn't have a 1M sibling -- or you want to cross
families/tiers -- there is no UI. Users who start in sonnet-4-6 cannot
switch to opus-4-7 without abandoning the conversation.

Task 08638's checklist claimed "User can switch models on an idle
conversation" but only the 1M upgrade case actually shipped. This task
closes that gap.

## Acceptance Criteria

- [x] On an idle conversation, user can open a model picker from the
      StateBar and select any available model (filtered by the same
      rules as the /new page: recommended-only by default, "show all"
      to expand).
- [x] Picker uses `api.upgradeModel()` -- no new backend work.
- [x] Picker is disabled (or hidden) when conversation is not idle,
      matching the existing 1M upgrade gate.
- [x] 1M "Switch to 1M?" affordance folded into the general picker
      (option b). The 1M variant is just another model in the
      dropdown; users who want 1M pick it like any other model. The
      `.model-1m-badge` on the current-model label remains so users
      can see at a glance they're on a 1M variant.
- [ ] Changing the model is reflected in the StateBar model label
      without a page reload (the conversation update flow already
      refreshes on model change via upgrade endpoint). -- DEFERRED TO
      QA-2: current implementation falls back to
      `window.location.reload()` after success, matching previous
      behaviour. Full-reload visual verification belongs to browser
      QA.

## Notes

- Existing 1M upgrade flow: `StateBar.tsx:225-310` (state,
  confirmation popover, ref).
- Backend handler: `src/api/handlers.rs:1461-1516`.
- Related task (closed but incomplete on this axis): 08638.
- Coordinate with task referenced for mobile StateBar layout fix --
  adding a picker will likely compete for the same horizontal space
  that's already overflowing on iPhone portrait.
