import { describe, expect, it } from 'vitest';
import {
  SETTLE_WATCH_MS,
  initialScrollMachineState,
  reduceScrollMachine,
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

    result = reduce(result.state, { type: 'touchEnd', remainingTouches: 0 });
    result = reduce(result.state, { type: 'scroll', snapshot: snap(600, 60, 400), nowMs: 1150 });
    result = reduce(result.state, measured({ totalHeight: 700, snapshot: snap(700, 60, 400), nowMs: 1200 }));
    expect(result.effects.map((e) => e.type)).not.toContain('snapToLastIndex');

    result = reduce(result.state, measured({ totalHeight: 800, snapshot: snap(800, 300, 400), nowMs: 1700 }));
    expect(result.effects).toContainEqual({ type: 'snapToLastIndex' });
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

  it('clears unread on at-bottom and jump-to-newest events', () => {
    let result = reduce(initialScrollMachineState(), { type: 'atBottomChanged', atBottom: true });
    expect(result.effects).toEqual([{ type: 'clearUnread' }]);

    result = reduce(result.state, { type: 'jumpToNewestClicked', unitCount: 3 });
    expect(result.effects).toEqual([{ type: 'clearUnread' }, { type: 'snapToLastIndex' }]);
  });
});
