# Sync conversation nav strip with scroll position

## Problem

The conversation navigation bar at the top of the conversation view highlights the active chapter as the message list scrolls, but the horizontal pill strip itself does not move to keep that active pill visible. In long conversations the highlighted/current item can be off-screen, so the nav stops reflecting the reader’s position in a useful way.

## Proposed fix

- Extend the reusable `PillStrip` component with an opt-in `scrollActiveIntoView` behavior.
- When the active item changes, locate the active pill in the strip and horizontally scroll the nav container just enough to keep it visible, preferably centered/nearest without disturbing vertical page scroll.
- Enable this behavior for `ConversationNav`.
- Preserve existing `autoScrollToEnd` behavior for callers that use it; avoid making all pill strips auto-follow active state unless opted in.
- Add focused UI/component tests for the new behavior, including:
  - active pill already visible: no unnecessary scroll jump;
  - active pill outside the visible horizontal range: strip scrolls to reveal it;
  - no active item: no scroll.

## Acceptance criteria

- As the user scrolls through a conversation, the active chapter pill remains visible in the top conversation nav strip.
- Clicking a nav pill still jumps the message list to that chapter.
- Existing breadcrumb/pill-strip callers keep their current behavior unless they opt in.
- Relevant UI tests pass.
