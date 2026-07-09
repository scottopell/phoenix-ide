import { describe, expect, it } from 'vitest';
import * as fc from 'fast-check';
import {
  PIN_TO_BOTTOM_THRESHOLD,
  SETTLE_WATCH_MS,
  TOUCH_GESTURE_SUPPRESS_MS,
  USER_SCROLL_SUPPRESS_MS,
  initialScrollMachineState,
  reduceScrollMachine,
  type ScrollEffect,
  type ScrollEvent,
  type ScrollMachineState,
  type ScrollSnapshot,
  type TailActivity,
} from './scrollMachine';

const snap = (scrollHeight: number, scrollTop: number, clientHeight: number): ScrollSnapshot => ({
  scrollHeight,
  scrollTop,
  clientHeight,
});

function reduce(state: ScrollMachineState, event: Parameters<typeof reduceScrollMachine>[1]) {
  return reduceScrollMachine(state, event);
}

function measured(opts: {
  conversationId?: string;
  totalHeight: number;
  unitCount?: number;
  snapshot?: ScrollSnapshot | null;
  nowMs?: number;
  tailActivity?: TailActivity;
}) {
  return {
    type: 'totalHeightChanged' as const,
    conversationId: opts.conversationId ?? 'conv',
    totalHeight: opts.totalHeight,
    unitCount: opts.unitCount ?? 5,
    snapshot: opts.snapshot ?? snap(opts.totalHeight, 0, 400),
    tailActivity: opts.tailActivity ?? 'none',
    nowMs: opts.nowMs ?? 1000,
  };
}

describe('scrollMachine', () => {
  it('re-snaps when pinned and height grows', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 500, snapshot: snap(500, 100, 400) }));
    result = reduce(result.state, { type: 'wheel', deltaY: 50, nowMs: 1100 });
    result = reduce(result.state, measured({ totalHeight: 600, snapshot: snap(600, 100, 400), nowMs: 1200 }));

    expect(result.effects).toContainEqual({ type: 'snapToLastIndex' });
  });

  it('does not re-snap when scrolled up past threshold and height grows', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 1000, snapshot: snap(1000, 0, 400) }));
    result = reduce(result.state, { type: 'wheel', deltaY: -50, nowMs: 1100 });
    result = reduce(result.state, measured({ totalHeight: 1200, snapshot: snap(1200, 0, 400), nowMs: 1200 }));

    expect(result.effects.map((e) => e.type)).not.toContain('snapToLastIndex');
  });

  it('snaps to bottom on first non-empty update after mounting empty', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 0, unitCount: 0, snapshot: snap(0, 0, 500) }));
    expect(result.effects).toEqual([]);

    result = reduce(result.state, measured({ totalHeight: 600, unitCount: 5, snapshot: snap(600, 0, 500), nowMs: 1200 }));

    expect(result.effects.map((e) => e.type)).toEqual(['snapToLastIndex', 'startSettleWatch', 'scheduleDomBottomWrite']);
  });

  it('seeds a fresh conversation baseline without a stale snap', () => {
    let result = reduce(initialScrollMachineState(), measured({ conversationId: 'a', totalHeight: 500, snapshot: snap(500, 100, 400) }));
    expect(result.effects.map((e) => e.type)).toEqual(['startSettleWatch', 'scheduleDomBottomWrite']);

    result = reduce(result.state, measured({ conversationId: 'b', totalHeight: 1000, snapshot: snap(1000, 0, 400), nowMs: 1200 }));
    expect(result.effects.map((e) => e.type)).toEqual(['startSettleWatch', 'scheduleDomBottomWrite']);
    expect(result.effects.map((e) => e.type)).not.toContain('snapToLastIndex');
  });

  it('self-heals a stranded mount during bounded settle', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 48_000, snapshot: snap(48_000, 0, 600), nowMs: 1000 }));
    result = reduce(result.state, measured({ totalHeight: 12_000_000, snapshot: snap(12_000_000, 0, 600), nowMs: 1100 }));

    expect(result.effects).toContainEqual({ type: 'scheduleDomBottomWrite' });
  });

  it('settle tick writes DOM bottom until engagement, then stops', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 1000, snapshot: snap(1000, 0, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'settleTick', snapshot: snap(1000, 100, 400), nowMs: 1200 });
    expect(result.effects).toContainEqual({ type: 'writeDomBottom' });

    result = reduce(result.state, { type: 'touchStart' });
    result = reduce(result.state, { type: 'settleTick', snapshot: snap(1000, 100, 400), nowMs: 1300 });
    expect(result.effects).toContainEqual({ type: 'stopSettleWatch' });
  });

  it('does not fight scroll-only input after the settle window', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 1000, snapshot: snap(1000, 600, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'scroll', snapshot: snap(1000, 100, 400), nowMs: 1000 + SETTLE_WATCH_MS + 500 });
    result = reduce(result.state, measured({ totalHeight: 1100, snapshot: snap(1100, 100, 400), nowMs: 1000 + SETTLE_WATCH_MS + 600 }));

    expect(result.effects.map((e) => e.type)).not.toContain('snapToLastIndex');
    expect(result.effects.map((e) => e.type)).not.toContain('writeDomBottom');
  });

  it('suppresses re-snap during touch and upward momentum, then resumes', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 500, snapshot: snap(500, 100, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'touchStart' });
    result = reduce(result.state, measured({ totalHeight: 600, snapshot: snap(600, 80, 400), nowMs: 1100 }));
    expect(result.effects.map((e) => e.type)).not.toContain('snapToLastIndex');

    result = reduce(result.state, { type: 'touchEnd', remainingTouches: 0, nowMs: 1120 });
    result = reduce(result.state, { type: 'scroll', snapshot: snap(600, 60, 400), nowMs: 1150 });
    result = reduce(result.state, measured({ totalHeight: 700, snapshot: snap(700, 60, 400), nowMs: 1200 }));
    expect(result.effects.map((e) => e.type)).not.toContain('snapToLastIndex');

    result = reduce(result.state, measured({ totalHeight: 800, snapshot: snap(800, 300, 400), nowMs: 1120 + TOUCH_GESTURE_SUPPRESS_MS + 1 }));
    expect(result.effects).toContainEqual({ type: 'snapToLastIndex' });
  });

  it('suppresses a second iOS braking touch even without a fresh upward scroll delta', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 500, snapshot: snap(500, 100, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'scroll', snapshot: snap(500, 100, 400), nowMs: 1000 });

    result = reduce(result.state, { type: 'touchStart' });
    result = reduce(result.state, { type: 'touchMove', nowMs: 1070 });
    result = reduce(result.state, { type: 'touchEnd', remainingTouches: 0, nowMs: 1080 });
    result = reduce(result.state, { type: 'scroll', snapshot: snap(500, 80, 400), nowMs: 1100 });

    result = reduce(result.state, { type: 'touchStart' });
    result = reduce(result.state, { type: 'touchMove', nowMs: 1820 });
    result = reduce(result.state, { type: 'touchEnd', remainingTouches: 0, nowMs: 1830 });
    result = reduce(result.state, measured({ totalHeight: 600, snapshot: snap(600, 80, 400), tailActivity: 'active', nowMs: 1900 }));
    expect(result.effects.map((e) => e.type)).not.toContain('snapToLastIndex');
    expect(result.effects).toContainEqual({ type: 'showUnread' });

    result = reduce(result.state, measured({ totalHeight: 700, snapshot: snap(700, 200, 400), tailActivity: 'active', nowMs: 1830 + TOUCH_GESTURE_SUPPRESS_MS + 1 }));
    expect(result.effects).toContainEqual({ type: 'snapToLastIndex' });
  });

  it('suppresses moved touch gestures even when no upward scroll event has arrived', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 500, snapshot: snap(500, 100, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'scroll', snapshot: snap(500, 100, 400), nowMs: 1000 });

    result = reduce(result.state, { type: 'touchStart' });
    result = reduce(result.state, { type: 'touchMove', nowMs: 1050 });
    result = reduce(result.state, { type: 'touchEnd', remainingTouches: 0, nowMs: 1060 });
    result = reduce(result.state, measured({ totalHeight: 600, snapshot: snap(600, 95, 400), tailActivity: 'active', nowMs: 1100 }));

    expect(result.effects.map((e) => e.type)).not.toContain('snapToLastIndex');
    expect(result.effects).toContainEqual({ type: 'showUnread' });
  });

  it('does not suppress on downward scroll', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 500, snapshot: snap(500, 50, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'wheel', deltaY: 50, nowMs: 1100 });
    result = reduce(result.state, { type: 'scroll', snapshot: snap(500, 100, 400), nowMs: 1150 });
    result = reduce(result.state, measured({ totalHeight: 600, snapshot: snap(600, 100, 400), nowMs: 1200 }));

    expect(result.effects).toContainEqual({ type: 'snapToLastIndex' });
  });

  it('uses DOM scrollHeight rather than virtualizer total height for pin distance', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 675, snapshot: snap(600, 168, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'wheel', deltaY: 50, nowMs: 1100 });
    result = reduce(result.state, measured({ totalHeight: 775, snapshot: snap(700, 168, 400), nowMs: 1200 }));

    expect(result.effects).toContainEqual({ type: 'snapToLastIndex' });
  });

  it('re-snaps when viewport shrinks while pinned', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 800, snapshot: snap(800, 100, 700), nowMs: 1000 }));
    result = reduce(result.state, { type: 'wheel', deltaY: 50, nowMs: 1100 });
    result = reduce(result.state, measured({ totalHeight: 800, snapshot: snap(800, 100, 500), nowMs: 1200 }));

    expect(result.effects).toContainEqual({ type: 'snapToLastIndex' });
  });

  it('marks unread when gesture suppression swallows a genuine tail-growth snap', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 500, snapshot: snap(500, 100, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'touchStart' });
    result = reduce(result.state, measured({ totalHeight: 600, snapshot: snap(600, 100, 400), tailActivity: 'active', nowMs: 1100 }));

    expect(result.effects).toContainEqual({ type: 'showUnread' });
    expect(result.effects.map((e) => e.type)).not.toContain('snapToLastIndex');
  });

  it('keeps auto-follow for tap-only touches while pinned', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 500, snapshot: snap(500, 100, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'touchStart' });
    result = reduce(result.state, { type: 'touchEnd', remainingTouches: 0, nowMs: 1060 });

    result = reduce(result.state, measured({ totalHeight: 600, snapshot: snap(600, 100, 400), tailActivity: 'active', nowMs: 1100 }));

    expect(result.effects).toContainEqual({ type: 'snapToLastIndex' });
    expect(result.effects.map((e) => e.type)).not.toContain('showUnread');
  });

  it('clears stale upward intent when returning to bottom before a tap-only touch', () => {
    let result = reduce(initialScrollMachineState(), measured({ totalHeight: 500, snapshot: snap(500, 100, 400), nowMs: 1000 }));
    result = reduce(result.state, { type: 'scroll', snapshot: snap(500, 80, 400), nowMs: 1050 });
    result = reduce(result.state, { type: 'atBottomChanged', atBottom: true });
    result = reduce(result.state, { type: 'touchStart' });
    result = reduce(result.state, { type: 'touchEnd', remainingTouches: 0, nowMs: 1110 });

    result = reduce(result.state, measured({ totalHeight: 600, snapshot: snap(600, 100, 400), tailActivity: 'active', nowMs: 1150 }));

    expect(result.effects).toContainEqual({ type: 'snapToLastIndex' });
    expect(result.effects.map((e) => e.type)).not.toContain('showUnread');
  });

  it('clears unread on at-bottom and jump-to-newest events', () => {
    let result = reduce(initialScrollMachineState(), { type: 'atBottomChanged', atBottom: true });
    expect(result.effects).toEqual([{ type: 'clearUnread' }]);

    result = reduce(result.state, { type: 'jumpToNewestClicked', unitCount: 3 });
    expect(result.effects).toEqual([{ type: 'clearUnread' }, { type: 'snapToLastIndex' }]);
  });
});

const effectTypes = (effects: ScrollEffect[]) => effects.map((effect) => effect.type);
const hasEffect = (effects: ScrollEffect[], type: ScrollEffect['type']) =>
  effects.some((effect) => effect.type === type);
const visibleScrollEffects = (effects: ScrollEffect[]) =>
  effects.filter((effect) => effect.type === 'snapToLastIndex' || effect.type === 'writeDomBottom');

const finiteNumber = fc.integer({ min: 0, max: 100_000 });
const snapshotArb: fc.Arbitrary<ScrollSnapshot> = fc
  .record({
    scrollHeight: fc.integer({ min: 0, max: 100_000 }),
    clientHeight: fc.integer({ min: 1, max: 5_000 }),
  })
  .chain(({ scrollHeight, clientHeight }) =>
    fc.record({
      scrollTop: fc.integer({ min: 0, max: Math.max(0, scrollHeight) }),
      scrollHeight: fc.constant(scrollHeight),
      clientHeight: fc.constant(clientHeight),
    }),
  );

const stateArb: fc.Arbitrary<ScrollMachineState> = fc.record({
  conversationId: fc.option(fc.string({ minLength: 1, maxLength: 12 }), { nil: undefined }),
  hasMeasuredConversation: fc.boolean(),
  prevTotalHeight: finiteNumber,
  prevScrollHeight: finiteNumber,
  prevClientHeight: finiteNumber,
  hasSeenContent: fc.boolean(),
  hasUserEngaged: fc.boolean(),
  touchActive: fc.boolean(),
  touchMovedAfterStart: fc.boolean(),
  lastUpwardScrollAt: finiteNumber,
  lastTouchGestureAt: finiteNumber,
  lastScrollTop: finiteNumber,
  settleDeadline: finiteNumber,
});

const eventArb: fc.Arbitrary<ScrollEvent> = fc.oneof(
  fc.record({ type: fc.constant('scrollerAttached' as const), snapshot: snapshotArb }),
  fc.record({ type: fc.constant('atBottomChanged' as const), atBottom: fc.boolean() }),
  fc.constant({ type: 'pointerDown' } as const),
  fc.constant({ type: 'touchStart' } as const),
  fc.record({ type: fc.constant('touchMove' as const), nowMs: finiteNumber }),
  fc.record({ type: fc.constant('touchEnd' as const), remainingTouches: fc.integer({ min: 0, max: 5 }), nowMs: finiteNumber }),
  fc.record({ type: fc.constant('wheel' as const), deltaY: fc.integer({ min: -2_000, max: 2_000 }), nowMs: finiteNumber }),
  fc.record({ type: fc.constant('scroll' as const), snapshot: snapshotArb, nowMs: finiteNumber }),
  fc.record({
    type: fc.constant('totalHeightChanged' as const),
    conversationId: fc.option(fc.string({ minLength: 1, maxLength: 12 }), { nil: undefined }),
    totalHeight: finiteNumber,
    unitCount: fc.integer({ min: 0, max: 200 }),
    snapshot: fc.option(snapshotArb, { nil: null }),
    tailActivity: fc.constantFrom<TailActivity>('none', 'active'),
    nowMs: finiteNumber,
  }),
  fc.record({ type: fc.constant('settleTick' as const), snapshot: fc.option(snapshotArb, { nil: null }), nowMs: finiteNumber }),
  fc.constant({ type: 'navJump' } as const),
  fc.record({ type: fc.constant('jumpToNewestClicked' as const), unitCount: fc.integer({ min: 0, max: 200 }) }),
);

const measuredEngagedState = (overrides: Partial<ScrollMachineState> = {}): ScrollMachineState => ({
  conversationId: 'conv',
  hasMeasuredConversation: true,
  prevTotalHeight: 1_000,
  prevScrollHeight: 1_000,
  prevClientHeight: 400,
  hasSeenContent: true,
  hasUserEngaged: true,
  touchActive: false,
  touchMovedAfterStart: false,
  lastUpwardScrollAt: 0,
  lastTouchGestureAt: 0,
  lastScrollTop: 600,
  settleDeadline: 0,
  ...overrides,
});

describe('scrollMachine fast-check properties', () => {
  it('preserves global effect and state-shape safety', () => {
    fc.assert(
      fc.property(stateArb, eventArb, (state, event) => {
        const result = reduceScrollMachine(state, event);
        const types = effectTypes(result.effects);

        expect(Number.isFinite(result.state.settleDeadline)).toBe(true);
        expect(Number.isFinite(result.state.prevTotalHeight)).toBe(true);
        expect(Number.isFinite(result.state.prevScrollHeight)).toBe(true);
        expect(Number.isFinite(result.state.prevClientHeight)).toBe(true);
        expect(Number.isFinite(result.state.lastTouchGestureAt)).toBe(true);
        expect(typeof result.state.touchMovedAfterStart).toBe('boolean');
        expect(!(types.includes('snapToLastIndex') && types.includes('writeDomBottom'))).toBe(true);
        expect(!(types.includes('showUnread') && types.includes('clearUnread'))).toBe(true);
        expect(visibleScrollEffects(result.effects).length).toBeLessThanOrEqual(1);
      }),
      { numRuns: 1_000 },
    );
  });

  it('never scrolls to the last item for empty height changes or empty jump-to-newest', () => {
    fc.assert(
      fc.property(stateArb, snapshotArb, finiteNumber, finiteNumber, (state, snapshot, totalHeight, nowMs) => {
        const heightResult = reduceScrollMachine(state, {
          type: 'totalHeightChanged',
          conversationId: state.conversationId,
          totalHeight,
          unitCount: 0,
          snapshot,
          tailActivity: 'active',
          nowMs,
        });
        const jumpResult = reduceScrollMachine({ ...state, hasSeenContent: false }, { type: 'jumpToNewestClicked', unitCount: 0 });

        expect(hasEffect(heightResult.effects, 'snapToLastIndex')).toBe(false);
        expect(hasEffect(jumpResult.effects, 'snapToLastIndex')).toBe(false);
      }),
      { numRuns: 500 },
    );
  });

  it('does not scroll engaged users who were not pinned before tail growth', () => {
    fc.assert(
      fc.property(
        fc.integer({ min: PIN_TO_BOTTOM_THRESHOLD + 1, max: 20_000 }),
        fc.integer({ min: 1, max: 2_000 }),
        fc.integer({ min: 1, max: 10_000 }),
        finiteNumber,
        (distanceFromBottom, clientHeight, heightDelta, nowMs) => {
          const scrollTop = 1_000;
          const state = measuredEngagedState({
            prevScrollHeight: scrollTop + clientHeight + distanceFromBottom,
            prevClientHeight: clientHeight,
            prevTotalHeight: 10_000,
          });
          const result = reduceScrollMachine(state, {
            type: 'totalHeightChanged',
            conversationId: state.conversationId,
            totalHeight: state.prevTotalHeight + heightDelta,
            unitCount: 5,
            snapshot: snap(state.prevScrollHeight + heightDelta, scrollTop, clientHeight),
            tailActivity: 'active',
            nowMs,
          });

          expect(hasEffect(result.effects, 'snapToLastIndex')).toBe(false);
          expect(hasEffect(result.effects, 'showUnread')).toBe(true);
        },
      ),
      { numRuns: 500 },
    );
  });

  it('lets active upward gesture suppression override pinned proximity', () => {
    fc.assert(
      fc.property(fc.integer({ min: 0, max: PIN_TO_BOTTOM_THRESHOLD }), finiteNumber, (distanceFromBottom, nowMs) => {
        const clientHeight = 400;
        const scrollTop = 1_000;
        const state = measuredEngagedState({
          prevScrollHeight: scrollTop + clientHeight + distanceFromBottom,
          prevClientHeight: clientHeight,
          prevTotalHeight: 2_000,
          lastUpwardScrollAt: nowMs,
        });
        const result = reduceScrollMachine(state, {
          type: 'totalHeightChanged',
          conversationId: state.conversationId,
          totalHeight: state.prevTotalHeight + 100,
          unitCount: 5,
          snapshot: snap(state.prevScrollHeight + 100, scrollTop, clientHeight),
          tailActivity: 'active',
          nowMs: nowMs + USER_SCROLL_SUPPRESS_MS - 1,
        });

        expect(hasEffect(result.effects, 'snapToLastIndex')).toBe(false);
        expect(hasEffect(result.effects, 'showUnread')).toBe(true);
      }),
      { numRuns: 500 },
    );
  });

  it('ignores non-tail growth while not pinned', () => {
    fc.assert(
      fc.property(fc.integer({ min: PIN_TO_BOTTOM_THRESHOLD + 1, max: 20_000 }), finiteNumber, (distanceFromBottom, nowMs) => {
        const clientHeight = 400;
        const scrollTop = 1_000;
        const state = measuredEngagedState({
          prevScrollHeight: scrollTop + clientHeight + distanceFromBottom,
          prevClientHeight: clientHeight,
          prevTotalHeight: 2_000,
        });
        const result = reduceScrollMachine(state, {
          type: 'totalHeightChanged',
          conversationId: state.conversationId,
          totalHeight: state.prevTotalHeight + 100,
          unitCount: 5,
          snapshot: snap(state.prevScrollHeight + 100, scrollTop, clientHeight),
          tailActivity: 'none',
          nowMs,
        });

        expect(visibleScrollEffects(result.effects)).toEqual([]);
        expect(hasEffect(result.effects, 'showUnread')).toBe(false);
      }),
      { numRuns: 500 },
    );
  });

  it('scrolls pinned, unsuppressed, engaged users to newest on genuine tail growth', () => {
    fc.assert(
      fc.property(fc.integer({ min: 0, max: PIN_TO_BOTTOM_THRESHOLD }), finiteNumber, (distanceFromBottom, nowMs) => {
        const clientHeight = 400;
        const scrollTop = 1_000;
        const state = measuredEngagedState({
          prevScrollHeight: scrollTop + clientHeight + distanceFromBottom,
          prevClientHeight: clientHeight,
          prevTotalHeight: 2_000,
          lastUpwardScrollAt: 0,
          touchActive: false,
        });
        const result = reduceScrollMachine(state, {
          type: 'totalHeightChanged',
          conversationId: state.conversationId,
          totalHeight: state.prevTotalHeight + 100,
          unitCount: 5,
          snapshot: snap(state.prevScrollHeight + 100, scrollTop, clientHeight),
          tailActivity: 'active',
          nowMs: Math.max(nowMs, USER_SCROLL_SUPPRESS_MS + 1),
        });

        expect(hasEffect(result.effects, 'snapToLastIndex')).toBe(true);
      }),
      { numRuns: 500 },
    );
  });

  it('conversation identity changes reset baselines, gestures, suppression, and engagement', () => {
    fc.assert(
      fc.property(stateArb, snapshotArb, fc.integer({ min: 0, max: 200 }), finiteNumber, (state, snapshot, unitCount, nowMs) => {
        const result = reduceScrollMachine(
          { ...state, conversationId: 'old', hasMeasuredConversation: true, hasUserEngaged: true, touchActive: true, touchMovedAfterStart: true, lastUpwardScrollAt: nowMs, lastTouchGestureAt: nowMs },
          {
            type: 'totalHeightChanged',
            conversationId: 'new',
            totalHeight: snapshot.scrollHeight,
            unitCount,
            snapshot,
            tailActivity: 'active',
            nowMs,
          },
        );

        expect(result.state.conversationId).toBe('new');
        expect(result.state.prevScrollHeight).toBe(snapshot.scrollHeight);
        expect(result.state.prevClientHeight).toBe(snapshot.clientHeight);
        expect(result.state.prevTotalHeight).toBe(snapshot.scrollHeight);
        expect(result.state.hasUserEngaged).toBe(false);
        expect(result.state.touchActive).toBe(false);
        expect(result.state.touchMovedAfterStart).toBe(false);
        expect(result.state.lastUpwardScrollAt).toBe(0);
        expect(result.state.lastTouchGestureAt).toBe(0);
      }),
      { numRuns: 500 },
    );
  });

  it('engagement remains sticky until scroller or conversation reset', () => {
    const nonResetEventArb = eventArb.filter(
      (event) => event.type !== 'scrollerAttached' && event.type !== 'totalHeightChanged',
    );
    fc.assert(
      fc.property(fc.array(nonResetEventArb, { minLength: 1, maxLength: 50 }), (events) => {
        let state = measuredEngagedState({ hasUserEngaged: true });
        for (const event of events) {
          state = reduceScrollMachine(state, event).state;
          expect(state.hasUserEngaged).toBe(true);
        }
      }),
      { numRuns: 300 },
    );
  });

  it('upward movement refreshes suppression and non-upward movement does not', () => {
    fc.assert(
      fc.property(finiteNumber, finiteNumber, (previousUpwardAt, nowMs) => {
        const base = measuredEngagedState({ lastUpwardScrollAt: previousUpwardAt, lastScrollTop: 100 });

        expect(reduceScrollMachine(base, { type: 'wheel', deltaY: -1, nowMs }).state.lastUpwardScrollAt).toBe(nowMs);
        expect(reduceScrollMachine(base, { type: 'wheel', deltaY: 1, nowMs }).state.lastUpwardScrollAt).toBe(previousUpwardAt);
        expect(reduceScrollMachine(base, { type: 'scroll', snapshot: snap(1_000, 99, 400), nowMs }).state.lastUpwardScrollAt).toBe(nowMs);
        expect(reduceScrollMachine(base, { type: 'scroll', snapshot: snap(1_000, 101, 400), nowMs }).state.lastUpwardScrollAt).toBe(previousUpwardAt);
      }),
      { numRuns: 500 },
    );
  });

  it('uses DOM scrollHeight rather than virtualizer total height for pin distance', () => {
    fc.assert(
      fc.property(fc.integer({ min: 0, max: PIN_TO_BOTTOM_THRESHOLD }), finiteNumber, (domDistance, nowMs) => {
        const clientHeight = 400;
        const scrollTop = 1_000;
        const state = measuredEngagedState({
          prevScrollHeight: scrollTop + clientHeight + domDistance,
          prevClientHeight: clientHeight,
          prevTotalHeight: 100_000,
        });
        const result = reduceScrollMachine(state, {
          type: 'totalHeightChanged',
          conversationId: state.conversationId,
          totalHeight: 100_050,
          unitCount: 5,
          snapshot: snap(state.prevScrollHeight + 50, scrollTop, clientHeight),
          tailActivity: 'active',
          nowMs: Math.max(nowMs, USER_SCROLL_SUPPRESS_MS + 1),
        });

        expect(hasEffect(result.effects, 'snapToLastIndex')).toBe(true);
      }),
      { numRuns: 500 },
    );
  });

  it('uses previous clientHeight to preserve pinned classification across viewport shrink', () => {
    fc.assert(
      fc.property(fc.integer({ min: 1, max: 300 }), finiteNumber, (shrinkBy, nowMs) => {
        const previousClientHeight = 600;
        const currentClientHeight = previousClientHeight - shrinkBy;
        const scrollTop = 1_000;
        const state = measuredEngagedState({
          prevScrollHeight: scrollTop + previousClientHeight,
          prevClientHeight: previousClientHeight,
          prevTotalHeight: 2_000,
        });
        const result = reduceScrollMachine(state, {
          type: 'totalHeightChanged',
          conversationId: state.conversationId,
          totalHeight: state.prevTotalHeight + 100,
          unitCount: 5,
          snapshot: snap(state.prevScrollHeight + 100, scrollTop, currentClientHeight),
          tailActivity: 'active',
          nowMs: Math.max(nowMs, USER_SCROLL_SUPPRESS_MS + 1),
        });

        expect(hasEffect(result.effects, 'snapToLastIndex')).toBe(true);
      }),
      { numRuns: 500 },
    );
  });

  it('settling past its deadline stops instead of correcting scroll', () => {
    fc.assert(
      fc.property(finiteNumber, snapshotArb, (deadline, snapshot) => {
        const state = measuredEngagedState({ hasUserEngaged: false, settleDeadline: deadline });
        const result = reduceScrollMachine(state, { type: 'settleTick', snapshot, nowMs: deadline + 1 });

        expect(result.effects).toEqual([{ type: 'stopSettleWatch' }]);
      }),
      { numRuns: 500 },
    );
  });

  it('user engagement makes the next settle tick stop instead of correcting scroll', () => {
    fc.assert(
      fc.property(finiteNumber, (nowMs) => {
        const state = measuredEngagedState({ hasUserEngaged: false, settleDeadline: nowMs + SETTLE_WATCH_MS });
        const engaged = reduceScrollMachine(state, { type: 'pointerDown' }).state;
        const result = reduceScrollMachine(engaged, { type: 'settleTick', snapshot: snap(1_000, 100, 400), nowMs });

        expect(result.effects).toEqual([{ type: 'stopSettleWatch' }]);
      }),
      { numRuns: 500 },
    );
  });
});
