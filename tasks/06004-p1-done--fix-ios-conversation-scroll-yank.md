# Fix iOS conversation scroll yank/state-machine loop

## Problem

Users sometimes get pulled away from their intended scroll position in the conversation message list. A concrete repro reported on iOS Safari/mobile:

1. Start at the bottom of a conversation.
2. Swipe once to scroll up.
3. Swipe a second time more slowly to slow/stop the existing momentum.
4. The conversation scroll behavior goes haywire, repeatedly dragging the user back toward the top (or otherwise fighting the user’s scroll position).

The likely code path is the manual auto-follow/settle state machine in:

- `ui/src/conversation/scrollMachine.ts`
- `ui/src/components/MessageList.tsx`
- `ui/src/components/MessageList.test.tsx`
- `ui/src/conversation/scrollMachine.test.ts`

Current coverage includes basic touch active suppression and short upward-momentum suppression, but it does not model the reported iOS pattern where a second touch is used to brake/redirect momentum and can produce a sparse/ambiguous sequence of `touchstart`, `touchend`, and `scroll` events. That gap can let later height changes or settle ticks classify the viewport as auto-followable even though the user still owns the scroll.

## Plan

1. **Reproduce as a failing test first**
   - Add reducer-level coverage for the iOS two-swipe/braking sequence:
     - pinned at bottom
     - first upward swipe/momentum begins
     - second touch occurs while still near the bottom threshold or while scroll direction events are sparse
     - tail/measurement height changes arrive after `touchend`
     - no `snapToLastIndex`, `writeDomBottom`, or repeated bottom-correction effects should fire while the user is trying to move/read upward
   - Add/extend a `MessageList` component test around the real event wiring (`touchstart`, `touchend`/`touchcancel`, `scroll`, `totalListHeightChanged`) so the DOM integration cannot regress independently of the pure reducer.

2. **Fix the scroll state machine structurally**
   - Treat touch gestures as viewport ownership, not only as an active-finger boolean.
   - Ensure a touch-start/touch-end braking gesture creates a suppression window even if iOS emits little or no upward `scrollTop` delta during the gesture.
   - Stop any bounded mount/settle rescue as soon as the user has touched the message scroller.
   - Keep the existing desirable behavior: if the user is genuinely pinned and new tail content arrives after user-owned suppression has expired, auto-follow still works.

3. **Review iOS-specific scroll ownership around the message scroller**
   - Confirm `#main-area.chat-main-area` remains non-scroll-owning and the Virtuoso scroller remains the only vertical chat scroller.
   - Check whether mobile Safari needs a scroller CSS guard such as `overscroll-behavior` / `-webkit-overflow-scrolling` on `.message-virtuoso` or its actual scroller element, without causing nested-scroll or rubber-band regressions.
   - Verify this does not conflict with `useIOSKeyboardFix`; only touch conversation-scroll behavior should change.

4. **Validate**
   - Run targeted tests:
     - `cd ui && pnpm test -- MessageList.test.tsx scrollMachine.test.ts`
   - Run project check as appropriate:
     - `./dev.py check`
   - Manually verify on mobile/iOS Safari if available:
     - bottom of a long conversation
     - first upward swipe
     - second slower braking swipe
     - ongoing streaming/tail growth while user is scrolled up
     - jump-to-newest button still works

## Acceptance criteria

- The reported two-swipe mobile sequence no longer fights the user or repeatedly yanks the message list.
- User scroll/touch ownership suppresses auto-follow/settle correction until the gesture/momentum has clearly ended.
- Auto-follow still works when the user is pinned at bottom and not actively trying to scroll away.
- Tests cover the reducer and the `MessageList` event wiring for the iOS-style gesture sequence.
- No regression to empty-conversation first-content snap, conversation-switch bottom placement, or jump-to-newest behavior.
