# Rebuild conversation scrolling around durable follow intent and explicit gesture state

## Why this supersedes task 06005

Task 06005 correctly identifies that the scroll reducer is a flat mixture of geometry, gesture flags, timing heuristics, mount recovery, and product policy. A code/spec/history audit confirms that a substantial refactor is warranted, but two parts of its proposed design need correction before implementation:

1. `canAutoFollowTail = appFollowing && geometry.isPinned` is not generally valid. Tail growth can make the *current* viewport non-pinned before Phoenix receives the height-change callback. Auto-follow must preserve a durable pre-growth follow intent, not require post-growth pinned geometry.
2. `totalListHeightChanged` cannot simply be replaced by `atBottomStateChange`. The former is the notification that content or layout height changed and a follow action may be needed; the latter reports geometry. They have different responsibilities.

This task replaces task 06005 with an audited target architecture. Mark 06005 done or wont-do as superseded when this work lands.

## Audit findings

### Confirmed architectural problems

- `ScrollMachineState` independently stores `hasUserEngaged`, `touchActive`, `touchMovedAfterStart`, two suppression timestamps, measurement baselines, and a settle deadline. It permits contradictory or meaningless combinations and makes ownership a derived time-sensitive expression.
- User ownership expires after 400/1200 ms even if the user remains near the bottom reading. A later height delta can therefore reclaim the viewport without an explicit return-to-tail action.
- `atBottomChanged(true)` clears some suppression fields conditionally. Geometry events are therefore also hidden ownership transitions, which produced the iOS event-order churn.
- Mount rescue and normal follow share events/effects and ambient fields even though mount rescue is a bounded recovery protocol with different authority.
- Unread policy has two paths: reducer effects from height changes and a separate React effect driven by message/pending/stream signals. The task 06005 claim that tail growth can have one central outcome is not true until these paths are unified.
- Fast-check generates arbitrary flat states and arbitrary unsequenced events. This is useful as no-throw fuzzing, but it does not prove reachable-state invariants.
- Component tests repeat much of the reducer matrix through a mocked Virtuoso. They protect known regressions, but many are coupled to timestamps and internal callbacks rather than the adapter contract.

### Claims that remain valid

- Browser/Virtuoso must own physical scrolling, measurement, momentum, and anchoring; Phoenix owns only product policy.
- `atBottom=true` is geometry, not unconditional permission to move the viewport.
- Tap-only touches must not disable pinned follow.
- A moved/braking gesture must block follow even when iOS emits sparse or misleading scroll/bottom events.
- Mount stranding is evidenced by regression tests and history, including silent stranding with no final height/scroll event. Do not remove bounded polling without reproducing and replacing that recovery behavior in a real browser.
- `followOutput={false}` must remain unless real-browser validation proves the relevant react-virtuoso version no longer has the size-increase/user-scroll misclassification documented by the streaming-scroll regression. Running Virtuoso follow and Phoenix follow simultaneously is forbidden.
- `totalListHeightChanged` remains useful as a height-change notification. It must cease being an independent ownership authority.

## Product invariants

1. Opening or switching to a populated conversation converges to the newest content unless the user interacts first.
2. While follow intent belongs to Phoenix, any list-height change preserves the bottom, including streaming growth, late tail measurement, and viewport shrink.
3. Upward user intent transfers the viewport to the user immediately, including wheel, touch movement, scrollbar/keyboard/find-in-page scroll, and conversation-navigation jumps.
4. User ownership is durable. Time passing never returns ownership to Phoenix.
5. User ownership is released only by an explicit return to the tail: jump-to-newest, or a bottom confirmation received when no moved gesture is active.
6. A tap-only touch does not transfer ownership. A moved touch blocks follow while active even before a scroll event arrives.
7. A bottom callback during an active moved touch cannot release ownership. The gesture adapter must retain whether the gesture departed the bottom so touch end cannot reinterpret a stale bottom callback as permission.
8. Tail content advancing while user-owned shows unread and never scrolls. Unrelated layout growth while user-owned neither scrolls nor creates unread.
9. Mount rescue cannot write after the first user interaction and cannot restart for that mounted conversation.
10. Conversation identity change resets geometry baselines, gesture state, follow intent, unread state, and mount-rescue eligibility atomically.
11. One transition emits at most one visible scroll command and cannot both show and clear unread.
12. Phoenix never attempts to model momentum duration or native scroll physics.

## Target state model

Use discriminated unions so only reachable lifecycle and gesture combinations can be constructed. The exact names may vary, but preserve these semantic boundaries:

```ts
type Session =
  | { kind: 'unmeasured'; conversationId?: string }
  | {
      kind: 'mount-rescue';
      conversationId?: string;
      deadlineMs: number;
      geometry: Geometry;
      gesture: Gesture;
      unread: boolean;
    }
  | {
      kind: 'live';
      conversationId?: string;
      follow: FollowMode;
      geometry: Geometry;
      gesture: Gesture;
      unread: boolean;
    };

type FollowMode =
  | { kind: 'following' }
  | { kind: 'reading' }
  | { kind: 'returning-to-tail' };

type Gesture =
  | { kind: 'idle' }
  | {
      kind: 'touch';
      moved: boolean;
      departedBottom: boolean;
      modeBeforeGesture: FollowMode;
    };

type Geometry = {
  atBottom: boolean;
  lastSnapshot: ScrollSnapshot | null;
  previousTotalHeight: number;
  previousScrollHeight: number;
  previousClientHeight: number;
};
```

Important interpretation:

- `following` is durable intent to preserve the tail. It may temporarily coexist with `atBottom=false` after content growth and before the follow effect executes.
- `reading` means Phoenix cannot move the live viewport.
- `returning-to-tail` represents an explicit jump request until bottom is confirmed; it prevents a failed/in-flight jump from being confused with ordinary reading.
- Mount rescue is a lifecycle mode, not a gesture timestamp. First interaction exits it permanently into live policy.
- If implementation evidence shows `departedBottom` needs a richer shape, keep it inside the touch variant rather than adding ambient booleans.

## Event and responsibility boundaries

### MessageList adapter

Translate raw browser/Virtuoso/React signals into policy events:

- `conversationMeasured`
- `viewportPinnedChanged`
- `touchStarted`, `touchMoved`, `touchEnded`
- `upwardIntent` / `downwardMovement` with a source where useful
- `navigationJumped`
- `tailContentAdvanced` for message append, pending append, and stream start
- `heightChanged` with whether active tail rendering could have caused the growth
- `settleProbe`
- `jumpToNewestRequested`

Do not pass timestamps for normal live ownership. The 400 ms and 1200 ms suppression constants should disappear. Time remains only in bounded mount rescue.

### Geometry sources

- Keep `atBottomStateChange` as the normal bottom/return-to-tail geometry signal.
- Keep `totalListHeightChanged` as the signal that height changed and following may require a scroll effect.
- Do not infer user ownership from height growth.
- Preserve pre-growth DOM baselines only where needed to classify resize/measurement behavior or validate callback ordering. Remove the old pin-threshold permission branch once durable follow intent makes it redundant.
- Reserve direct `scrollTop = scrollHeight` for mount rescue. Normal live follow uses Virtuoso `scrollToIndex`.

### Unread ownership

Move unread truth into the policy state. Route the existing message-length, pending-length, stream-start, and active-tail height signals through the reducer. The component may render a projection/effect of that state, but it must not maintain a second independent policy that can disagree.

Centralize outcomes:

- tail advance + following/returning: preserve/follow tail, unread false;
- tail advance + reading or active moved gesture: no scroll, unread true;
- unrelated height change + following: preserve tail, unread unchanged;
- unrelated height change + reading: no scroll, unread unchanged.

## Implementation sequence

1. **Make the normative behavior timeless and explicit**
   - Rewrite `scroll_policy.allium` around lifecycle, follow mode, geometry, gesture, and unread invariants rather than code field names and suppression timestamps.
   - Update REQ-MLRU-014/015 to distinguish durable follow intent, Virtuoso bottom geometry, height-change notification, and bounded mount rescue.
   - Remove stale task references and time-relative implementation prose from requirements while touching it; put implementation status in `executive.md`.
   - Run the `specs/AUTHORING.md` pre-flight and `allium check`.

2. **Refactor the pure reducer in one coherent change**
   - Introduce the discriminated state model and normalized events.
   - Delete `USER_SCROLL_SUPPRESS_MS`, `TOUCH_GESTURE_SUPPRESS_MS`, `hasUserEngaged`, `touchActive`, `touchMovedAfterStart`, `lastUpwardScrollAt`, and `lastTouchGestureAt` as ambient fields.
   - Retain time only inside `mount-rescue`.
   - Add transition helpers for `takeUserOwnership`, `requestTailReturn`, `confirmTailReturn`, `advanceTail`, and `exitMountRescue` so unread and scrolling cannot diverge across branches.

3. **Rewire MessageList as an adapter/effect interpreter**
   - Keep passive DOM listeners and Virtuoso callbacks, but dispatch semantic events.
   - Ensure touchcancel has an explicit semantic outcome rather than being an accidental alias with unclear remaining-touch behavior.
   - Route all unread-producing signals through the machine.
   - Keep `followOutput={false}` and one Phoenix auto-follow path.
   - Keep the bounded settle watch initially, isolated to `mount-rescue`; stop and cancel it synchronously on lifecycle exit. Simplify its rAF/interval effects only if silent-stranding behavior remains proven.

4. **Rebuild tests around reachable histories**
   - Write reducer table tests for every product invariant and key callback ordering.
   - Generate fast-check histories from valid commands, replaying events from `initialScrollMachineState`, instead of generating arbitrary internal states. Retain one small arbitrary-input robustness property only if useful.
   - Assert state-union invariants, durable user ownership without elapsed-time release, mount-rescue irreversibility, and effect exclusivity.
   - Reduce component tests to adapter/effect anchors; retain the known iOS braking, first-non-empty, silent-stranding, viewport-shrink, conversation-switch, and unread regressions.

5. **Validate with the real library and real browsers**
   - Add or use a deterministic long/streaming conversation fixture; mocked Virtuoso cannot establish callback order or native gesture behavior.
   - Desktop: pinned streaming, wheel/keyboard/find scroll-up, return to bottom, jump-to-newest, viewport shrink, and conversation switch.
   - Mobile Safari/iOS: swipe up, second slow braking swipe during growth, tap-only while following, moved touch with sparse scroll events, and return to tail.
   - Capture callback/event traces during QA if behavior differs from assumptions; fix the event model rather than adding a release timeout.

## Required reducer scenarios

- following + height growth => one follow effect, no unread;
- following + upward wheel/scroll => reading, and later growth never follows regardless of elapsed time;
- reading + bottom confirmation while idle => following and clear unread;
- reading + bottom callback during moved touch => remain blocked;
- tap-only touch restores the pre-gesture mode;
- moved touch with no scroll event blocks follow;
- moved braking touch with stale `atBottom=true` cannot release reading;
- jump-to-newest => returning-to-tail + one snap, then bottom confirmation => following;
- navigation jump => reading and mount rescue disabled;
- tail advance while reading => unread without scroll;
- unrelated layout growth while reading => neither unread nor scroll;
- first content after empty mount enters rescue and converges to bottom;
- any user interaction permanently exits rescue for that mounted conversation;
- conversation switch constructs a fresh session with no stale unread/gesture/geometry;
- no transition emits conflicting scroll or unread effects.

## Validation

```bash
allium check specs/messagelist-render-units/scroll_policy.allium
cd ui && pnpm exec vitest run src/conversation/scrollMachine.test.ts src/components/MessageList.test.tsx
./dev.py check
```

Run the real-browser scenarios above before handoff. Treat mobile Safari verification as required acceptance evidence, not optional polish.

## Acceptance criteria

- Live user ownership has no time-based expiry; only explicit return-to-tail signals release it.
- The reducer uses discriminated lifecycle/follow/gesture state and cannot represent the old contradictory boolean/timestamp combinations.
- `atBottomStateChange`, `totalListHeightChanged`, and mount DOM writes each have one documented, non-overlapping responsibility.
- All unread decisions flow through one policy state.
- Mount rescue remains bounded, cannot restart after interaction, and is the only policy allowed to write the DOM bottom directly.
- `followOutput` remains disabled unless replacement is supported by captured real-browser evidence and all custom normal-follow behavior is removed in the same change.
- Fast-check exercises reachable event histories.
- Known iOS, desktop, mount-stranding, resize, empty-to-content, and conversation-switch regressions pass.
- The Allium spec and REQ-MLRU-014/015 describe the final behavior without task/PR/status-relative prose.
- Task 06005 is closed as superseded.
