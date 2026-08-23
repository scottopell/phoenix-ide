import { type ReactElement } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { GESTURE_STALE_MS, VirtualTranscript, type VirtualTranscriptHandle, type VirtualTranscriptPhysicalSnapshot } from './VirtualTranscript';

interface TestItem {
  id: string;
  label: string;
  height: number;
}

type ResizeObserverCallback = ConstructorParameters<typeof ResizeObserver>[0];

const resizeObservers: TestResizeObserver[] = [];

class TestResizeObserver implements ResizeObserver {
  readonly callback: ResizeObserverCallback;
  readonly elements = new Set<Element>();

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    resizeObservers.push(this);
  }

  observe(element: Element): void {
    this.elements.add(element);
  }

  unobserve(element: Element): void {
    this.elements.delete(element);
  }

  disconnect(): void {
    this.elements.clear();
  }

  trigger(height: number): void {
    this.triggerEntries([...this.elements].map((target) => [target, height]));
  }

  triggerEntries(entries: Array<[Element, number]>): void {
    this.callback(entries.map(([target, height]) => ({
      target,
      contentRect: { height } as DOMRectReadOnly,
    })) as ResizeObserverEntry[], this);
  }
}

function heightFromElement(element: Element): number {
  const direct = element.getAttribute('data-height');
  if (direct) return Number(direct);
  const child = element.querySelector('[data-height]');
  if (child) return Number(child.getAttribute('data-height'));
  return 0;
}

function makeItems(count: number, height = 20): TestItem[] {
  return Array.from({ length: count }, (_, index) => ({
    id: `item-${index}`,
    label: `Item ${index}`,
    height,
  }));
}

function renderRow(item: TestItem): ReactElement {
  return (
    <div data-testid={`payload-${item.id}`} data-height={item.height}>
      {item.label}
    </div>
  );
}

function virtualRows(): HTMLElement[] {
  return screen.queryAllByText(/Item /).map((element) => {
    const row = element.closest('[data-virtual-index]');
    if (!(row instanceof HTMLElement)) throw new Error('row not found');
    return row;
  });
}

function rowIndexes(): number[] {
  return virtualRows().map((row) => Number(row.dataset['virtualIndex']));
}

function scrollTopOf(element: HTMLDivElement | null): number | undefined {
  return element?.scrollTop;
}

beforeEach(() => {
  resizeObservers.length = 0;
  vi.stubGlobal('ResizeObserver', TestResizeObserver);

  Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
    configurable: true,
    get() {
      if (this.classList.contains('virtual-transcript')) return 100;
      return heightFromElement(this);
    },
  });

  HTMLElement.prototype.getBoundingClientRect = function getBoundingClientRect() {
    const height = heightFromElement(this);
    return {
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      bottom: height,
      right: 0,
      width: 320,
      height,
      toJSON: () => ({}),
    } as DOMRect;
  };
});

describe('VirtualTranscript', () => {
  it('renders a bounded contiguous range with top and bottom spacers', () => {
    const ranges: VirtualTranscriptPhysicalSnapshot[] = [];

    render(
      <VirtualTranscript
        items={makeItems(100)}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={20}
        initialTail={false}
        renderItem={renderRow}
        onRangeChange={(snapshot) => ranges.push(snapshot)}
      />,
    );

    const indexes = rowIndexes();
    expect(indexes).toEqual([0, 1, 2, 3, 4, 5]);
    expect(indexes.length).toBeLessThan(100);
    expect(ranges.at(-1)?.renderedRange).toEqual({ startIndex: 0, endIndex: 5 });
    expect(ranges.at(-1)?.layoutRevision).toBeGreaterThan(0);

    const scroller = document.querySelector('.virtual-transcript');
    expect(scroller).toBeInstanceOf(HTMLElement);
    expect(getComputedStyle(scroller as Element).overflowAnchor).toBe('none');
  });

  it('quarantines duplicate semantic keys into independent physical rows', () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    const items = [
      { id: 'same', label: 'Item first', height: 20 },
      { id: 'same', label: 'Item second', height: 60 },
      { id: 'same', label: 'Item third', height: 30 },
      { id: 'same\u0000duplicate:1', label: 'Reserved suffix', height: 20 },
      ...makeItems(10, 20),
    ];

    render(
      <VirtualTranscript
        ref={ref}
        items={items}
        getKey={(item) => item.id}
        estimatedExtent={(item) => item.height}
        overscan={200}
        initialTail={false}
        renderItem={renderRow}
        scrollerRef={(element) => { scroller = element; }}
      />,
    );

    const mountedRows = Array.from(document.querySelectorAll<HTMLElement>('[data-virtual-index]'));
    const duplicateRows = mountedRows.slice(0, 3);
    const physicalKeys = mountedRows.slice(0, 4).map((row) => row.dataset['virtualKey']);
    expect(new Set(physicalKeys).size).toBe(4);
    expect(physicalKeys[0]).toBe('same');
    expect(physicalKeys[1]).toContain('duplicate:2');
    expect(physicalKeys[2]).toContain('duplicate:3');
    expect(physicalKeys[3]).toBe('same\u0000duplicate:1');
    expect(error).toHaveBeenCalledWith(
      '[VirtualTranscript] duplicate semantic keys quarantined',
      { duplicateKeys: ['same'] },
    );

    act(() => ref.current?.scrollToIndex(4, 'start'));
    const anchoredTop = scrollTopOf(scroller);
    act(() => resizeObservers[0]!.triggerEntries([
      [duplicateRows[0]!, 25],
      [duplicateRows[1]!, 70],
      [duplicateRows[2]!, 35],
    ]));
    expect(scrollTopOf(scroller)).toBe((anchoredTop ?? 0) + 20);
    expect(ref.current?.captureVisibleAnchor()?.key).toBe('item-0');
  });

  it('initially tails and reports signed physical anchor offsets', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;

    render(
      <VirtualTranscript
        ref={ref}
        items={makeItems(20, 10)}
        getKey={(item) => item.id}
        estimatedExtent={10}
        overscan={0}
        initialTail
        renderItem={renderRow}
        scrollerRef={(element) => { scroller = element; }}
      />,
    );

    expect(scrollTopOf(scroller)).toBe(100);
    expect(rowIndexes()).toEqual([10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);

    act(() => ref.current?.scrollToIndex(10, 'start', 25));

    expect(scrollTopOf(scroller)).toBe(75);
    let anchor = null as ReturnType<VirtualTranscriptHandle['captureVisibleAnchor']>;
    act(() => {
      anchor = ref.current?.captureVisibleAnchor() ?? null;
    });
    expect(anchor).toEqual({
      index: 7,
      key: 'item-7',
      offset: -5,
    });
  });

  it('positions an intra-row target through the transcript executor', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;

    render(
      <VirtualTranscript
        ref={ref}
        items={[{ id: 'group', label: 'Group', height: 200 }]}
        getKey={(item) => item.id}
        estimatedExtent={200}
        overscan={0}
        initialTail={false}
        renderItem={(item) => (
          <div data-height={item.height}>
            <div data-member="first">First</div>
            <div data-member="second">Second</div>
          </div>
        )}
        scrollerRef={(element) => { scroller = element; }}
      />,
    );

    const row = screen.getByText('First').closest('[data-virtual-index]') as HTMLElement;
    const second = screen.getByText('Second');
    vi.spyOn(row, 'getBoundingClientRect').mockReturnValue({ top: 20 } as DOMRect);
    vi.spyOn(second, 'getBoundingClientRect').mockReturnValue({ top: 80 } as DOMRect);

    act(() => ref.current?.scrollToIndex(0, 'start', 0, '[data-member="second"]'));

    expect(scrollTopOf(scroller)).toBe(60);
    expect(ref.current?.physicalSnapshot(0, '[data-member="second"]')).toMatchObject({
      targetIndex: 0,
      targetOffset: 0,
      targetMeasured: true,
    });
  });

  it('compensates scrollTop when a measured row above the top-edge anchor resizes', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    const items = makeItems(30, 20);
    const estimate = (item: TestItem) => item.height;

    const view = render(
      <VirtualTranscript
        ref={ref}
        items={items}
        getKey={(item) => item.id}
        estimatedExtent={estimate}
        overscan={200}
        initialTail={false}
        renderItem={renderRow}
        scrollerRef={(element) => { scroller = element; }}
      />,
    );

    act(() => ref.current?.scrollToIndex(5, 'start'));
    expect(scrollTopOf(scroller)).toBe(100);

    const resized = items.map((item) => item.id === 'item-2'
      ? { ...item, height: 50 }
      : item);

    act(() => {
      view.rerender(
        <VirtualTranscript
          ref={ref}
          items={resized}
          getKey={(item) => item.id}
          estimatedExtent={estimate}
          overscan={200}
          initialTail={false}
          renderItem={renderRow}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );
    });

    const row2 = document.querySelector<HTMLElement>('[data-virtual-key="item-2"]')!;
    act(() => resizeObservers[0]!.triggerEntries([[row2, 50]]));

    expect(scrollTopOf(scroller)).toBe(130);
    let anchor = null as ReturnType<VirtualTranscriptHandle['captureVisibleAnchor']>;
    act(() => {
      anchor = ref.current?.captureVisibleAnchor() ?? null;
    });
    expect(anchor).toEqual({
      index: 5,
      key: 'item-5',
      offset: 0,
    });
  });

  it('absorbs above-anchor resize into the top spacer mid-scroll and reconciles scrollTop once settled', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;
      const items = makeItems(30, 20);

      render(
        <VirtualTranscript
          ref={ref}
          items={items}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={200}
          initialTail={false}
          renderItem={renderRow}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      act(() => ref.current?.scrollToIndex(20, 'start'));
      expect(scrollTopOf(scroller)).toBe(400);
      // The write's own echo is recognized as programmatic, not user motion.
      expect(ref.current?.isProgrammaticScroll(400)).toBe(true);

      // A user scroll event (non-matching scrollTop) marks scrolling as
      // in-flight; a resize of a mounted row above the anchor must then keep
      // scrollTop untouched (momentum-preserving) while the top spacer
      // absorbs the delta.
      act(() => {
        scroller!.scrollTop = 395;
        fireEvent.scroll(scroller!);
      });
      expect(ref.current?.isProgrammaticScroll(395)).toBe(false);
      const row12 = document.querySelector<HTMLElement>('[data-virtual-key="item-12"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[row12, 50]]));
      expect(scrollTopOf(scroller)).toBe(395);

      const spacer = document.querySelector<HTMLElement>('.virtual-transcript__spacer')!;
      expect(spacer.style.height).toBe('190px');

      let anchor = null as ReturnType<VirtualTranscriptHandle['captureVisibleAnchor']>;
      act(() => {
        anchor = ref.current?.captureVisibleAnchor() ?? null;
      });
      expect(anchor).toMatchObject({ index: 19, key: 'item-19', offset: -15 });

      // Once scrolling settles, drift reconciles into true layout coordinates
      // with a single scrollTop adjustment.
      act(() => {
        vi.advanceTimersByTime(400);
      });
      expect(scrollTopOf(scroller)).toBe(425);
      act(() => {
        anchor = ref.current?.captureVisibleAnchor() ?? null;
      });
      expect(anchor).toMatchObject({ index: 19, key: 'item-19', offset: -15 });
    } finally {
      vi.useRealTimers();
    }
  });

  it('defers drift reconciliation while a touch is held, then reconciles after lift', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;

      render(
        <VirtualTranscript
          ref={ref}
          items={makeItems(30, 20)}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={200}
          initialTail={false}
          renderItem={renderRow}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      act(() => ref.current?.scrollToIndex(20, 'start'));
      act(() => {
        fireEvent.touchStart(scroller!, { touches: [{ identifier: 1 }], changedTouches: [{ identifier: 1 }] });
        scroller!.scrollTop = 395;
        fireEvent.scroll(scroller!);
      });
      const row12 = document.querySelector<HTMLElement>('[data-virtual-key="item-12"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[row12, 50]]));
      expect(scrollTopOf(scroller)).toBe(395);

      // A stationary held finger keeps the gesture active even with no
      // scroll events: reconciliation must not write scrollTop mid-gesture.
      act(() => {
        vi.advanceTimersByTime(1000);
      });
      expect(scrollTopOf(scroller)).toBe(395);

      act(() => {
        fireEvent.touchEnd(scroller!, { touches: [], changedTouches: [{ identifier: 1 }] });
      });
      act(() => {
        vi.advanceTimersByTime(400);
      });
      expect(scrollTopOf(scroller)).toBe(425);
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps reconciliation deferred while a second finger stays down on another row', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;

      render(
        <VirtualTranscript
          ref={ref}
          items={makeItems(30, 20)}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={200}
          initialTail={false}
          renderItem={renderRow}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      act(() => ref.current?.scrollToIndex(20, 'start'));
      // Two fingers land on different rows, so their touch events target
      // different descendants of the scroller.
      const rowA = document.querySelector<HTMLElement>('[data-virtual-key="item-18"]')!;
      const rowB = document.querySelector<HTMLElement>('[data-virtual-key="item-20"]')!;
      act(() => {
        fireEvent.touchStart(rowA, { touches: [{ identifier: 1 }], changedTouches: [{ identifier: 1 }] });
        fireEvent.touchStart(rowB, {
          touches: [{ identifier: 1 }, { identifier: 2 }],
          changedTouches: [{ identifier: 2 }],
        });
        scroller!.scrollTop = 395;
        fireEvent.scroll(scroller!);
      });
      const row12 = document.querySelector<HTMLElement>('[data-virtual-key="item-12"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[row12, 50]]));
      expect(scrollTopOf(scroller)).toBe(395);

      // First finger lifts; the second is still down, so the gesture is not
      // over and reconciliation must stay deferred.
      act(() => {
        fireEvent.touchEnd(rowA, { touches: [{ identifier: 2 }], changedTouches: [{ identifier: 1 }] });
      });
      act(() => {
        vi.advanceTimersByTime(1000);
      });
      expect(scrollTopOf(scroller)).toBe(395);

      act(() => {
        fireEvent.touchEnd(rowB, { touches: [], changedTouches: [{ identifier: 2 }] });
      });
      act(() => {
        vi.advanceTimersByTime(400);
      });
      expect(scrollTopOf(scroller)).toBe(425);
    } finally {
      vi.useRealTimers();
    }
  });

  it('reconciles within the stale bound when a touch lift is unobservable', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;

      render(
        <VirtualTranscript
          ref={ref}
          items={makeItems(30, 20)}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={200}
          initialTail={false}
          renderItem={renderRow}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      act(() => ref.current?.scrollToIndex(20, 'start'));
      const row = document.querySelector<HTMLElement>('[data-virtual-key="item-20"]')!;
      act(() => {
        fireEvent.touchStart(row, { touches: [{ identifier: 5 }], changedTouches: [{ identifier: 5 }] });
        scroller!.scrollTop = 395;
        fireEvent.scroll(scroller!);
      });
      const row12 = document.querySelector<HTMLElement>('[data-virtual-key="item-12"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[row12, 50]]));
      expect(scrollTopOf(scroller)).toBe(395);

      // Virtualization detaches the touched row and the finger lifts. That
      // touchend is dispatched at the detached node and reaches no listener
      // anywhere — verified against Chromium — and pointer events are no
      // help either, being cancelled when the pan took over and never
      // reporting the lift. Reconciliation must therefore be bounded rather
      // than waiting on an event that will never arrive.
      row.remove();
      act(() => {
        vi.advanceTimersByTime(400);
      });
      expect(scrollTopOf(scroller)).toBe(395);

      act(() => {
        vi.advanceTimersByTime(GESTURE_STALE_MS);
      });
      expect(scrollTopOf(scroller)).toBe(425);
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not tail-follow while the policy withholds the intent from a still-pinned viewport', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;

    render(
      <VirtualTranscript
        ref={ref}
        items={makeItems(30, 20)}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={200}
        initialTail
        renderItem={renderRow}
        scrollerRef={(element) => { scroller = element; }}
      />,
    );

    const pinnedTop = scrollTopOf(scroller);
    // The finger goes down at the tail and starts dragging. No scroll event
    // has landed yet and the position is still inside the pinned epsilon, so
    // the physical layer still considers itself pinned. The policy is what
    // notices the moved touch and withdraws the tail-follow grant; the
    // physical layer never infers that from its own geometry.
    const row = document.querySelector<HTMLElement>('[data-virtual-key="item-29"]')!;
    act(() => {
      fireEvent.touchStart(row, { touches: [{ identifier: 3 }], changedTouches: [{ identifier: 3 }] });
      fireEvent.touchMove(row, { touches: [{ identifier: 3 }] });
      ref.current?.setTailFollowAllowed(false);
    });

    // A mounted row above the viewport grows in that window. Tail-following
    // here would write scrollTop and snap the nascent drag to the bottom;
    // holding the anchor absorbs the growth into the spacer instead.
    const grown = document.querySelector<HTMLElement>('[data-virtual-key="item-16"]')!;
    expect(grown).not.toBeNull();
    act(() => resizeObservers[0]!.triggerEntries([[grown, 60]]));
    expect(scrollTopOf(scroller)).toBe(pinnedTop);

    // The growth left the viewport genuinely off the tail, so holding
    // position stays correct until something returns it there. Once the
    // policy restores the grant and the viewport is pinned again,
    // tail-following resumes rather than being disabled for good.
    act(() => {
      fireEvent.touchEnd(row, { touches: [], changedTouches: [{ identifier: 3 }] });
      ref.current?.setTailFollowAllowed(true);
    });
    act(() => ref.current?.scrollToTail());
    const repinnedTop = scrollTopOf(scroller);
    const grownAgain = document.querySelector<HTMLElement>('[data-virtual-key="item-17"]')!;
    expect(grownAgain).not.toBeNull();
    act(() => resizeObservers[0]!.triggerEntries([[grownAgain, 60]]));
    expect(scrollTopOf(scroller)).toBeGreaterThan(repinnedTop ?? 0);
  });

  it('never publishes a pinned state the reconciled position does not hold', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;
      const pinnedStates: boolean[] = [];

      render(
        <VirtualTranscript
          ref={ref}
          items={makeItems(30, 50)}
          getKey={(item) => item.id}
          estimatedExtent={50}
          overscan={2000}
          initialTail={false}
          renderItem={renderRow}
          onPinnedChange={(pinned) => pinnedStates.push(pinned)}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      // Reading 20px off the tail: 1500 of content, a 100px viewport, so the
      // maximum scroll position is 1400.
      act(() => {
        scroller!.scrollTop = 1380;
        fireEvent.scroll(scroller!);
      });
      const row = document.querySelector<HTMLElement>('[data-virtual-key="item-27"]')!;
      act(() => {
        fireEvent.touchStart(row, { touches: [{ identifier: 9 }], changedTouches: [{ identifier: 9 }] });
      });
      pinnedStates.length = 0;

      // A row above the anchor shrinks by 30. The gesture defers the
      // correction into the spacer, which keeps the total extent — and so the
      // pinned answer — exactly where it was.
      const above = document.querySelector<HTMLElement>('[data-virtual-key="item-5"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[above, 20]]));
      expect(scrollTopOf(scroller)).toBe(1380);

      // Reconciliation removes the spacer shift and writes the equivalent
      // scroll position. Those are two halves of one position-preserving
      // operation: between them the extent has shrunk while the viewport has
      // not yet moved, which reads as pinned even though the reader stays
      // 20px off the tail throughout. Publishing that intermediate would hand
      // tail-follow back to a reader who never asked for it.
      act(() => {
        fireEvent.touchEnd(row, { touches: [], changedTouches: [{ identifier: 9 }] });
        vi.advanceTimersByTime(GESTURE_STALE_MS + 500);
      });

      expect(scrollTopOf(scroller)).toBe(1350);
      expect(pinnedStates).not.toContain(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('never publishes a visible range the reconciled position does not hold', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;
      const ranges: Array<{ startIndex: number; endIndex: number } | null> = [];

      render(
        <VirtualTranscript
          ref={ref}
          items={makeItems(30, 50)}
          getKey={(item) => item.id}
          estimatedExtent={50}
          overscan={2000}
          initialTail={false}
          renderItem={renderRow}
          onRangeChange={(snapshot) => ranges.push(snapshot.visibleRange)}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      act(() => {
        scroller!.scrollTop = 1380;
        fireEvent.scroll(scroller!);
      });
      const row = document.querySelector<HTMLElement>('[data-virtual-key="item-27"]')!;
      act(() => {
        fireEvent.touchStart(row, { touches: [{ identifier: 4 }], changedTouches: [{ identifier: 4 }] });
      });
      const settled = ranges.at(-1);
      ranges.length = 0;

      // The same position-preserving correction as the pinned case. The
      // reader sees the same rows throughout, so the published range must say
      // so at every step: an intermediate range is evidence a positioning
      // command can be acknowledged from, and REQ-VT-005 admits only
      // observations of the position that actually holds.
      const above = document.querySelector<HTMLElement>('[data-virtual-key="item-5"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[above, 20]]));
      act(() => {
        fireEvent.touchEnd(row, { touches: [], changedTouches: [{ identifier: 4 }] });
        vi.advanceTimersByTime(GESTURE_STALE_MS + 500);
      });

      expect(scrollTopOf(scroller)).toBe(1350);
      expect(ranges.every((range) => JSON.stringify(range) === JSON.stringify(settled))).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('discards a pending correction that an absolute reposition has superseded', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;
      let followTail = false;

      render(
        <VirtualTranscript
          ref={ref}
          items={makeItems(30, 50)}
          getKey={(item) => item.id}
          estimatedExtent={50}
          overscan={2000}
          initialTail={false}
          renderItem={renderRow}
          onTotalExtentChange={() => { if (followTail) ref.current?.scrollToTail(); }}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      act(() => {
        scroller!.scrollTop = 1380;
        fireEvent.scroll(scroller!);
      });
      const row = document.querySelector<HTMLElement>('[data-virtual-key="item-27"]')!;
      act(() => {
        fireEvent.touchStart(row, { touches: [{ identifier: 7 }], changedTouches: [{ identifier: 7 }] });
      });
      const above = document.querySelector<HTMLElement>('[data-virtual-key="item-5"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[above, 20]]));

      // The gesture ends and the policy hands tail-follow back before the
      // deferred correction has been written. Clearing the drift changes the
      // total extent, and the extent callback is dispatched from an effect
      // declared ahead of reconciliation — so the tail snap lands first.
      followTail = true;
      act(() => {
        fireEvent.touchEnd(row, { touches: [], changedTouches: [{ identifier: 7 }] });
        vi.advanceTimersByTime(GESTURE_STALE_MS + 500);
      });

      // The correction was computed against the position the snap replaced.
      // Applying it on top would leave a following viewport short of the
      // tail by exactly the drift.
      const maxScrollTop = 30 * 50 - 30 - 100;
      expect(scrollTopOf(scroller)).toBe(maxScrollTop);
    } finally {
      vi.useRealTimers();
    }
  });

  it('never captures an anchor offset the reconciled position does not hold', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;

      render(
        <VirtualTranscript
          ref={ref}
          items={makeItems(30, 50)}
          getKey={(item) => item.id}
          estimatedExtent={50}
          overscan={2000}
          initialTail={false}
          renderItem={renderRow}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      act(() => {
        scroller!.scrollTop = 1380;
        fireEvent.scroll(scroller!);
      });
      const row = document.querySelector<HTMLElement>('[data-virtual-key="item-27"]')!;
      act(() => {
        fireEvent.touchStart(row, { touches: [{ identifier: 8 }], changedTouches: [{ identifier: 8 }] });
      });
      const before = ref.current?.captureVisibleAnchor() ?? null;

      const above = document.querySelector<HTMLElement>('[data-virtual-key="item-5"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[above, 20]]));
      act(() => {
        fireEvent.touchEnd(row, { touches: [], changedTouches: [{ identifier: 8 }] });
      });

      // Run the settle timer without flushing React, leaving the correction
      // computed but not yet written — the window in which a range change can
      // synchronously ask for an anchor to acquire history from.
      vi.advanceTimersByTime(GESTURE_STALE_MS + 500);
      const during = ref.current?.captureVisibleAnchor() ?? null;

      act(() => {});
      const after = ref.current?.captureVisibleAnchor() ?? null;

      // Prefix restoration replays this offset to put the same row back in the
      // same place, so an anchor taken mid-correction makes history
      // acquisition jump by exactly the drift (REQ-VT-005).
      expect(during).toEqual(before);
      expect(after).toEqual(before);
    } finally {
      vi.useRealTimers();
    }
  });

  it('reconciles immediately when the top spacer can no longer absorb the drift', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;

      render(
        <VirtualTranscript
          ref={ref}
          items={makeItems(30, 20)}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={200}
          initialTail={false}
          renderItem={renderRow}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      act(() => ref.current?.scrollToIndex(20, 'start'));
      act(() => {
        scroller!.scrollTop = 395;
        fireEvent.scroll(scroller!);
      });
      const row12 = document.querySelector<HTMLElement>('[data-virtual-key="item-12"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[row12, 50]]));
      expect(scrollTopOf(scroller)).toBe(395);

      // Continued upward scrolling reaches the top of the layout before the
      // settle timer fires: the drift is reconciled through the direct-write
      // fallback instead of clamping the spacer away from the layout model.
      act(() => {
        scroller!.scrollTop = 10;
        fireEvent.scroll(scroller!);
      });
      expect(scrollTopOf(scroller)).toBe(40);
      const spacer = document.querySelector<HTMLElement>('.virtual-transcript__spacer')!;
      expect(spacer.style.height).toBe('0px');
      let anchor = null as ReturnType<VirtualTranscriptHandle['captureVisibleAnchor']>;
      act(() => {
        anchor = ref.current?.captureVisibleAnchor() ?? null;
      });
      expect(anchor).toMatchObject({ index: 2, key: 'item-2', offset: 0 });
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not republish unchanged height or pinned state when a callback scrolls to the tail', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const totals: number[] = [];
    const pinnedStates: boolean[] = [];

    render(
      <VirtualTranscript
        ref={ref}
        items={makeItems(20, 10)}
        getKey={(item) => item.id}
        estimatedExtent={10}
        overscan={0}
        initialTail
        renderItem={renderRow}
        onTotalExtentChange={(total) => {
          totals.push(total);
          ref.current?.scrollToTail();
        }}
        onPinnedChange={(pinned) => pinnedStates.push(pinned)}
      />,
    );

    expect(totals).toEqual([200]);
    expect(pinnedStates).toEqual([false, true]);
    expect(rowIndexes()).toEqual([10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);
  });

  it('updates viewport geometry through ResizeObserver before publishing range', () => {
    const ranges: VirtualTranscriptPhysicalSnapshot[] = [];

    render(
      <VirtualTranscript
        items={makeItems(20, 20)}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={0}
        initialTail={false}
        renderItem={renderRow}
        onRangeChange={(snapshot) => ranges.push(snapshot)}
      />,
    );

    expect(ranges.at(-1)?.renderedRange).toEqual({ startIndex: 0, endIndex: 4 });

    const scroller = document.querySelector('.virtual-transcript')!;
    act(() => {
      resizeObservers.at(-1)?.triggerEntries([[scroller, 60]]);
    });

    expect(ranges.at(-1)?.renderedRange).toEqual({ startIndex: 0, endIndex: 2 });
    expect(rowIndexes()).toEqual([0, 1, 2]);
  });

  it('uses one shared ResizeObserver for scroller, header, and every mounted row, with cleanup', () => {
    const { unmount } = render(
      <VirtualTranscript
        items={makeItems(3, 20)}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={200}
        initialTail={false}
        header={<div data-testid="header" data-height={15}>Header</div>}
        renderItem={renderRow}
      />,
    );

    expect(resizeObservers).toHaveLength(1);
    const observer = resizeObservers[0]!;
    expect(observer.elements.size).toBe(5);
    expect(observer.elements.has(document.querySelector('.virtual-transcript')!)).toBe(true);
    expect(observer.elements.has(document.querySelector('[data-virtual-header]')!)).toBe(true);
    for (const row of virtualRows()) expect(observer.elements.has(row)).toBe(true);

    unmount();

    expect(observer.elements.size).toBe(0);
  });

  it('updates keyed intrinsic resize through ResizeObserver with anchor compensation', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    const totals: number[] = [];

    render(
      <VirtualTranscript
        ref={ref}
        items={makeItems(20, 20)}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={200}
        initialTail={false}
        renderItem={renderRow}
        scrollerRef={(element) => { scroller = element; }}
        onTotalExtentChange={(total) => totals.push(total)}
      />,
    );

    act(() => ref.current?.scrollToIndex(5, 'start'));
    expect(scrollTopOf(scroller)).toBe(100);

    const row2 = document.querySelector<HTMLElement>('[data-virtual-key="item-2"]')!;
    act(() => resizeObservers[0]!.triggerEntries([[row2, 50]]));

    expect(scrollTopOf(scroller)).toBe(130);
    expect(totals.at(-1)).toBe(430);
  });

  it('measures header into total extent, offsets, initial tail, and async resize geometry', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    const totals: number[] = [];

    render(
      <VirtualTranscript
        ref={ref}
        items={makeItems(20, 10)}
        getKey={(item) => item.id}
        estimatedExtent={10}
        overscan={0}
        initialTail
        header={<div data-testid="header" data-height={30}>Header</div>}
        renderItem={renderRow}
        scrollerRef={(element) => { scroller = element; }}
        onTotalExtentChange={(total) => totals.push(total)}
      />,
    );

    expect(totals.at(-1)).toBe(230);
    expect(scrollTopOf(scroller)).toBe(130);

    act(() => ref.current?.scrollToIndex(0, 'start'));
    expect(scrollTopOf(scroller)).toBe(30);
    expect(ref.current?.measureOffsetForIndex(0)).toBe(0);

    const snapshot = ref.current?.physicalSnapshot(0);
    expect(snapshot?.renderedRange).toEqual({ startIndex: 0, endIndex: 9 });
    expect(snapshot?.targetOffset).toBe(0);
    expect(ref.current?.measureOffsetForIndexAtSnapshot(0, snapshot!)).toBe(0);
    expect(ref.current?.measureOffsetForIndexAtSnapshot(1, snapshot!)).toBeNull();

    const header = document.querySelector<HTMLElement>('[data-virtual-header]')!;
    act(() => resizeObservers[0]!.triggerEntries([[header, 60]]));

    expect(totals.at(-1)).toBe(260);
    expect(scrollTopOf(scroller)).toBe(60);
    expect(ref.current?.measureOffsetForIndex(0)).toBe(0);
  });

  it('reports no visible range while the physical viewport is wholly inside the header', () => {
    const ranges: VirtualTranscriptPhysicalSnapshot[] = [];
    const ref = { current: null as VirtualTranscriptHandle | null };

    render(
      <VirtualTranscript
        ref={ref}
        items={makeItems(5, 20)}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={0}
        initialTail={false}
        header={<div data-testid="header" data-height={150}>Header</div>}
        renderItem={renderRow}
        onRangeChange={(snapshot) => ranges.push(snapshot)}
      />,
    );

    expect(ranges.at(-1)?.visibleRange).toBeNull();
    expect(ranges.at(-1)?.renderedRange).toEqual({ startIndex: 0, endIndex: 4 });
    expect(ref.current?.physicalSnapshot().visibleRange).toBeNull();
  });

  it('clips visible range to the row region when the viewport partially overlaps the header', () => {
    const ranges: VirtualTranscriptPhysicalSnapshot[] = [];
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;

    render(
      <VirtualTranscript
        ref={ref}
        items={makeItems(5, 20)}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={0}
        initialTail={false}
        header={<div data-testid="header" data-height={30}>Header</div>}
        renderItem={renderRow}
        onRangeChange={(snapshot) => ranges.push(snapshot)}
        scrollerRef={(element) => { scroller = element; }}
      />,
    );

    act(() => {
      scroller!.scrollTop = 10;
      ref.current?.scrollToIndex(0, 'start', 20);
    });

    const snapshot = ref.current?.physicalSnapshot();
    expect(snapshot?.viewportTop).toBe(10);
    expect(snapshot?.visibleRange).toEqual({ startIndex: 0, endIndex: 3 });
    expect(ranges.at(-1)?.visibleRange).toEqual({ startIndex: 0, endIndex: 3 });
  });

  it('renders header with the empty state and observes async header resize', () => {
    const totals: number[] = [];

    render(
      <VirtualTranscript
        items={[]}
        getKey={(item: TestItem) => item.id}
        estimatedExtent={20}
        initialTail={false}
        header={<div data-testid="header" data-height={25}>Header</div>}
        empty={<div data-testid="empty">Empty</div>}
        renderItem={renderRow}
        onTotalExtentChange={(total) => totals.push(total)}
      />,
    );

    expect(screen.getByTestId('header')).toBeInTheDocument();
    expect(screen.getByTestId('empty')).toBeInTheDocument();
    expect(totals.at(-1)).toBe(25);

    const header = document.querySelector<HTMLElement>('[data-virtual-header]')!;
    act(() => resizeObservers[0]!.triggerEntries([[header, 40]]));

    expect(totals.at(-1)).toBe(40);
  });

  it('preserves absolute viewport position for a header-only view across the next prepend', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const initial = makeItems(20, 20);
    const view = render(
      <VirtualTranscript
        ref={ref}
        items={initial}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={0}
        initialTail={false}
        header={<div data-height={500}>System prompt</div>}
        renderItem={renderRow}
      />,
    );
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    act(() => ref.current?.scrollToIndex(10, 'start'));
    const viewportTop = scroller.scrollTop;

    act(() => ref.current?.preserveViewportOnNextItemsChange());
    const unrelatedTailUpdate = [...initial, { id: 'streaming-tail', height: 40, label: 'streaming tail' }];
    act(() => {
      view.rerender(
        <VirtualTranscript
          ref={ref}
          items={unrelatedTailUpdate}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={0}
          initialTail={false}
          header={<div data-height={500}>System prompt</div>}
          renderItem={renderRow}
        />,
      );
    });
    scroller.scrollTop = viewportTop + 25;
    fireEvent.scroll(scroller);
    act(() => {
      view.rerender(
        <VirtualTranscript
          ref={ref}
          items={[...makeItems(5, 20).map((item) => ({ ...item, id: `older-${item.id}` })), ...unrelatedTailUpdate]}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={0}
          initialTail={false}
          header={<div data-height={500}>System prompt</div>}
          renderItem={renderRow}
        />,
      );
    });

    expect(scroller.scrollTop).toBe(viewportTop + 25);
  });

  it('preserves measured extents by stable key across prepends and removes only absent keys', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };

    const initial: TestItem[] = [
      { id: 'a', label: 'Item A', height: 20 },
      { id: 'b', label: 'Item B', height: 20 },
      { id: 'c', label: 'Item C', height: 20 },
    ];
    const view = render(
      <VirtualTranscript
        ref={ref}
        items={initial}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={200}
        initialTail={false}
        renderItem={renderRow}
      />,
    );

    const rowB = document.querySelector<HTMLElement>('[data-virtual-key="b"]')!;
    act(() => resizeObservers[0]!.triggerEntries([[rowB, 80]]));
    act(() => ref.current?.scrollToIndex(0, 'start'));
    expect(ref.current?.measureOffsetForIndex(2)).toBe(100);

    act(() => {
      view.rerender(
        <VirtualTranscript
          ref={ref}
          items={[{ id: 'z', label: 'Item Z', height: 20 }, ...initial]}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={200}
          initialTail={false}
          renderItem={renderRow}
        />,
      );
    });

    act(() => ref.current?.scrollToIndex(0, 'start'));
    expect(ref.current?.measureOffsetForIndex(3)).toBe(120);

    act(() => {
      view.rerender(
        <VirtualTranscript
          ref={ref}
          items={[initial[0]!, initial[2]!]}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={200}
          initialTail={false}
          renderItem={renderRow}
        />,
      );
    });

    act(() => ref.current?.scrollToIndex(0, 'start'));
    expect(ref.current?.measureOffsetForIndex(1)).toBe(20);
  });

  it('keeps getKey identities stable when new items are prepended', () => {
    function MountedRow({ item }: { item: TestItem }) {
      return <div data-testid={`mounted-${item.id}`} data-height={item.height}>{item.label}</div>;
    }

    const initial: TestItem[] = [
      { id: 'a', label: 'Item A', height: 20 },
      { id: 'b', label: 'Item B', height: 20 },
      { id: 'c', label: 'Item C', height: 20 },
    ];
    const view = render(
      <VirtualTranscript
        items={initial}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={200}
        initialTail={false}
        renderItem={(item) => <MountedRow item={item} />}
      />,
    );
    const originalA = screen.getByTestId('mounted-a');
    const originalB = screen.getByTestId('mounted-b');
    const originalC = screen.getByTestId('mounted-c');

    act(() => {
      view.rerender(
        <VirtualTranscript
          items={[{ id: 'z', label: 'Item Z', height: 20 }, ...initial]}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={200}
          initialTail={false}
          renderItem={(item) => <MountedRow item={item} />}
        />,
      );
    });

    expect(screen.getByTestId('mounted-a')).toBe(originalA);
    expect(screen.getByTestId('mounted-b')).toBe(originalB);
    expect(screen.getByTestId('mounted-c')).toBe(originalC);
    expect(rowIndexes()).toEqual([0, 1, 2, 3]);
  });
});
