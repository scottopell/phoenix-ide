# Fix mobile conversation list storage footer scroll positioning

## Problem

On mobile, the conversation list page renders `StorageStatus` as a sibling after `ConversationList` inside `#main-area`. The footer shows conversation count and cached MB. Because the page scroll container and list section are not structurally owning that footer, it can appear to float in the background while scrolling the conversation list instead of staying visually attached to the bottom of the list area.

## Scope

Fix the mobile conversation list layout so the storage footer is positioned predictably at the bottom edge of the conversation-list scroll area and never visually drifts behind/through list content during scroll.

Likely files:

- `ui/src/pages/ConversationListPage.tsx`
- `ui/src/components/StorageStatus.tsx`
- `ui/src/components/StorageStatus.css`
- `ui/src/index.css`
- mobile conversation-list fixture/story and tests as needed

## Proposed approach

1. Reproduce with the mobile conversation list fixture or a narrow viewport on `/` with enough rows to scroll.
2. Inspect the actual scroll owner (`#main-area`) and the relationship between `ConversationList` and `StorageStatus`.
3. Make the footer a structurally owned part of the mobile list layout rather than an unrelated trailing sibling. Prefer a layout fix over z-index band-aids:
   - either render the footer in a dedicated list-page footer slot inside the list container, or
   - wrap `ConversationList` + `StorageStatus` in a mobile-only flex column whose scroll behavior makes the list content and footer one coherent scroll surface.
4. Ensure the footer background/border covers its own area and does not overlay rows unexpectedly.
5. Add or update a focused test/fixture coverage point for mobile list layout so regression is visible. If DOM-level tests cannot assert scroll behavior reliably, add/adjust the Ladle/mobile fixture and document manual QA steps.

## Acceptance criteria

- On mobile width, scrolling a long conversation list does not make the storage footer float over or behind conversation rows.
- The footer is visually attached to the bottom of the conversation list scroll area.
- The footer remains reachable and readable after scrolling to the end of a long list.
- Desktop conversation-list behavior is unchanged.
- Existing conversation list interactions still work: new conversation, archive toggle, row menu, chain expand/collapse, and storage status rendering.

## Verification

- Run targeted UI tests for conversation list/storage status if available.
- Run the mobile conversation list fixture/story at a narrow viewport and capture before/after behavior.
- Run the relevant `./dev.py check` lanes or full `./dev.py check` before landing.
