# Fix detached scrolling in the main conversation view

## Problem

After the latest deploy, the main conversation view can show a scrollbar that appears to move while the visible messages do not move. This matches a likely nested-scroll-container regression in the chat layout: `MessageList` renders Virtuoso’s own scroller inside `#main-area`, while `#main-area` also has `overflow: hidden auto` for other pages. If the outer container becomes scrollable in the current desktop/split-pane layout, wheel/trackpad input can target the wrong scroller and feel “detached” from the message content.

## Plan

1. Reproduce in the Phoenix UI with a long conversation on the latest build/dev server.
   - Inspect DOM scroll metrics for `#main-area`, `#messages` / Virtuoso scroller, and viewport wrappers before/after wheel events.
   - Confirm whether wheel input is changing the outer container while Virtuoso’s scrollTop stays fixed.
2. Make the conversation route structurally single-scroll-owner.
   - Keep Virtuoso as the only vertical scroll container for the chat message list.
   - Preserve the existing `#main-area` overflow behavior for the conversation-list/mobile surfaces that depend on it, using route/view-specific CSS rather than a globally ambiguous container rule.
   - Ensure flex wrappers in the desktop layout (`.desktop-main`, `#app`, `.conversation-column`, `#main-area`, `#chat-view`, `.message-virtuoso`) all have the height/min-height constraints Virtuoso needs.
3. Add regression coverage.
   - Extend `MessageList` / layout tests to assert the stamped `#messages` scroller is the intended scroll target.
   - Add a DOM/CSS-oriented regression test where feasible to prevent `#main-area` from being scrollable on the chat route when Virtuoso is mounted.
4. Validate manually and with checks.
   - Test desktop with and without file explorer/sidebar, terminal pane collapsed/expanded, and wide split-pane viewer open.
   - Run the relevant UI tests and project checks.

## Acceptance criteria

- In a long conversation, wheel/trackpad scrolling visibly moves messages immediately.
- The visible vertical scrollbar belongs to the same element whose content moves.
- `#main-area` does not compete with Virtuoso as a vertical scroll container on conversation pages.
- Conversation list/mobile scrolling behavior remains intact.
