# Redesign mobile conversation work controls around the transcript

Implement the approved conversation-first mobile UX: replace the wrapped desktop Work Actions bar with one contextual persistent action and a single accessible work sheet containing PR selection, diffs, GitHub actions, associated PR history, and finish actions. Preserve desktop behavior and keep ConversationPrStatusHandle as the sole active-PR authority.

## Acceptance criteria

- At 390x844 the default mobile conversation leaves the transcript as the dominant region and renders at most one contextual work action above the composer.
- Ambiguous multi-PR state exposes an explicit Choose active PR action and never silently selects.
- The mobile sheet is viewport-safe, touch-safe, keyboard/screen-reader operable, and restores focus on close.
- Active/open PR feedback, mixed open/closed history, review actions, link-outs, and finish actions remain reachable in the sheet.
- Desktop retains the normative three-zone Work Actions bar and existing disposition behavior.
- Shared derived state prevents parallel mobile/desktop PR semantics.
- Specs, targeted tests, mobile QA fixtures, typecheck, lint, and full ./dev.py check pass.
