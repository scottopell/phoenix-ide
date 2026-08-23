import { describe, expect, it } from 'vitest';
import * as fc from 'fast-check';
import {
  SETTLE_WATCH_MS,
  initialScrollMachineState,
  reduceScrollMachine,
  snapshotIsPinned,
  type ScrollEffect,
  type ScrollEvent,
  type ScrollMachineState,
  type ScrollSnapshot,
} from './scrollMachine';

const snap = (scrollHeight: number, scrollTop: number, clientHeight: number): ScrollSnapshot => ({
  scrollHeight,
  scrollTop,
  clientHeight,
});

function measured(
  unitCount = 5,
  snapshot: ScrollSnapshot | null = snap(1_000, 600, 400),
  conversationId = 'conv',
) {
  return reduceScrollMachine(initialScrollMachineState(conversationId), {
    type: 'conversationMeasured',
    conversationId,
    totalHeight: snapshot?.scrollHeight ?? 0,
    unitCount,
    snapshot,
    nowMs: 1_000,
  });
}

function liveFollowing(): Extract<ScrollMachineState, { kind: 'live' }> {
  const mounted = measured();
  expect(mounted.state.kind).toBe('mount-rescue');
  const result = reduceScrollMachine(mounted.state, { type: 'interactionStarted' });
  expect(result.state.kind).toBe('live');
  return result.state as Extract<ScrollMachineState, { kind: 'live' }>;
}

// Reading state whose geometry sits far above the pin-to-bottom zone; the
// off-bottom snapshot matters because gesture ends and pinned downward
// movement release reading ownership when geometry is at the bottom.
function reading(): Extract<ScrollMachineState, { kind: 'live' }> {
  return reduceScrollMachine(liveFollowing(), {
    type: 'upwardIntent',
    snapshot: snap(1_000, 100, 400),
  }).state as Extract<ScrollMachineState, { kind: 'live' }>;
}

const effectTypes = (effects: ScrollEffect[]) => effects.map((effect) => effect.type);

function expectLiveMode(state: ScrollMachineState, mode: 'following' | 'reading' | 'navigating' | 'returning-to-tail') {
  expect(state.kind).toBe('live');
  if (state.kind === 'live') expect(state.follow.kind).toBe(mode);
}

describe('scrollMachine durable follow policy', () => {
  it('follows every height change while follow intent belongs to Phoenix', () => {
    const result = reduceScrollMachine(liveFollowing(), {
      type: 'heightChanged',
      totalHeight: 1_100,
      unitCount: 5,
      snapshot: snap(1_100, 600, 400),
      tailActivity: 'none',
    });

    expect(result.effects).toEqual([{ type: 'snapToLastIndex' }]);
    expect(result.state.kind === 'live' && result.state.unread).toBe(false);
  });

  it('keeps upward ownership durable across arbitrary time and later growth', () => {
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), {
      type: 'upwardIntent',
      snapshot: snap(1_000, 590, 400),
    }).state;
    expectLiveMode(state, 'reading');

    state = reduceScrollMachine(state, {
      type: 'settleProbe',
      snapshot: snap(1_000, 590, 400),
      nowMs: Number.MAX_SAFE_INTEGER,
    }).state;
    const result = reduceScrollMachine(state, {
      type: 'heightChanged',
      totalHeight: 1_100,
      unitCount: 5,
      snapshot: snap(1_100, 590, 400),
      tailActivity: 'active',
    });

    expectLiveMode(result.state, 'reading');
    expect(result.effects).toEqual([{ type: 'showUnread' }]);
  });

  it('bottom confirmation while idle returns reading to following and clears unread', () => {
    const state = reduceScrollMachine(reading(), { type: 'tailContentAdvanced' }).state;
    const result = reduceScrollMachine(state, { type: 'viewportPinnedChanged', atBottom: true });

    expectLiveMode(result.state, 'following');
    expect(result.effects).toEqual([{ type: 'clearUnread' }]);
  });

  it('a bottom callback during a moved touch cannot release ownership mid-gesture', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), {
      type: 'viewportPinnedChanged',
      atBottom: false,
    }).state;
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, { type: 'viewportPinnedChanged', atBottom: true }).state;
    expectLiveMode(state, 'reading');
    expect(state.kind === 'live' && state.gesture.kind === 'touch' && state.gesture.departedBottom).toBe(true);
  });

  it('a moved touch ending at the bottom confirms tail return and clears unread', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'tailContentAdvanced' }).state;
    expect(state.kind === 'live' && state.unread).toBe(true);
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    // The drag carries the viewport back to the tail. A pinned callback may
    // also fire and be blocked mid-gesture; the arrival is legible from the
    // movement either way.
    state = reduceScrollMachine(state, {
      type: 'downwardMovement',
      snapshot: snap(1_000, 600, 400),
    }).state;
    state = reduceScrollMachine(state, { type: 'viewportPinnedChanged', atBottom: true }).state;
    expectLiveMode(state, 'reading');

    const result = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 });
    expectLiveMode(result.state, 'following');
    expect(result.state.kind === 'live' && result.state.unread).toBe(false);
    expect(effectTypes(result.effects)).toContain('clearUnread');
  });

  it('a moved touch ending away from the bottom keeps reading ownership', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 }).state;
    expectLiveMode(state, 'reading');

    const tailGrowth = reduceScrollMachine(state, {
      type: 'heightChanged',
      totalHeight: 1_100,
      unitCount: 5,
      snapshot: snap(1_100, 100, 400),
      tailActivity: 'active',
    });
    expectLiveMode(tailGrowth.state, 'reading');
    expect(effectTypes(tailGrowth.effects)).not.toContain('snapToLastIndex');
  });

  it('downward movement into the pin zone confirms tail return without a pinned edge', () => {
    const state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'tailContentAdvanced' }).state;
    expect(state.kind === 'live' && state.unread).toBe(true);

    const midway = reduceScrollMachine(state, {
      type: 'downwardMovement',
      snapshot: snap(1_000, 300, 400),
    });
    expectLiveMode(midway.state, 'reading');
    expect(midway.effects).toEqual([]);

    const arrival = reduceScrollMachine(midway.state, {
      type: 'downwardMovement',
      snapshot: snap(1_000, 520, 400),
    });
    expectLiveMode(arrival.state, 'following');
    expect(arrival.state.kind === 'live' && arrival.state.unread).toBe(false);
    expect(effectTypes(arrival.effects)).toContain('clearUnread');
  });

  it('a moved touch whose geometry leaves the pin zone before lift keeps reading', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'tailContentAdvanced' }).state;
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, { type: 'viewportPinnedChanged', atBottom: true }).state;

    // Streaming grows the tail past the pin threshold before the finger
    // lifts; the fresher height snapshot supersedes the stale at-bottom flag.
    state = reduceScrollMachine(state, {
      type: 'heightChanged',
      totalHeight: 1_500,
      unitCount: 6,
      snapshot: snap(1_500, 600, 400),
      tailActivity: 'active',
    }).state;

    const result = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 });
    expectLiveMode(result.state, 'reading');
    expect(result.state.kind === 'live' && result.state.unread).toBe(true);
  });

  it('a zone arrival restores following without asserting the physical edge', () => {
    // The reader has left the tail, so the physical edge reads false.
    const departed = reduceScrollMachine(reading(), {
      type: 'viewportPinnedChanged',
      atBottom: false,
    }).state;
    const state = reduceScrollMachine(departed, {
      type: 'downwardMovement',
      snapshot: snap(1_000, 520, 400),
    }).state;
    expectLiveMode(state, 'following');
    // 1000 - 520 - 400 = 80: inside the pin zone, but the viewport never
    // crossed the physical edge, so the edge must still read false. Recording
    // it as true would survive as a stale value that a later navigation reads
    // as permission to resume following.
    expect(state.kind === 'live' && state.geometry.atBottom).toBe(false);

    // Concretely: navigate away, then interact. A stale edge would confirm
    // the tail return here and let streaming snap the reader to the bottom.
    let navigated = reduceScrollMachine(state, { type: 'navigationJumped' }).state;
    navigated = reduceScrollMachine(navigated, { type: 'interactionStarted' }).state;
    expectLiveMode(navigated, 'navigating');
    expect(navigated.kind === 'live' && navigated.follow.kind === 'navigating' && navigated.follow.phase).toBe('user-returning');
  });

  it('a second finger extends the gesture instead of restarting it', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'tailContentAdvanced' }).state;
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, {
      type: 'downwardMovement',
      snapshot: snap(1_000, 600, 400),
    }).state;

    // A second finger lands. Restarting the interaction would reseed the
    // travelled maximum from the current position, erasing the evidence that
    // the viewport came back from farther away.
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    expect(state.kind === 'live' && state.gesture.kind === 'touch' && state.gesture.moved).toBe(true);

    state = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 1 }).state;
    const result = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 });
    expectLiveMode(result.state, 'following');
    expect(result.state.kind === 'live' && result.state.unread).toBe(false);
  });

  it('navigation ownership survives a gesture ending at the tail', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, {
      type: 'downwardMovement',
      snapshot: snap(1_000, 600, 400),
    }).state;

    // Navigation takes the viewport mid-gesture. The lift must not hand it
    // back: navigation ownership is released only by its own rules.
    state = reduceScrollMachine(state, { type: 'navigationJumped' }).state;
    expectLiveMode(state, 'navigating');

    const result = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 });
    expectLiveMode(result.state, 'navigating');
    expect(effectTypes(result.effects)).not.toContain('clearUnread');

    // A cancellation ends the interaction for reasons of the platform's own —
    // a system gesture, a container claiming the pan — none of which say
    // anything about ownership either.
    const cancelled = reduceScrollMachine(state, { type: 'touchCancelled', remainingTouches: 0 });
    expectLiveMode(cancelled.state, 'navigating');
    expect(effectTypes(cancelled.effects)).not.toContain('clearUnread');
  });

  it('a pending navigation measurement leaves the physical edge unobserved', () => {
    let result = reduceScrollMachine(initialScrollMachineState('conv'), { type: 'navigationJumped' });
    result = reduceScrollMachine(result.state, {
      type: 'conversationMeasured',
      conversationId: 'conv',
      totalHeight: 1_000,
      unitCount: 5,
      // 1000 - 550 - 400 = 50: inside the pin zone, but no pinned callback
      // has been observed, so the edge must not be asserted from it.
      snapshot: snap(1_000, 550, 400),
      nowMs: 1_000,
    });
    expectLiveMode(result.state, 'navigating');
    expect(result.state.kind === 'live' && result.state.geometry.atBottom).toBe(false);

    // Otherwise the first interaction would release navigation ownership.
    const interacted = reduceScrollMachine(result.state, { type: 'interactionStarted' });
    expectLiveMode(interacted.state, 'navigating');
  });

  it('a gesture that never moved the viewport does not confirm on lift', () => {
    // iOS delivers touchmove before, or without, the scroll events that would
    // show where the viewport went. With no observed travel there is nothing
    // to distinguish an arrival from a finger that never shifted anything, so
    // ownership stays with the reader.
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    const result = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 });
    expectLiveMode(result.state, 'reading');
  });

  it('tail growth under a held touch is not travel and does not confirm on lift', () => {
    // The viewport never moves here: content grows away beneath a stationary
    // finger and then reflows back. Distance-from-tail rises and falls, but
    // the reader travelled nothing, so this must resolve exactly like the
    // never-moved case above. Letting layout raise the travel maximum would
    // manufacture the one precondition the lift derivation depends on.
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, {
      type: 'heightChanged',
      totalHeight: 1_400,
      unitCount: 5,
      snapshot: snap(1_400, 600, 400),
      tailActivity: 'active',
    }).state;
    state = reduceScrollMachine(state, {
      type: 'heightChanged',
      totalHeight: 1_000,
      unitCount: 5,
      snapshot: snap(1_000, 600, 400),
      tailActivity: 'active',
    }).state;

    const result = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 });
    expectLiveMode(result.state, 'reading');
  });

  it('content shrinking toward a held touch is not travel and does not confirm on lift', () => {
    // The mirror of the growth case: the reader is 500px up, holds still, and
    // content below them collapses until the tail is 50px away. The viewport
    // never moved, so the gesture ending near the tail is the content's doing
    // and must not hand the viewport back.
    let state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, {
      type: 'heightChanged',
      totalHeight: 550,
      unitCount: 5,
      snapshot: snap(550, 100, 400),
      tailActivity: 'none',
    }).state;

    const result = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 });
    expectLiveMode(result.state, 'reading');
  });

  it('an abandoned gesture keeps ownership and drops its evidence', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, { type: 'downwardMovement', snapshot: snap(1_000, 450, 400) }).state;

    // The lift is never observed, so there is no position to confirm from.
    // Ownership stays where the movement put it and the evidence is dropped.
    const abandoned = reduceScrollMachine(state, { type: 'gestureAbandoned' });
    expectLiveMode(abandoned.state, 'reading');
    expect(abandoned.state.kind === 'live' && abandoned.state.gesture.kind).toBe('idle');
    expect(effectTypes(abandoned.effects)).toEqual([]);

    // A later gesture that ends in the return zone without travelling of its
    // own cannot borrow the abandoned one's.
    let next: ScrollMachineState = reduceScrollMachine(abandoned.state, { type: 'touchStarted' }).state;
    next = reduceScrollMachine(next, { type: 'touchMoved' }).state;
    next = reduceScrollMachine(next, {
      type: 'heightChanged',
      totalHeight: 900,
      unitCount: 5,
      snapshot: snap(900, 450, 400),
      tailActivity: 'none',
    }).state;
    expectLiveMode(reduceScrollMachine(next, { type: 'touchEnded', remainingTouches: 0 }).state, 'reading');
  });

  it('a gesture that departs the tail and returns to it confirms on lift', () => {
    // The same start and end position as the case above; only the travel in
    // between distinguishes them, which is why the maximum is recorded.
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, { type: 'upwardIntent', snapshot: snap(1_000, 100, 400) }).state;
    state = reduceScrollMachine(state, { type: 'tailContentAdvanced' }).state;
    expect(state.kind === 'live' && state.unread).toBe(true);
    state = reduceScrollMachine(state, { type: 'downwardMovement', snapshot: snap(1_000, 600, 400) }).state;

    const result = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 });
    expectLiveMode(result.state, 'following');
    expect(result.state.kind === 'live' && result.state.unread).toBe(false);
  });

  it('downward movement in the pin zone during a moved touch defers to gesture end', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    const during = reduceScrollMachine(state, {
      type: 'downwardMovement',
      snapshot: snap(1_000, 520, 400),
    });
    expectLiveMode(during.state, 'reading');

    const released = reduceScrollMachine(during.state, { type: 'touchEnded', remainingTouches: 0 });
    expectLiveMode(released.state, 'following');
  });

  it('tap-only touch restores the mode from before the gesture', () => {
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 }).state;
    expectLiveMode(state, 'following');

    const result = reduceScrollMachine(state, {
      type: 'heightChanged',
      totalHeight: 1_100,
      unitCount: 5,
      snapshot: snap(1_100, 600, 400),
      tailActivity: 'active',
    });
    expect(result.effects).toEqual([{ type: 'snapToLastIndex' }]);
  });

  it('moved touch blocks follow without requiring a scroll event', () => {
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), {
      type: 'viewportPinnedChanged',
      atBottom: false,
    }).state;
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 }).state;
    const result = reduceScrollMachine(state, {
      type: 'heightChanged',
      totalHeight: 1_100,
      unitCount: 5,
      snapshot: snap(1_100, 600, 400),
      tailActivity: 'active',
    });

    expectLiveMode(result.state, 'reading');
    expect(result.effects).toEqual([{ type: 'showUnread' }]);
  });

  it('touch cancellation has an explicit durable-reading outcome after movement', () => {
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), {
      type: 'viewportPinnedChanged',
      atBottom: false,
    }).state;
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, { type: 'touchCancelled', remainingTouches: 0 }).state;
    expectLiveMode(state, 'reading');
    expect(state.kind === 'live' && state.gesture.kind).toBe('idle');
  });

  it('jump-to-newest remains returning until bottom is confirmed', () => {
    const state = reduceScrollMachine(reading(), { type: 'tailContentAdvanced' }).state;
    let result = reduceScrollMachine(state, { type: 'jumpToNewestRequested', unitCount: 5 });

    expectLiveMode(result.state, 'returning-to-tail');
    expect(result.effects).toEqual([{ type: 'clearUnread' }, { type: 'snapToLastIndex' }]);

    result = reduceScrollMachine(result.state, { type: 'viewportPinnedChanged', atBottom: true });
    expectLiveMode(result.state, 'following');
    expect(result.effects).toEqual([]);
  });

  it('keeps mount rescue alive through an initial bottom confirmation', () => {
    const mounted = measured();
    const result = reduceScrollMachine(mounted.state, {
      type: 'viewportPinnedChanged',
      atBottom: true,
    });

    expect(result.state.kind).toBe('mount-rescue');
    expect(result.effects).toEqual([]);

    const stranded = reduceScrollMachine(result.state, {
      type: 'settleProbe',
      snapshot: snap(1_200, 500, 400),
      nowMs: 1_100,
    });
    expect(stranded.effects).toEqual([{ type: 'writeDomBottom' }]);
  });

  it('preserves a navigation jump made before first measurement', () => {
    let result = reduceScrollMachine(initialScrollMachineState('conv'), { type: 'navigationJumped' });
    expect(result.state.kind === 'unmeasured' && result.state.navigationPending).toBe(true);

    result = reduceScrollMachine(result.state, {
      type: 'conversationMeasured',
      conversationId: 'conv',
      totalHeight: 1_000,
      unitCount: 5,
      snapshot: snap(1_000, 200, 400),
      nowMs: 1_000,
    });

    expectLiveMode(result.state, 'navigating');
    expect(result.state.kind === 'live' && result.state.follow.kind === 'navigating' && result.state.follow.phase).toBe('positioning');
    expect(result.effects).toEqual([]);

    result = reduceScrollMachine(result.state, { type: 'viewportPinnedChanged', atBottom: true });
    expectLiveMode(result.state, 'navigating');
  });

  it('preserves a navigation jump while an initially empty list waits for content', () => {
    let result = reduceScrollMachine(initialScrollMachineState('conv'), { type: 'navigationJumped' });
    result = reduceScrollMachine(result.state, {
      type: 'conversationMeasured',
      conversationId: 'conv',
      totalHeight: 0,
      unitCount: 0,
      snapshot: snap(0, 0, 400),
      nowMs: 1_000,
    });
    expect(result.state.kind === 'measured-empty' && result.state.navigationPending).toBe(true);

    result = reduceScrollMachine(result.state, {
      type: 'conversationMeasured',
      conversationId: 'conv',
      totalHeight: 1_000,
      unitCount: 5,
      snapshot: snap(1_000, 600, 400),
      nowMs: 1_100,
    });
    expectLiveMode(result.state, 'navigating');
    expect(result.effects).toEqual([]);
  });

  it('navigation jumps take durable ownership and disable mount rescue', () => {
    const mounted = measured();
    const result = reduceScrollMachine(mounted.state, { type: 'navigationJumped' });

    expectLiveMode(result.state, 'navigating');
    expect(result.effects).toEqual([{ type: 'stopSettleWatch' }]);
    expect(reduceScrollMachine(result.state, {
      type: 'settleProbe',
      snapshot: snap(1_000, 0, 400),
      nowMs: 1_001,
    }).effects).toEqual([]);
  });

  it('keeps an off-bottom jump positioning until post-jump user interaction', () => {
    let state = reduceScrollMachine(liveFollowing(), {
      type: 'viewportPinnedChanged',
      atBottom: false,
    }).state;
    state = reduceScrollMachine(state, { type: 'navigationJumped' }).state;

    let result = reduceScrollMachine(state, { type: 'viewportPinnedChanged', atBottom: true });
    expectLiveMode(result.state, 'navigating');
    result = reduceScrollMachine(result.state, { type: 'viewportPinnedChanged', atBottom: false });

    result = reduceScrollMachine(result.state, { type: 'interactionStarted' });
    expect(result.state.kind === 'live' && result.state.follow.kind === 'navigating' && result.state.follow.phase).toBe('user-returning');
    result = reduceScrollMachine(result.state, { type: 'viewportPinnedChanged', atBottom: true });
    expectLiveMode(result.state, 'following');
  });

  it('preserves positioning through programmatic upward scroll events', () => {
    let state = reduceScrollMachine(liveFollowing(), { type: 'navigationJumped' }).state;
    state = reduceScrollMachine(state, {
      type: 'upwardIntent',
      snapshot: snap(1_000, 200, 400),
    }).state;

    expectLiveMode(state, 'navigating');
    expect(state.kind === 'live' && state.follow.kind === 'navigating' && state.follow.phase).toBe('positioning');
  });

  it('does not mark a near-tail navigation jump as departed while it remains pinned', () => {
    let state = reduceScrollMachine(liveFollowing(), { type: 'navigationJumped' }).state;
    state = reduceScrollMachine(state, {
      type: 'upwardIntent',
      snapshot: snap(1_000, 550, 400),
    }).state;

    expectLiveMode(state, 'navigating');
    expect(state.kind === 'live' && state.follow.kind === 'navigating' && state.follow.phase).toBe('positioning');

    const result = reduceScrollMachine(state, { type: 'viewportPinnedChanged', atBottom: true });
    expectLiveMode(result.state, 'navigating');
  });

  it('releases positioned navigation on interaction when geometry is already pinned', () => {
    let result = reduceScrollMachine(liveFollowing(), { type: 'navigationJumped' });
    result = reduceScrollMachine(result.state, { type: 'viewportPinnedChanged', atBottom: true });
    expectLiveMode(result.state, 'navigating');

    result = reduceScrollMachine(result.state, { type: 'interactionStarted' });
    expectLiveMode(result.state, 'following');
    expect(result.effects).toEqual([]);
  });

  it('navigation ownership survives movement and later height growth without a tail snap', () => {
    let result = reduceScrollMachine(liveFollowing(), { type: 'navigationJumped' });
    result = reduceScrollMachine(result.state, {
      type: 'downwardMovement',
      snapshot: snap(1_000, 250, 400),
    });
    expectLiveMode(result.state, 'navigating');
    expect(result.effects).toEqual([]);

    result = reduceScrollMachine(result.state, { type: 'viewportPinnedChanged', atBottom: true });
    expectLiveMode(result.state, 'navigating');
    expect(result.effects).toEqual([]);

    result = reduceScrollMachine(result.state, { type: 'viewportPinnedChanged', atBottom: false });
    expectLiveMode(result.state, 'navigating');
    result = reduceScrollMachine(result.state, { type: 'interactionStarted' });
    result = reduceScrollMachine(result.state, { type: 'viewportPinnedChanged', atBottom: true });
    expectLiveMode(result.state, 'following');
    expect(result.effects).toEqual([]);

    result = reduceScrollMachine(
      reduceScrollMachine(liveFollowing(), { type: 'navigationJumped' }).state,
      {
        type: 'heightChanged',
        totalHeight: 1_200,
        unitCount: 5,
        snapshot: snap(1_200, 250, 400),
        tailActivity: 'active',
      },
    );
    expectLiveMode(result.state, 'navigating');
    expect(result.effects).toEqual([{ type: 'showUnread' }]);
  });

  it('tail advance while following requests one live follow action', () => {
    const result = reduceScrollMachine(liveFollowing(), { type: 'tailContentAdvanced' });
    expect(result.effects).toEqual([{ type: 'scheduleTailFollow', conversationId: 'conv' }]);
    expect(result.state.kind === 'live' && result.state.unread).toBe(false);
  });

  it('tail advance while reading shows unread without scrolling', () => {
    const result = reduceScrollMachine(reading(), { type: 'tailContentAdvanced' });
    expect(result.effects).toEqual([{ type: 'showUnread' }]);
    expect(result.state.kind === 'live' && result.state.unread).toBe(true);
  });

  it('unrelated layout growth while reading neither scrolls nor creates unread', () => {
    const result = reduceScrollMachine(reading(), {
      type: 'heightChanged',
      totalHeight: 1_100,
      unitCount: 5,
      snapshot: snap(1_100, 500, 400),
      tailActivity: 'none',
    });
    expect(result.effects).toEqual([]);
    expect(result.state.kind === 'live' && result.state.unread).toBe(false);
  });

  it('first content after an empty mount enters bounded rescue', () => {
    let result = measured(0, snap(0, 0, 500));
    expect(result.state.kind).toBe('measured-empty');

    result = reduceScrollMachine(result.state, {
      type: 'conversationMeasured',
      conversationId: 'conv',
      totalHeight: 600,
      unitCount: 5,
      snapshot: snap(600, 0, 500),
      nowMs: 1_200,
    });
    expect(result.state.kind).toBe('mount-rescue');
    expect(effectTypes(result.effects)).toEqual(['snapToLastIndex', 'startSettleWatch', 'scheduleDomBottomWrite']);
  });

  it('mount rescue repairs silent stranding and expires into live follow', () => {
    const mounted = measured(5, snap(12_000_000, 11_951_000, 600));
    let result = reduceScrollMachine(mounted.state, {
      type: 'settleProbe',
      snapshot: snap(12_000_000, 11_951_000, 600),
      nowMs: 1_100,
    });
    expect(result.effects).toEqual([{ type: 'writeDomBottom' }]);

    result = reduceScrollMachine(result.state, {
      type: 'settleProbe',
      snapshot: snap(12_000_000, 11_951_000, 600),
      nowMs: 1_000 + SETTLE_WATCH_MS + 1,
    });
    expectLiveMode(result.state, 'following');
    expect(result.effects).toEqual([{ type: 'stopSettleWatch' }]);
  });

  it('any interaction permanently exits rescue for the mounted conversation', () => {
    const exited = reduceScrollMachine(measured().state, { type: 'interactionStarted' });
    expectLiveMode(exited.state, 'following');
    expect(exited.effects).toEqual([{ type: 'stopSettleWatch' }]);

    const height = reduceScrollMachine(exited.state, {
      type: 'heightChanged',
      totalHeight: 1_100,
      unitCount: 5,
      snapshot: snap(1_100, 0, 400),
      tailActivity: 'none',
    });
    expect(height.effects).toEqual([{ type: 'snapToLastIndex' }]);
    expect(effectTypes(height.effects)).not.toContain('scheduleDomBottomWrite');
  });

  it('mismatched measurement preserves reset and new-mount effects', () => {
    const unread = reduceScrollMachine(reading(), { type: 'tailContentAdvanced' }).state;
    const unreadResult = reduceScrollMachine(unread, {
      type: 'conversationMeasured',
      conversationId: 'new',
      totalHeight: 800,
      unitCount: 4,
      snapshot: snap(800, 400, 400),
      nowMs: 2_000,
    });
    expect(effectTypes(unreadResult.effects)).toEqual([
      'clearUnread',
      'startSettleWatch',
      'scheduleDomBottomWrite',
    ]);

    const rescueResult = reduceScrollMachine(measured().state, {
      type: 'conversationMeasured',
      conversationId: 'new',
      totalHeight: 800,
      unitCount: 4,
      snapshot: snap(800, 400, 400),
      nowMs: 2_000,
    });
    expect(effectTypes(rescueResult.effects)).toEqual([
      'stopSettleWatch',
      'startSettleWatch',
      'scheduleDomBottomWrite',
    ]);
  });

  it('keeps a multi-touch gesture active until the final touch ends or cancels', () => {
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), {
      type: 'viewportPinnedChanged',
      atBottom: false,
    }).state;
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 2 }).state;
    expect(state.kind === 'live' && state.gesture.kind).toBe('touch');
    expectLiveMode(state, 'reading');

    state = reduceScrollMachine(state, { type: 'touchCancelled', remainingTouches: 1 }).state;
    expect(state.kind === 'live' && state.gesture.kind).toBe('touch');
    expectLiveMode(state, 'reading');

    state = reduceScrollMachine(state, { type: 'touchCancelled', remainingTouches: 0 }).state;
    expect(state.kind === 'live' && state.gesture.kind).toBe('idle');
    expectLiveMode(state, 'reading');
  });

  it('stationary touch cancellation restores the pre-gesture mode', () => {
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchCancelled', remainingTouches: 0 }).state;
    expect(state.kind === 'live' && state.gesture.kind).toBe('idle');
    expectLiveMode(state, 'following');
  });

  it('never scrolls an empty jump-to-newest request', () => {
    const result = reduceScrollMachine(reading(), {
      type: 'jumpToNewestRequested',
      unitCount: 0,
    });
    expect(result.effects).toEqual([]);
    expectLiveMode(result.state, 'reading');
  });

  it('conversation switch atomically resets lifecycle, unread, gesture, and geometry', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), { type: 'tailContentAdvanced' }).state;
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    const result = reduceScrollMachine(state, {
      type: 'conversationChanged',
      conversationId: 'new',
    });

    expect(result.state).toEqual({ kind: 'unmeasured', conversationId: 'new', navigationPending: false });
    expect(result.effects).toEqual([{ type: 'clearUnread' }]);
  });
});

const commandArb: fc.Arbitrary<ScrollEvent> = fc.oneof(
  fc.constant({ type: 'interactionStarted' } as const),
  fc.constant({ type: 'touchStarted' } as const),
  fc.constant({ type: 'touchMoved' } as const),
  fc.record({
    type: fc.constant('touchEnded' as const),
    remainingTouches: fc.integer({ min: 0, max: 4 }),
  }),
  fc.record({
    type: fc.constant('touchCancelled' as const),
    remainingTouches: fc.integer({ min: 0, max: 4 }),
  }),
  fc.constant({ type: 'upwardIntent' } as const),
  fc.record({
    type: fc.constant('downwardMovement' as const),
    snapshot: fc.constantFrom(snap(1_000, 100, 400), snap(1_000, 520, 400), snap(1_000, 600, 400)),
  }),
  fc.constant({ type: 'navigationJumped' } as const),
  fc.constant({ type: 'tailContentAdvanced' } as const),
  fc.record({ type: fc.constant('viewportPinnedChanged' as const), atBottom: fc.boolean() }),
  fc.record({
    type: fc.constant('heightChanged' as const),
    totalHeight: fc.integer({ min: 1, max: 100_000 }),
    unitCount: fc.integer({ min: 1, max: 200 }),
    snapshot: fc.constantFrom(snap(1_000, 600, 400), snap(1_000, 100, 400)),
    tailActivity: fc.constantFrom<'none' | 'active'>('none', 'active'),
  }),
  fc.record({
    type: fc.constant('jumpToNewestRequested' as const),
    unitCount: fc.integer({ min: 0, max: 200 }),
  }),
);

function assertReachableInvariants(state: ScrollMachineState, effects: ScrollEffect[]) {
  const visibleScrolls = effects.filter((effect) =>
    effect.type === 'snapToLastIndex' ||
    effect.type === 'scheduleTailFollow' ||
    effect.type === 'writeDomBottom',
  );
  expect(visibleScrolls.length).toBeLessThanOrEqual(1);
  expect(!(effectTypes(effects).includes('showUnread') && effectTypes(effects).includes('clearUnread'))).toBe(true);
  if (effectTypes(effects).includes('writeDomBottom')) expect(state.kind).toBe('mount-rescue');
  if (state.kind === 'unmeasured' || state.kind === 'measured-empty') {
    expect('gesture' in state).toBe(false);
  }
  if (state.kind === 'mount-rescue') expect(Number.isFinite(state.deadlineMs)).toBe(true);
}

describe('scrollMachine reachable-history properties', () => {
  it('preserves union and effect invariants across generated valid histories', () => {
    fc.assert(
      fc.property(fc.array(commandArb, { maxLength: 100 }), (events) => {
        let state: ScrollMachineState = initialScrollMachineState('conv');
        let result = reduceScrollMachine(state, {
          type: 'conversationMeasured',
          conversationId: 'conv',
          totalHeight: 1_000,
          unitCount: 5,
          snapshot: snap(1_000, 600, 400),
          nowMs: 1_000,
        });
        state = result.state;
        assertReachableInvariants(state, result.effects);

        for (const event of events) {
          result = reduceScrollMachine(state, event);
          state = result.state;
          assertReachableInvariants(state, result.effects);
        }
      }),
      { numRuns: 1_000 },
    );
  });

  it('never returns reading ownership merely because more events or time pass', () => {
    // Excluded events are the legitimate release paths: bottom confirmation,
    // explicit jump, navigation, and downward arrival in the pin zone. Any
    // pinned-snapshot event marks geometry at-bottom, which a later gesture
    // end is then allowed to confirm — so pinned snapshots are excluded too.
    const nonReleaseArb = commandArb.filter((event) =>
      event.type !== 'viewportPinnedChanged' &&
      event.type !== 'jumpToNewestRequested' &&
      event.type !== 'navigationJumped' &&
      !(event.type === 'downwardMovement' && snapshotIsPinned(event.snapshot)) &&
      !(event.type === 'heightChanged' && event.snapshot !== null && snapshotIsPinned(event.snapshot)),
    );
    fc.assert(
      fc.property(fc.array(nonReleaseArb, { maxLength: 100 }), (events) => {
        let state: ScrollMachineState = reading();
        for (const event of events) state = reduceScrollMachine(state, event).state;
        expectLiveMode(state, 'reading');
      }),
      { numRuns: 500 },
    );
  });
});
