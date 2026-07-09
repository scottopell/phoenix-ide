# Refactor conversation scroll policy around explicit viewport ownership

## Context

PR #458 (`task-06004-fix-ios-conversation-scroll-yank`) is a tactical fix for an iOS/mobile Safari conversation scroll bug. The reported repro was:

1. Start at the bottom of a long/streaming conversation on mobile Safari.
2. Swipe once to scroll up.
3. Swipe a second time more slowly to brake/slow the native momentum.
4. The message list may fight the user, repeatedly snapping/yanking instead of letting them read.

That PR moved the immediate fix from broad timestamp heuristics to a more intent-based touch policy:

- tap-only touches do **not** suppress pinned auto-follow;
- moved touches (`touchmove`) create a short post-touch suppression window;
- `atBottom=true` clears stale suppression only when no touch is active;
- if `atBottom=true` fires during an active moved touch, touch suppression is preserved until `touchend`;
- `specs/messagelist-render-units/scroll_policy.allium` was updated to match the tactical behavior.

The PR passed targeted tests, `allium check`, and `./dev.py check`, but review churn revealed a deeper design problem: the scroll reducer is still a flat bag of geometry facts, gesture flags, timestamps, mount-settle state, and product-policy decisions. Each edge-case fix risks creating another edge case.

This task is the follow-up architectural cleanup. Do **not** simply add another iOS timing heuristic unless you first show why the explicit ownership model below cannot cover the case.

## Relevant code/spec entry points

- `ui/src/conversation/scrollMachine.ts`
- `ui/src/conversation/scrollMachine.test.ts`
- `ui/src/components/MessageList.tsx`
- `ui/src/components/MessageList.test.tsx`
- `specs/messagelist-render-units/scroll_policy.allium`
- `specs/messagelist-render-units/requirements.md`
- `specs/messagelist-render-units/executive.md`

The message list uses `react-virtuoso`. The scroll policy is shared by desktop and mobile.

Important event/effect concepts currently involved:

- browser/native events: `scroll`, `wheel`, `touchstart`, `touchmove`, `touchend`, `touchcancel`, `pointerdown`;
- Virtuoso events: `atBottomStateChange`, `totalListHeightChanged`;
- policy effects: `snapToLastIndex`, `writeDomBottom`, `showUnread`, `clearUnread`, `startSettleWatch`, `stopSettleWatch`;
- state currently includes fields such as `hasUserEngaged`, `touchActive`, `touchMovedAfterStart`, `lastUpwardScrollAt`, `lastTouchGestureAt`, and `settleDeadline`.

## Problem to solve

The reducer currently mixes four different concepts:

1. **Geometry** — where the viewport appears to be (`atBottom`, scrollTop/clientHeight/scrollHeight, old distance from bottom).
2. **User intent / viewport ownership** — whether the app or the user owns the viewport.
3. **Native gesture lifecycle** — touch/wheel/scroll/momentum/braking details, especially on iOS Safari.
4. **Mount/measurement recovery** — initial conversation load or virtualization measurement can strand the viewport away from bottom.

Because these concepts are not structurally separated, the code repeatedly has to answer questions like:

- Does `atBottom=true` mean it is safe to snap, or is the user still touching/braking?
- Does a touch count as user intent if it did not move?
- Should stale upward-scroll timestamps affect a later pinned tap?
- Should the mount settle watchdog be allowed to write the DOM after the user interacts?
- Should `totalListHeightChanged` compute pinnedness separately from Virtuoso’s `atBottomStateChange`?

The desired cleanup is to make invalid combinations harder to represent and make policy decisions flow from explicit viewport ownership rather than scattered timestamp/boolean checks.

## Design direction

Refactor the scroll policy around an explicit ownership model.

Recommended starting point:

```ts
type ViewportOwnership =
  | { kind: 'mount-settling'; deadlineMs: number }
  | { kind: 'app-following' }
  | { kind: 'user-owned'; reason: 'scroll' | 'touch' | 'navigation'; releaseAfterMs?: number };
```

The exact shape may differ, but it should make the key distinction structural:

- **Geometry** answers: “is the viewport near/pinned to the bottom?”
- **Ownership** answers: “is Phoenix allowed to move the viewport?”

Policy should be framed around this invariant:

```ts
canAutoFollowTail = ownership.kind === 'app-following' && geometry.isPinned;
```

Not around a flat expression such as:

```ts
!touchActive && now - lastUpwardScrollAt >= N && now - lastTouchGestureAt >= M
```

### Target semantics

Preserve these current user-visible semantics:

- Opening/switching to a conversation with content should land at/near the bottom unless the user has already interacted.
- While genuinely pinned and app-owned, active tail growth should auto-follow newest content.
- If tail content grows while the user owns the viewport, do not snap; show the jump-to-newest/unread affordance.
- Tap-only touch on the message list while pinned should **not** disable auto-follow or create false unread state.
- Any gesture that visibly moves/manipulates scrollback should transfer ownership to the user.
- A mobile/iOS braking gesture must not be treated as permission to auto-follow just because Virtuoso reports `atBottom=true` or no fresh upward `scroll` event has arrived.
- `atBottom=true` is a geometry signal, not by itself permission to snap. User ownership dominates pinned geometry until explicitly released.
- Returning to newest/bottom after the gesture is done should be the path back to `app-following`.
- Conversation identity changes reset all ownership/suppression/mount baselines for the new conversation.
- Mount/settle rescue must stop permanently once the user interacts.

### Recommended structural split

Consider separating state into two submodels:

```ts
type ScrollGeometry = {
  isPinned: boolean;
  lastSnapshot: ScrollSnapshot | null;
  previousMeasuredHeight: number;
};

type MountRescue =
  | { kind: 'eligible'; deadlineMs: number }
  | { kind: 'disabled' };

type ScrollPolicyState = {
  conversationId?: string;
  ownership: ViewportOwnership;
  geometry: ScrollGeometry;
  mountRescue: MountRescue;
  hasSeenContent: boolean;
};
```

This is a sketch, not a mandate. The important thing is that “mount settling”, “user-owned”, and “app-following” should not be encoded as independent booleans/timestamps that can contradict each other.

### Event vocabulary cleanup

The reducer currently receives low-level events. Consider introducing adapter-level/intention-level events before the reducer, e.g.:

- `viewportPinnedChanged(isPinned)`
- `userGestureStarted(kind)`
- `userGestureMoved(kind)`
- `userGestureEnded({ kind, moved })`
- `userScrolled({ direction, snapshot })`
- `tailHeightChanged({ tailActivity, snapshot })`
- `conversationMeasured(...)`
- `settleTick(...)`

This can be done incrementally: keep DOM/Virtuoso listeners in `MessageList`, but translate raw browser events into clearer policy events before dispatching to the reducer.

### Virtuoso / DOM responsibility boundary

Investigate and document the responsibility split with Virtuoso:

- Prefer Virtuoso’s `atBottomStateChange` as the normal geometry source for “currently pinned” if feasible.
- Avoid using `totalListHeightChanged` as a second independent permission-to-snap detector for normal live-follow.
- Reserve raw DOM distance checks and direct `scrollTop`/bottom writes for narrow mount/measurement rescue cases.
- Evaluate whether any `followOutput` or Virtuoso-native behavior can replace Phoenix-owned normal follow behavior. If not, record why in the task/PR summary and ensure the custom behavior is limited to policy, not physics.

Do **not** attempt to reimplement native scroll physics or iOS momentum. Browser/Virtuoso own physical scrolling; Phoenix owns product policy.

## Suggested implementation plan

1. **Distill current behavior into explicit invariants**
   - Update `scroll_policy.allium` first or alongside code.
   - Make the spec distinguish geometry from ownership.
   - Capture invariants such as:
     - pinned geometry is not permission to snap while user-owned;
     - tap-only gestures cannot enter user-owned suppression;
     - moved gestures do enter user ownership;
     - at-bottom during active gesture cannot release ownership;
     - tail growth while user-owned creates unread instead of snap;
     - mount rescue cannot run after user interaction.

2. **Refactor reducer state shape**
   - Introduce explicit ownership and mount-rescue variants.
   - Remove or encapsulate loose fields like `hasUserEngaged`, `lastUpwardScrollAt`, `lastTouchGestureAt`, `touchActive`, `touchMovedAfterStart` where possible.
   - If a timestamp remains necessary, it should live inside the state variant it belongs to, e.g. `{ kind: 'user-owned', releaseAfterMs }`, not as global ambient memory.

3. **Normalize events at the boundary**
   - Keep browser/Virtuoso listeners in `MessageList`, but dispatch policy-level events where practical.
   - A tap-only touch should have a distinct outcome from a moved gesture.
   - A moved touch should not require a corresponding upward `scroll` event to protect against iOS sparse-event braking.

4. **Simplify tail-growth outcome logic**
   - Prefer one central branch:
     - if `canAutoFollowTail`, snap/follow and clear unread;
     - otherwise show unread.
   - Avoid parallel branches where snap suppression and unread marking can diverge.

5. **Separate mount rescue from live follow**
   - Keep one-shot/short-window recovery only for initial measurement stranding.
   - Disable rescue on first user ownership transition.
   - Ensure settle ticks cannot write the DOM while user-owned.

6. **Reassess duplicate pinnedness calculations**
   - Decide whether Virtuoso `atBottomStateChange` or DOM distance is authoritative for normal live-follow geometry.
   - If DOM distance remains necessary, document why and constrain its use.

## Testing expectations

Add/reshape tests around invariants rather than only scenario regressions.

Reducer-level coverage should include:

- app-following + pinned + active tail growth => snap/follow, no unread;
- user-owned + active tail growth => no snap, unread;
- tap-only touch while pinned + active tail growth => snap/follow, no unread;
- moved touch with no fresh upward scroll event + active tail growth => no snap, unread;
- `atBottom=true` during active moved touch does not release user ownership;
- gesture ended + explicit return to bottom releases user ownership and resumes follow;
- conversation switch resets ownership, geometry baseline, unread/suppression state;
- settle tick may write bottom only during mount-settling and never after user ownership;
- no event can emit both visible scroll effects (`snapToLastIndex` and `writeDomBottom`) in one transition;
- unread and clear-unread effects do not conflict in one transition.

Component-level coverage should include real wiring for:

- touchstart/touchmove/touchend/touchcancel;
- Virtuoso `atBottomStateChange` arriving before `touchend`;
- `totalListHeightChanged` during active tail growth;
- jump-to-newest returning ownership to app-following;
- desktop wheel/upward scroll path still suppresses follow;
- downward return-to-bottom path resumes follow.

Manual QA target cases:

- iOS Safari/mobile: bottom of long conversation, swipe up, second slow braking swipe, streaming/tail growth continues — no yank/fight.
- iOS Safari/mobile: tap message list while pinned during streaming — remains following, no false unread.
- iOS Safari/mobile: swipe up/read old output during streaming — unread/jump-to-newest appears and no snap.
- Desktop: wheel upward during streaming — no snap; jump-to-newest works.
- Conversation switch/open long conversation — lands at bottom without later fighting user input.

## Validation commands

At minimum:

```bash
allium check specs/messagelist-render-units/scroll_policy.allium
cd ui && pnpm exec vitest run src/components/MessageList.test.tsx src/conversation/scrollMachine.test.ts
./dev.py check
```

If the implementation changes generated types or broader UI behavior, run the additional relevant checks before handoff.

## Handoff notes from PR #458 review/panel

A persona panel was run after PR #458. Consensus:

- Phoenix should not try to own iOS/native scroll physics.
- Browser/Virtuoso own physical scrolling and measurement; Phoenix owns product policy.
- The policy should be “who owns the viewport?” rather than “which recent timestamp suppresses snapping?”
- `atBottom=true` is geometry, not permission.
- User intent/ownership must dominate geometry until explicitly released.
- The mount/settle rescue and live auto-follow are different problems and should be separated.
- `totalListHeightChanged` should not remain a second independent pin-authority for normal follow if Virtuoso bottom state can be used instead.

Useful summary from the panel:

> The next move that most improves both simplicity and correctness is to refactor the scroll machine around explicit viewport ownership, where user-owned/following/mount-settling are structural states, and make `atBottom` merely a geometry fact.

## Non-goals

- Do not add more one-off iOS timing heuristics as the primary solution.
- Do not attempt to emulate iOS momentum, rubber-band, or native scroll physics.
- Do not remove mount/settle recovery unless you first verify the stranded-conversation case is no longer real or is covered by a simpler mechanism.
- Do not regress desktop wheel behavior, pinned live-follow, jump-to-newest, or conversation-switch bottom placement.

## Acceptance criteria

- The reducer state structurally separates viewport ownership from bottom geometry and mount rescue.
- Tail growth has one clear policy outcome: follow when app-owned+pinned, otherwise show unread.
- Tap-only, moved-touch, wheel/upward-scroll, active-touch repin, and return-to-bottom semantics are covered by invariant-style reducer tests.
- `MessageList` integration tests cover the browser/Virtuoso event-order cases that caused PR #458 review churn.
- The Allium scroll policy is updated and validates.
- Targeted tests and `./dev.py check` pass.
- The implementation is simpler to reason about than the current timestamp/boolean bag; the PR summary must explicitly describe which old state fields/branches were removed or encapsulated.
