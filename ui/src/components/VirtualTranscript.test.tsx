import { type ReactElement } from 'react';
import { act, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { VirtualTranscript, type VirtualTranscriptHandle, type VirtualTranscriptPhysicalSnapshot } from './VirtualTranscript';

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

    const duplicateRows = Array.from(document.querySelectorAll<HTMLElement>('[data-virtual-index]')).slice(0, 3);
    const physicalKeys = duplicateRows.map((row) => row.dataset['virtualKey']);
    expect(new Set(physicalKeys).size).toBe(3);
    expect(physicalKeys[0]).toBe('same');
    expect(physicalKeys[1]).toContain('duplicate:1');
    expect(physicalKeys[2]).toContain('duplicate:2');
    expect(error).toHaveBeenCalledWith(
      '[VirtualTranscript] duplicate semantic keys quarantined',
      { duplicateKeys: ['same'] },
    );

    act(() => ref.current?.scrollToIndex(3, 'start'));
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
