# Refactor MessageList scroll behavior around a deterministic state machine

## Context

The conversation message-list scroll behavior has accumulated several generations of fixes:

- Task 60410 / commit `3145d0c4`: replaced hand-rolled spacer/windowing with `react-virtuoso` because browser-native `overflow-anchor` is not viable for Phoenix’s target matrix: Chrome and Safari, desktop and mobile, including iOS Safari.
- Task 65004 / PR #409 / commit `e6384f7f`: disabled Virtuoso `followOutput` because its built-in size-increase handling fought user scroll-up during streaming; Phoenix now owns auto-follow through `totalListHeightChanged`.
- PR #417 / commit `4ff1d78e` cluster: added DOM-accurate pin detection, gesture suppression, and bounded self-healing for stranded mounts.
- Existing tests in `MessageList.test.tsx` cover many edge cases, but mostly by driving a component mock and asserting imperative calls. The policy is still distributed across refs, callbacks, timers, `Date.now()`, and DOM reads/writes inside `MessageList.tsx`.

There is also a small spec drift to clean up while touching this area: `specs/messagelist-render-units/executive.md` still describes `followOutput="auto"` / “pure followOutput semantics”, while requirements/design/code now use `followOutput={false}` and manual `totalListHeightChanged` auto-follow.

## Goal

Make scroll policy deterministic and inspectable by extracting a small pure state machine/reducer that decides **what should happen** from an explicit snapshot of scroll facts and semantic events. Keep browser/Virtuoso interactions as an adapter layer.

This is not a request to abandon Virtuoso. Virtuoso still owns virtualization, measuring, and anchor compensation. Phoenix owns only the product-level auto-follow policy that Virtuoso cannot express correctly for streaming height growth across Chrome/Safari/iOS.

## Desired invariants

The extracted model should make these invariants explicit and testable:

1. **Bottom-pinned users keep seeing the tail.** If the user was pinned before tail content growth or viewport shrink, the next effect should re-snap to bottom.
2. **Non-pinned users are never yanked.** If the user has intentionally left the bottom, incoming content of any type (including system messages and approval prompts) must not auto-scroll; the jump-to-newest affordance is the only return path.
3. **Active upward gestures own the viewport.** Touch drag, wheel-up, scrollbar/keyboard/find-in-page upward movement within the suppression window must suppress auto-follow even if the previous distance is within the pin threshold.
4. **Downward motion does not suppress follow.** Programmatic snaps and users heading toward the bottom should not poison the pinned state.
5. **Mount opens at newest unless the user takes over.** Before user engagement, a bounded settle phase may correct stranded Virtuoso initial placement using DOM-bottom assignment; after the bounded window, scroll-only user movement must not be fought.
6. **First content after an empty mount snaps to newest.** `initialTopMostItemIndex` only controls mount; first non-empty content after an empty mount requires an explicit snap.
7. **Pin distance is computed in DOM units.** The policy uses previous DOM `scrollHeight`, current `scrollTop`, and the appropriate pre-shrink viewport height, not Virtuoso’s estimated total height.
8. **Unread signal is not swallowed.** If a snap is suppressed while genuine tail activity grows, the model must request/show unread/jump-to-newest state.
9. **Conversation identity resets policy state.** Fresh conversation instance means fresh baseline, no stale scroll or measurement state crossing conversations.
10. **Render-unit keys remain structural.** Pending→sent and streaming→finalized transitions should remain in-place keyed updates; the scroll model should not introduce parallel message status state.

## Web compatibility constraints

The refactor must preserve the hard-won compatibility assumptions:

- Do not rely on CSS `overflow-anchor`; Safari/iOS Safari support remains insufficient for Phoenix’s target platforms.
- Do not delegate product auto-follow back to Virtuoso `followOutput="auto"` unless a targeted investigation proves the current react-virtuoso version changed the size-increase/user-scroll classification behavior across Chrome, Safari, and iOS Safari.
- Treat DOM scroll values as the compatibility boundary. Browser engines and virtualizer model heights disagree during measurement churn, especially on long variable-height conversations.
- Preserve touch-specific behavior: iOS momentum scrolling can continue after finger lift, so suppression cannot be tied only to `touchstart`/`touchend`.
- Keep all imperative DOM writes in the adapter layer; the pure model should emit effects like `snapToBottom`, `writeDomBottom`, `startSettleWatch`, `showUnread`, not call browser APIs directly.

## Proposed architecture

Introduce a pure module, e.g. `ui/src/conversation/scrollMachine.ts`, with typed events, state, snapshots, and effects.

Possible shape:

```ts
type ScrollPhase =
  | { kind: 'settling'; deadlineMs: number; hasSeenContent: boolean }
  | { kind: 'engaged'; hasSeenContent: boolean };

type ScrollEvent =
  | { type: 'conversationMeasured'; conversationId: string; totalHeight: number; unitCount: number; snapshot: ScrollSnapshot; nowMs: number }
  | { type: 'heightChanged'; totalHeight: number; unitCount: number; snapshot: ScrollSnapshot; tailActivity: TailActivity; nowMs: number }
  | { type: 'scroll'; snapshot: ScrollSnapshot; nowMs: number }
  | { type: 'touchStart'; nowMs: number }
  | { type: 'touchEnd'; nowMs: number }
  | { type: 'wheel'; deltaY: number; nowMs: number }
  | { type: 'pointerDown'; nowMs: number }
  | { type: 'navJump'; nowMs: number }
  | { type: 'settleTick'; snapshot: ScrollSnapshot; nowMs: number }
  | { type: 'jumpToNewestClicked'; unitCount: number; nowMs: number };

type ScrollEffect =
  | { type: 'snapToLastIndex' }
  | { type: 'writeDomBottom' }
  | { type: 'startSettleWatch'; deadlineMs: number }
  | { type: 'stopSettleWatch' }
  | { type: 'showUnread' }
  | { type: 'clearUnread' };
```

Exact names are flexible. The important property is that policy decisions become pure and exhaustive, while `MessageList.tsx` becomes a thin adapter from Virtuoso/DOM events to model events and model effects to DOM/Virtuoso calls.

## Implementation plan

1. Audit current `MessageList.tsx` scroll refs/callbacks and map each branch to a named model event/effect.
2. Add `scrollMachine.ts` with pure reducer/helpers and no React/browser dependencies.
3. Port the existing `handleTotalListHeightChanged` tests into pure reducer tests first:
   - pinned height growth re-snaps;
   - scrolled-up height growth does not snap;
   - first non-empty update snaps;
   - conversation switch seeds baseline without stale snap;
   - stranded mount schedules/writes DOM-bottom during bounded settle;
   - settle window stops after engagement;
   - scroll-only input is not fought after settle window;
   - touch/momentum suppression works;
   - DOM scrollHeight beats virtualizer model height;
   - viewport shrink while pinned re-snaps;
   - suppressed tail growth marks unread.
4. Adapt `MessageList.tsx` to hold machine state in a ref/reducer and dispatch events from current Virtuoso callbacks/listeners.
5. Keep existing component tests as integration coverage, but reduce direct branch complexity in the component after pure tests are in place.
6. Update `specs/messagelist-render-units/executive.md` to match current code/spec reality (`followOutput={false}`, manual auto-follow as sole policy owner).
7. Run focused UI tests for `MessageList`, then `./dev.py check` if feasible.

## Acceptance criteria

- Scroll policy decisions are covered by deterministic pure tests that do not depend on happy-dom measurement behavior, real timers, or React/Virtuoso mocks except where explicitly testing the adapter.
- `MessageList.tsx` no longer contains the full policy as distributed ad hoc refs/branches; it dispatches model events and interprets model effects.
- Current behavior is preserved for the known regression set from tasks 60410, 65004, and PR #417.
- Requirements/design/executive docs agree on the current `followOutput={false}` + manual auto-follow architecture.
- No new reliance on browser features unsupported by Safari/iOS Safari.

## Out of scope

- Replacing `react-virtuoso`.
- Reintroducing saved scroll restoration.
- Changing the product decision that system messages do not force-scroll non-pinned users.
- Adding a separate browser-specific scroll implementation for mobile.
- Solving every remaining visual edge case in one pass; the goal is to make future edge cases modelable and testable.
