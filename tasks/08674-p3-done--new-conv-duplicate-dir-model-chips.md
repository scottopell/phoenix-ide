---
created: 2026-04-21
priority: p3
status: done
artifact: ui/src/pages/NewConversationPage.tsx
---

# /new page shows directory + model twice (top form + bottom chips)

## Summary

On the desktop variant of `/new`, the directory and model are rendered
both by `ConversationSettings` at the top of the card and by the
"chips" row near the send button. Since the top pickers are always
visible, the chips are redundant -- they display the same values in a
compact form and, when clicked, expand a third copy of the settings
panel.

## Context

Layout in `NewConversationPage.tsx:87-165`:

- Lines 89-114: `<ConversationSettings>` renders full Directory + Model
  pickers (plus mode, branches, etc.). Always visible in the desktop
  card.
- Lines 131-140: `new-conv-chips` row with `cwdDisplay` and
  `modelDisplay` chips. Clicking either toggles `showSettings`.
- Lines 148-164: `showSettings` expands `<SettingsFields>`, which is
  another copy of the directory/model pickers already shown up top.

The chips appear to be a mobile-era pattern that was left in place
when the desktop card layout was added. Result: user sees their chosen
directory and model three times, and the "expand settings" action opens
a duplicate of what is already on screen.

Observed visually: top of form shows `DIRECTORY` + `MODEL` pickers
filled in; bottom of form shows the same values again as
`[check ~/work/example-repo/] [claude-4-6]` chips.

## Acceptance Criteria

- [x] Desktop `/new` card shows directory and model exactly once.
- [x] Decide and document which surface wins: (a) keep the top pickers,
      drop the chips + expand-settings; (b) keep the compact chip row,
      collapse the top pickers behind it; (c) something else.
      **Decision: (a)** — kept the top `ConversationSettings` pickers,
      dropped the `new-conv-chips` row and the `showSettings`-gated
      expanded `SettingsFields` panel from the desktop branch.
- [x] Mobile layout (the `mobile-only` branch, line 168-193) is
      unaffected -- it does not have this duplication.
- [ ] Verified on desktop viewport that the `/new` page still exposes
      every field that `ConversationSettings` currently exposes
      (directory, model, show-all-models toggle, mode, branch pickers).
      (Code-level only: `ConversationSettings` props unchanged; visual
      verification deferred to QA-2.)

## Notes

- Low priority -- cosmetic/UX only, no functional bug.
- If (a) is chosen, the `showSettings` state and the expanded
  `SettingsFields` block can be removed entirely from the desktop
  branch.
