import { describe, expect, it } from 'vitest';
import * as fc from 'fast-check';
import {
  SETTLE_WATCH_MS,
  initialScrollMachineState,
  reduceScrollMachine,
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

function reading(): Extract<ScrollMachineState, { kind: 'live' }> {
  return reduceScrollMachine(liveFollowing(), { type: 'upwardIntent' }).state as Extract<ScrollMachineState, { kind: 'live' }>;
}

const effectTypes = (effects: ScrollEffect[]) => effects.map((effect) => effect.type);

function expectLiveMode(state: ScrollMachineState, mode: 'following' | 'reading' | 'returning-to-tail') {
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

  it('a bottom callback during a moved touch cannot release ownership', () => {
    let state: ScrollMachineState = reduceScrollMachine(reading(), {
      type: 'viewportPinnedChanged',
      atBottom: false,
    }).state;
    state = reduceScrollMachine(state, { type: 'touchStarted' }).state;
    state = reduceScrollMachine(state, { type: 'touchMoved' }).state;
    state = reduceScrollMachine(state, { type: 'viewportPinnedChanged', atBottom: true }).state;
    expectLiveMode(state, 'reading');
    expect(state.kind === 'live' && state.gesture.kind === 'touch' && state.gesture.departedBottom).toBe(true);

    state = reduceScrollMachine(state, { type: 'touchEnded', remainingTouches: 0 }).state;
    expectLiveMode(state, 'reading');

    const tailGrowth = reduceScrollMachine(state, {
      type: 'heightChanged',
      totalHeight: 1_100,
      unitCount: 5,
      snapshot: snap(1_100, 700, 400),
      tailActivity: 'active',
    });
    expectLiveMode(tailGrowth.state, 'reading');
    expect(effectTypes(tailGrowth.effects)).not.toContain('snapToLastIndex');
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
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), { type: 'touchStarted' }).state;
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
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), { type: 'touchStarted' }).state;
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

  it('navigation jumps take durable ownership and disable mount rescue', () => {
    const mounted = measured();
    const result = reduceScrollMachine(mounted.state, { type: 'navigationJumped' });

    expectLiveMode(result.state, 'reading');
    expect(result.effects).toEqual([{ type: 'stopSettleWatch' }]);
    expect(reduceScrollMachine(result.state, {
      type: 'settleProbe',
      snapshot: snap(1_000, 0, 400),
      nowMs: 1_001,
    }).effects).toEqual([]);
  });

  it('navigation ownership survives movement and later height growth without a tail snap', () => {
    let result = reduceScrollMachine(liveFollowing(), { type: 'navigationJumped' });
    result = reduceScrollMachine(result.state, {
      type: 'downwardMovement',
      snapshot: snap(1_000, 250, 400),
    });
    expectLiveMode(result.state, 'reading');
    expect(result.effects).toEqual([]);

    result = reduceScrollMachine(result.state, {
      type: 'heightChanged',
      totalHeight: 1_200,
      unitCount: 5,
      snapshot: snap(1_200, 250, 400),
      tailActivity: 'active',
    });
    expectLiveMode(result.state, 'reading');
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
    let state: ScrollMachineState = reduceScrollMachine(liveFollowing(), { type: 'touchStarted' }).state;
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

    expect(result.state).toEqual({ kind: 'unmeasured', conversationId: 'new' });
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
  fc.constant({ type: 'navigationJumped' } as const),
  fc.constant({ type: 'tailContentAdvanced' } as const),
  fc.record({ type: fc.constant('viewportPinnedChanged' as const), atBottom: fc.boolean() }),
  fc.record({
    type: fc.constant('heightChanged' as const),
    totalHeight: fc.integer({ min: 1, max: 100_000 }),
    unitCount: fc.integer({ min: 1, max: 200 }),
    snapshot: fc.constant(snap(1_000, 600, 400)),
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
    const nonReleaseArb = commandArb.filter((event) =>
      event.type !== 'viewportPinnedChanged' && event.type !== 'jumpToNewestRequested',
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
