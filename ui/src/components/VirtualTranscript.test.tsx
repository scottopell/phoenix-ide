import { useCallback, useLayoutEffect, useRef, useState, type ReactElement } from 'react';
import { initialTranscriptPositioningState, reduceTranscriptPositioning, transcriptPositioningCommandKey, type TranscriptPositioningInput, type TranscriptPositioningEvent } from '../conversation/transcriptPositioning';
import type { HistoryScrollCommand } from '../conversation/historyExpansion';

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

function isHiddenByAncestor(element: Element): boolean {
  for (let ancestor: Element | null = element; ancestor; ancestor = ancestor.parentElement) {
    if (ancestor.hasAttribute('hidden') || (ancestor instanceof HTMLElement && ancestor.style.display === 'none')) return true;
  }
  return false;
}

function heightFromElement(element: Element): number {
  if (isHiddenByAncestor(element)) return 0;
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
      if (isHiddenByAncestor(this)) return 0;
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
        ariaLabel="Transcript"
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

  it('exposes the scroll port as a named region a keyboard can reach', () => {
    render(
      <VirtualTranscript
        items={makeItems(100)}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={20}
        initialTail={false}
        ariaLabel="Conversation transcript"
        renderItem={renderRow}
      />,
    );

    const scroller = screen.getByRole('region', { name: 'Conversation transcript' });
    expect(scroller).toHaveClass('virtual-transcript');
    // Sequential focusability, not merely programmatic: tabIndex -1 would leave
    // the port unreachable for a keyboard reader (REQ-VT-013).
    expect(scroller.tabIndex).toBe(0);

    scroller.focus();
    expect(document.activeElement).toBe(scroller);
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
        ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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

  it('keeps dock padding out of the virtual viewport extent after scrolling and tailing', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    render(
      <VirtualTranscript
        ariaLabel="Transcript"
        ref={ref}
        items={makeItems(20, 10)}
        getKey={(item) => item.id}
        estimatedExtent={10}
        overscan={0}
        initialTail={false}
        renderItem={renderRow}
        scrollerRef={(element) => { scroller = element; }}
      />,
    );
    scroller!.style.paddingBottom = '40px';
    act(() => resizeObservers[0]!.triggerEntries([[scroller!, 60]]));
    fireEvent.scroll(scroller!, { target: { scrollTop: 1 } });
    act(() => ref.current?.scrollToTail());
    expect(scrollTopOf(scroller)).toBe(140);
    expect(rowIndexes()).toEqual([14, 15, 16, 17, 18, 19]);
  });

  it('advances a pinned tail when dock padding reserves viewport space', () => {
    let scroller: HTMLDivElement | null = null;
    render(
      <VirtualTranscript
        ariaLabel="Transcript"
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
    scroller!.classList.add('reaction-dock-reserved');
    scroller!.style.paddingBottom = '40px';
    act(() => resizeObservers[0]!.triggerEntries([[scroller!, 60]]));
    expect(scrollTopOf(scroller)).toBe(140);
    expect(rowIndexes()).toEqual([14, 15, 16, 17, 18, 19]);
  });

  it('uses dock-reserved viewport extent before clamping clearance for an unpinned reader', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    const pinnedStates: boolean[] = [];
    render(
      <VirtualTranscript
        ariaLabel="Transcript"
        ref={ref}
        items={makeItems(20, 10)}
        getKey={(item) => item.id}
        estimatedExtent={10}
        overscan={0}
        initialTail
        renderItem={renderRow}
        scrollerRef={(element) => { scroller = element; }}
        onPinnedChange={(pinned) => pinnedStates.push(pinned)}
      />,
    );
    fireEvent.scroll(scroller!, { target: { scrollTop: 65 } });
    act(() => ref.current?.setTailFollowAllowed(false));
    expect(pinnedStates.at(-1)).toBe(false);

    scroller!.classList.add('reaction-dock-reserved');
    scroller!.style.paddingBottom = '40px';
    act(() => ref.current?.scrollBy(70));
    expect(scrollTopOf(scroller)).toBe(135);
    act(() => resizeObservers[0]!.triggerEntries([[scroller!, 60]]));
    expect(scrollTopOf(scroller)).toBe(135);
    expect(pinnedStates.at(-1)).toBe(false);
  });

  it('marks scrollBy as programmatic for the scroll-policy echo', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    render(
      <VirtualTranscript ariaLabel="Transcript" ref={ref} items={makeItems(20, 10)} getKey={(item) => item.id} estimatedExtent={10} overscan={0} initialTail renderItem={renderRow} scrollerRef={(element) => { scroller = element; }} />,
    );
    expect(scrollTopOf(scroller)).toBe(100);
    act(() => ref.current?.scrollBy(-35));
    expect(scrollTopOf(scroller)).toBe(65);
    expect(ref.current?.isProgrammaticScroll(65)).toBe(true);
  });

  it.each([
    { deferred: false, headerHeight: 42 },
    { deferred: true, headerHeight: 42 },
    { deferred: false, headerHeight: 42.5 },
    { deferred: true, headerHeight: 42.5 },
    { deferred: false, headerHeight: 42.5, quantize: false },
    { deferred: true, headerHeight: 42.5, quantize: false },
    { deferred: false, headerHeight: 42.5, intra: 8, viewportOffset: 12 },
    { deferred: true, headerHeight: 42.5, intra: 8, viewportOffset: 12 },
    { deferred: false, headerHeight: 42.5, viewportOffset: 100 },
    { deferred: true, headerHeight: 42.5, viewportOffset: 100 },
  ].map(testCase => ({ quantize: true, intra: 0, viewportOffset: 0, ...testCase })))('keeps the physical first-user start inside the clip ($deferred, $headerHeight, $quantize, $intra, $viewportOffset)', ({ deferred, headerHeight, quantize, intra, viewportOffset }) => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    let actualTop = 0;
    let measuredHeader = headerHeight;
    const requested: number[] = [];
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    const rectSpy = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      const rect = originalRect.call(this);
      const row = this.closest<HTMLElement>('[data-virtual-index]');
      const top = this.classList.contains('virtual-transcript') ? 93
        : row ? 93 + measuredHeader + Number(row.dataset['virtualIndex']) * 75 - actualTop + (this.hasAttribute('data-testid') ? intra : 0)
          : rect.top;
      const height = this.classList.contains('virtual-transcript') ? this.clientHeight : rect.height;
      return { ...rect, top, y: top, height, bottom: top + height } as DOMRect;
    });
    try {
      render(<div data-testid="quantized-wrapper"><VirtualTranscript ariaLabel="Quantized transcript" ref={ref}
        items={makeItems(20, 75)} getKey={(item) => item.id} estimatedExtent={75}
        overscan={0} initialTail={false} header={<div data-height={headerHeight}>Header</div>}
        renderItem={(item) => <div data-height={75}><div data-testid={`quantized-${item.id}`} data-height={67}>First user</div></div>}
        scrollerRef={(element) => {
          scroller = element;
          if (element) Object.defineProperty(element, 'scrollTop', {
            configurable: true,
            get: () => actualTop,
            set: (value: number) => { requested.push(value); actualTop = quantize ? Math.round(value) : value; },
          });
        }} /></div>);
      const wrapper = screen.getByTestId('quantized-wrapper');
      const selector = '[data-testid="quantized-item-0"]';
      if (deferred) wrapper.style.display = 'none';
      act(() => ref.current?.scrollToIndex(0, 'start', viewportOffset, selector));
      if (deferred) {
        expect(ref.current?.physicalSnapshot(0, selector).targetMeasured).toBe(false);
        wrapper.style.display = '';
        const header = document.querySelector<HTMLElement>('[data-virtual-header]')!;
        act(() => resizeObservers[0]!.triggerEntries([[scroller!, 100], [header, headerHeight]]));
      }
      if (quantize) expect(Number.isInteger(actualTop)).toBe(true);
      if (viewportOffset === 100) expect(requested).toHaveLength(0);
      else if (!quantize || Number.isInteger(headerHeight)) expect(requested).toHaveLength(1);
      else expect(requested).toHaveLength(2);
      expect(ref.current?.isProgrammaticScroll(actualTop)).toBe(viewportOffset !== 100);
      expect(ref.current?.physicalSnapshot(0, selector).targetMeasured).toBe(true);
      const child = screen.getByTestId('quantized-item-0');
      const row = child.closest<HTMLElement>('[data-virtual-index]')!;
      const clip = scroller!.getBoundingClientRect();
      if (viewportOffset !== 100) {
        expect(child.getBoundingClientRect().bottom).toBeLessThanOrEqual(clip.bottom);
        expect(row.getBoundingClientRect().bottom).toBeLessThanOrEqual(clip.bottom);
      }
      const writes = JSON.stringify({ requested, actualTop });
      expect.soft(row.getBoundingClientRect().top - clip.top, writes).toBeGreaterThanOrEqual(0);
      expect(child.getBoundingClientRect().top - clip.top, writes).toBeGreaterThanOrEqual(Math.min(viewportOffset, headerHeight + intra));
      const beforeResize = ref.current!.physicalSnapshot(0, selector);
      expect(beforeResize.viewportTop).toBe(actualTop);
      const initialOffset = child.getBoundingClientRect().top - clip.top;
      measuredHeader += 10;
      const header = document.querySelector<HTMLElement>('[data-virtual-header]')!;
      act(() => resizeObservers[0]!.triggerEntries([[header, measuredHeader]]));
      expect(child.getBoundingClientRect().top - clip.top).toBe(initialOffset);
    } finally {
      rectSpy.mockRestore();
    }
  });

  it.each(['upper', 'end', 'tail', 'continuity', 'large-displacement'] as const)('bounds directional correction without changing %s positioning', mode => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const writes: number[] = [];
    let actualTop = 0;
    render(<VirtualTranscript ariaLabel="Quantized controls" ref={ref}
      items={makeItems(20, 75)} getKey={item => item.id} estimatedExtent={75} overscan={0}
      initialTail={false} header={<div data-height={42.5}>Header</div>} renderItem={renderRow}
      scrollerRef={element => {
        if (element) Object.defineProperty(element, 'scrollTop', {
          configurable: true, get: () => actualTop,
          set: (value: number) => { writes.push(value); actualTop = mode === 'large-displacement' ? value + 5 : Math.round(value); },
        });
      }} />);
    act(() => {
      if (mode === 'upper') ref.current?.setTailFollowAllowed(false);
      if (mode === 'tail') ref.current?.scrollToTail();
      else ref.current?.scrollToIndex(mode === 'upper' ? 19 : 0, mode === 'end' ? 'end' : 'start', 0, undefined, mode === 'continuity' ? 'continuity' : 'directional');
    });
    if (mode === 'upper') {
      expect(writes).toEqual([1442.5, 1441.5]);
      expect(actualTop).toBe(1442);
    } else {
      if (mode === 'tail') expect(writes.every(value => value === 1442.5)).toBe(true);
      else expect(writes).toEqual([mode === 'end' ? 17.5 : 42.5]);
      expect(actualTop).toBe(mode === 'end' ? 18 : mode === 'tail' ? 1443 : mode === 'large-displacement' ? 47.5 : 43);
    }
    expect(ref.current?.isProgrammaticScroll(actualTop)).toBe(true);
    if (mode === 'upper' || mode === 'large-displacement') expect(ref.current?.physicalSnapshot().viewportTop).toBe(actualTop);
  });

  it('does not classify user scrolling after a clamped no-op start as an own-write echo', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    render(<VirtualTranscript ariaLabel="No-op start" ref={ref} items={makeItems(20, 75)}
      getKey={item => item.id} estimatedExtent={75} initialTail={false}
      header={<div data-height={42.5}>Header</div>} renderItem={renderRow} />);
    act(() => ref.current?.scrollToIndex(0, 'start', 100));
    expect(ref.current?.isProgrammaticScroll(0)).toBe(false);
    const scroller = screen.getByRole('region', {name: 'No-op start'});
    fireEvent.scroll(scroller, {target: {scrollTop: 1}});
    expect(ref.current?.isProgrammaticScroll(1)).toBe(false);
    expect(ref.current?.physicalSnapshot().viewportTop).toBe(1);
  });

  it.each(['directional', 'fraction-accepted', 'continuity', 'interrupted', 'cancelled', 'replacement'] as const)('preserves %s positioning when the actual header ref mounts after the start command', mode => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let actualTop = 0;
    let headerHeight = 0;
    const writes: number[] = [];
    const items = makeItems(20, 75);
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    const geometry = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      const rect = originalRect.call(this);
      const row = this.closest<HTMLElement>('[data-virtual-index]');
      const top = this.classList.contains('virtual-transcript') ? 93
        : row ? 93 + headerHeight + Number(row.dataset['virtualIndex']) * 75 - actualTop : rect.top;
      const height = this.classList.contains('virtual-transcript') ? this.clientHeight : rect.height;
      return { ...rect, top, y: top, height, bottom: top + height } as DOMRect;
    });
    const key = (item: TestItem) => item.id;
    const rowPayload = (item: TestItem) => <div data-height={75}><div data-testid={`late-${item.id}`} data-height={67}>First user</div></div>;
    const attachScroller = (element: HTMLDivElement | null) => {
      if (element) Object.defineProperty(element, 'scrollTop', {
        configurable: true, get: () => actualTop,
        set: (value: number) => { writes.push(value); actualTop = mode === 'fraction-accepted' ? value : Math.round(value); },
      });
    };
    let currentItems = items;
    const view = (withHeader: boolean) => <VirtualTranscript ariaLabel="Late header" ref={ref}
      positioningAuthority={{kind: 'command', commandKey: 'late-start'}}
      items={currentItems} getKey={key} estimatedExtent={75} initialTail={false}
      header={withHeader ? <div data-height={42.5}>Late header</div> : undefined}
      renderItem={rowPayload} scrollerRef={attachScroller} />;
    try {
      const {rerender} = render(view(false));
      act(() => ref.current?.scrollToIndex(0, 'start', 0, undefined, mode === 'continuity' ? 'continuity' : 'directional'));
      if (mode === 'interrupted') fireEvent.scroll(screen.getByRole('region', {name: 'Late header'}), {target: {scrollTop: 0}});
      if (mode === 'cancelled') {
        act(() => ref.current?.scrollToOwnedIndex('late-start', 0, 'start'));
        act(() => ref.current?.cancelPosition('late-start'));
      }
      if (mode === 'replacement') act(() => ref.current?.scrollToIndex(0, 'start', 0, undefined, 'continuity'));
      writes.length = 0;
      headerHeight = 42.5;
      rerender(view(true));
      const scroller = screen.getByRole('region', {name: 'Late header'});
      const child = screen.getByTestId('late-item-0');
      const row = child.closest<HTMLElement>('[data-virtual-index]')!;
      expect(ref.current?.physicalSnapshot().viewportTop).toBe(actualTop);
      if (mode === 'directional' || mode === 'fraction-accepted') {
        expect(writes).toEqual(mode === 'directional' ? [42.5, 41.5] : [42.5]);
        expect.soft(row.getBoundingClientRect().top - scroller.getBoundingClientRect().top, JSON.stringify(writes)).toBeGreaterThanOrEqual(0);
        expect(child.getBoundingClientRect().top - scroller.getBoundingClientRect().top).toBeGreaterThanOrEqual(0);
        headerHeight = 43;
        const header = document.querySelector<HTMLElement>('[data-virtual-header]')!;
        act(() => resizeObservers[0]!.triggerEntries([[header, headerHeight]]));
        expect(child.getBoundingClientRect().top - scroller.getBoundingClientRect().top).toBeGreaterThanOrEqual(0);
        const offset = child.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
        currentItems = [...items];
        rerender(view(true));
        expect(child.getBoundingClientRect().top - scroller.getBoundingClientRect().top).toBe(offset);
      } else {
        expect(writes).toEqual([42.5]);
        expect(actualTop).toBe(43);
      }
    } finally { geometry.mockRestore(); }
  });

  it.each(['applied', 'user-cancel', 'replacement'] as const)('separates successful anchor continuity from command capability retirement (%s)', terminal => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const items = makeItems(20, 75);
    const key = (item: TestItem) => item.id;
    const command: HistoryScrollCommand = {kind: 'jump_to_message', token: 1, requestToken: 1,
      view: {conversationId: 'late-parent', generation: 1, transcriptGeneration: 1}, targetMessageId: 'item-0'};
    const commandKey = transcriptPositioningCommandKey(command);
    const replacement: HistoryScrollCommand = {...command, token: 2};
    const finishes: string[] = [];
    const positions: string[] = [];
    const authorities: string[] = [];
    const writes: number[] = [];
    let actualTop = 0;
    let headerHeight = 0;
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    const geometry = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      const rect = originalRect.call(this);
      const row = this.closest<HTMLElement>('[data-virtual-index]');
      const top = this.classList.contains('virtual-transcript') ? 93
        : row ? 93 + headerHeight + Number(row.dataset['virtualIndex']) * 75 - actualTop : rect.top;
      const height = this.classList.contains('virtual-transcript') ? this.clientHeight : rect.height;
      return {...rect, top, y: top, height, bottom: top + height} as DOMRect;
    });
    const attach = (element: HTMLDivElement | null) => {
      if (element) Object.defineProperty(element, 'scrollTop', {configurable: true,
        get: () => actualTop, set: (value: number) => {writes.push(value); actualTop = Math.round(value);}});
    };
    function Parent({withHeader, begin = false}: {withHeader: boolean; begin?: boolean}) {
      const [input, setInput] = useState<TranscriptPositioningInput>({kind: 'idle', view: command.view});
      useLayoutEffect(() => {if (begin) setInput({kind: 'positioning', command});}, [begin]);
      const state = useRef(initialTranscriptPositioningState(command.view));
      const dispatch = useCallback((event: TranscriptPositioningEvent): void => {
        const reduced = reduceTranscriptPositioning(state.current, event);
        state.current = reduced.state;
        for (const effect of reduced.effects) {
          if (effect.type === 'resolve_target' && effect.commandKey === commandKey) {
            dispatch({type: 'target_resolved', commandKey, targetIndex: 0});
          } else if (effect.type === 'position') {
            positions.push(effect.commandKey);
            ref.current!.scrollToOwnedIndex(effect.commandKey, effect.targetIndex, effect.align, effect.viewportStartOffset);
            const observed = ref.current!.physicalSnapshot(effect.targetIndex);
            expect(observed.targetMeasured).toBe(true);
            dispatch({type: 'position_issued', commandKey, targetIndex: effect.targetIndex, layoutRevision: observed.layoutRevision});
            if (terminal === 'user-cancel') dispatch({type: 'user_interrupted'});
            else if (terminal === 'replacement') {
              setInput({kind: 'positioning', command: replacement});
              dispatch({type: 'input_changed', input: {kind: 'positioning', command: replacement}});
            } else dispatch({type: 'physical_observed', commandKey, range: observed.visibleRange,
              actualOffset: observed.targetOffset ?? null, layoutRevision: observed.layoutRevision, targetMeasured: observed.targetMeasured ?? false});
          } else if (effect.type === 'finish') {
            finishes.push(effect.result);
            if (effect.result === 'applied') ref.current!.completePosition(effect.commandKey);
            if (effect.result === 'superseded') ref.current!.cancelPosition(effect.commandKey);
            if (terminal !== 'replacement') setInput({kind: 'idle', view: command.view});
          }
        }
      }, []);
      useLayoutEffect(() => {dispatch({type: 'input_changed', input});}, [dispatch, input]);
      const authority = input.kind === 'positioning' ? transcriptPositioningCommandKey(input.command) : null;
      authorities.push(authority ?? 'none');
      return <VirtualTranscript ariaLabel="Completed parent" ref={ref} items={items} getKey={key}
        positioningAuthority={authority ? {kind: 'command', commandKey: authority} : {kind: 'absent'}}
        estimatedExtent={75} initialTail={false} scrollerRef={attach}
        header={withHeader ? <div data-height={42.5}>Late parent header</div> : undefined}
        renderItem={renderRow} />;
    }
    try {
      const {rerender} = render(<Parent withHeader={false} />);
      rerender(<Parent withHeader={false} begin />);
      expect(positions).toEqual([commandKey]);
      expect(finishes).toEqual([terminal === 'applied' ? 'applied' : 'superseded']);
      expect(authorities.at(-1)).toBe(terminal === 'replacement' ? transcriptPositioningCommandKey(replacement) : 'none');
      writes.length = 0;
      headerHeight = 42.5;
      rerender(<Parent withHeader begin />);
      expect(positions).toEqual([commandKey]);
      expect(finishes).toEqual([terminal === 'applied' ? 'applied' : 'superseded']);
      const row = document.querySelector<HTMLElement>('[data-virtual-key="item-0"]')!;
      const clip = screen.getByRole('region', {name: 'Completed parent'}).getBoundingClientRect();
      if (terminal === 'applied') {
        const physicalOffset = row.getBoundingClientRect().top - clip.top;
        expect(writes.length).toBeGreaterThan(0);
        expect(actualTop).toBeGreaterThanOrEqual(41.5);
        expect(physicalOffset).toBeLessThanOrEqual(1);
        expect(physicalOffset, JSON.stringify({writes, actualTop, finishes, authorities})).toBeGreaterThanOrEqual(0);
      } else {expect(writes).toEqual([42.5]); expect(actualTop).toBe(43);}
    } finally {geometry.mockRestore();}
  });

  it.each(['keep', 'user-scroll', 'takeover-without-scroll', 'cancel', 'replacement', 'view-reset', 'stale-completion'] as const)('does not resurrect a settled start anchor after %s', transition => {
    const ref = {current: null as VirtualTranscriptHandle | null};
    const items = makeItems(20, 75);
    const key = (item: TestItem) => item.id;
    const writes: number[] = [];
    let actualTop = 0;
    const attach = (element: HTMLDivElement | null) => {
      if (element) Object.defineProperty(element, 'scrollTop', {configurable: true,
        get: () => actualTop, set: (value: number) => {writes.push(value); actualTop = Math.round(value);}});
    };
    let viewKey = 'view-1';
    const view = (authority: 'first' | 'next' | null, header: boolean) => <VirtualTranscript ref={ref}
      positioningViewKey={viewKey}
      ariaLabel="Settled controls" items={items} getKey={key} initialTail={false} estimatedExtent={75}
      positioningAuthority={authority ? {kind: 'command', commandKey: authority} : {kind: 'absent'}}
      header={header ? <div data-height={42.5}>Header</div> : undefined} renderItem={renderRow} scrollerRef={attach} />;
    const {rerender} = render(view('first', false));
    act(() => {ref.current!.scrollToOwnedIndex('first', 0, 'start'); ref.current!.completePosition('first');});
    rerender(view(null, false));
    let authority: 'next' | null = null;
    if (transition === 'user-scroll') fireEvent.scroll(screen.getByRole('region', {name: 'Settled controls'}), {target: {scrollTop: 0}});
    if (transition === 'cancel') act(() => ref.current!.cancelPosition('first'));
    if (transition === 'takeover-without-scroll') act(() => ref.current!.interruptPosition());
    if (transition === 'view-reset') viewKey = 'view-2';
    if (transition === 'replacement' || transition === 'stale-completion') {
      authority = 'next';
      rerender(view(authority, false));
      if (transition === 'stale-completion') act(() => ref.current!.completePosition('first'));
    }
    writes.length = 0;
    rerender(view(authority, true));
    expect(writes).toEqual(transition === 'keep' ? [42.5, 41.5] : [42.5]);
    expect(actualTop).toBe(transition === 'keep' ? 42 : 43);
  });

  it('positions an intra-row target through the transcript executor', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;

    render(
      <VirtualTranscript
        ariaLabel="Transcript"
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

  it('resolves an exact range target after its virtualized row mounts', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    render(<VirtualTranscript ariaLabel="Transcript" ref={ref} items={makeItems(30, 200)}
      getKey={(item) => item.id} estimatedExtent={200} overscan={0} initialTail={false}
      renderItem={renderRow} scrollerRef={(element) => { scroller = element; }} />);
    expect(screen.queryByTestId('payload-item-20')).toBeNull();
    const resolve = vi.fn((row: HTMLElement) => {
      const range = document.createRange();
      range.selectNodeContents(row.querySelector('[data-height]')!);
      range.getBoundingClientRect = () => ({ top: 38, height: 10 } as DOMRect);
      return range;
    });
    act(() => ref.current?.scrollToIndex(20, 'start', 12, resolve));
    expect(resolve).toHaveBeenCalled();
    expect(screen.getByTestId('payload-item-20')).toBeInTheDocument();
    expect(scrollTopOf(scroller)).toBe(4026);
  });

  it('compensates scrollTop when a measured row above the top-edge anchor resizes', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    let scroller: HTMLDivElement | null = null;
    const items = makeItems(30, 20);
    const estimate = (item: TestItem) => item.height;

    const view = render(
      <VirtualTranscript
        ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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

  it.each([400, GESTURE_STALE_MS + 500])('stationary held contact preserves modeled reading position on resize after %sms', delay => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;
      render(<VirtualTranscript ariaLabel="Transcript" ref={ref} items={makeItems(30, 20)}
        getKey={item => item.id} estimatedExtent={20} overscan={200} initialTail={false}
        renderItem={renderRow} scrollerRef={element => { scroller = element; }} />);
      act(() => ref.current!.scrollToIndex(20, 'start'));
      const anchor = document.querySelector<HTMLElement>('[data-virtual-key="item-20"]')!;
      const above = document.querySelector<HTMLElement>('[data-virtual-key="item-12"]')!;
      expect(anchor?.isConnected).toBe(true);
      expect(above?.isConnected).toBe(true);
      const viewportOffset = () => {
        const current = document.querySelector<HTMLElement>('[data-virtual-key="item-20"]')!;
        let top = 0;
        for (let sibling = current.previousElementSibling; sibling; sibling = sibling.previousElementSibling) {
          top += sibling.classList.contains('virtual-transcript__spacer')
            ? Number.parseFloat((sibling as HTMLElement).style.height) || 0 : heightFromElement(sibling);
        }
        if (!scroller) throw new Error('Mounted scroller missing');
        return top - scroller.scrollTop;
      };
      act(() => fireEvent.touchStart(anchor, {
        touches: [{ identifier: 31, clientX: 20, clientY: 10 }],
        changedTouches: [{ identifier: 31, clientX: 20, clientY: 10 }],
      }));
      const baseline = viewportOffset();
      act(() => vi.advanceTimersByTime(delay));
      expect(viewportOffset()).toBe(baseline);
      const beforeControl = scroller!.scrollTop;
      scroller!.scrollTop = beforeControl + 12;
      expect(viewportOffset() - baseline).toBe(-12);
      scroller!.scrollTop = beforeControl;
      expect(viewportOffset()).toBe(baseline);
      expect(document.querySelectorAll('[data-virtual-key="item-20"]')).toHaveLength(1);
      expect(anchor.isConnected).toBe(true);
      act(() => {
        above.querySelector('[data-height]')!.setAttribute('data-height', '50');
        resizeObservers[0]!.triggerEntries([[above, 50]]);
      });
      expect(document.querySelectorAll('[data-virtual-key="item-20"]')).toHaveLength(1);
      expect(document.querySelector('[data-virtual-key="item-20"]')).toBe(anchor);
      expect(anchor.isConnected).toBe(true);
      expect(viewportOffset()).toBe(baseline);
      act(() => vi.advanceTimersByTime(GESTURE_STALE_MS + 500));
      expect(document.querySelectorAll('[data-virtual-key="item-20"]')).toHaveLength(1);
      expect(document.querySelector('[data-virtual-key="item-20"]')).toBe(anchor);
      expect(anchor.isConnected).toBe(true);
      expect(viewportOffset()).toBe(baseline);
    } finally { vi.useRealTimers(); }
  });

  it.each([200, 2000])('held reading position survives repeated growth across spacer capacity with overscan %s', overscan => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;
      render(<VirtualTranscript ariaLabel="Transcript" ref={ref} items={makeItems(30, 20)}
        getKey={item => item.id} estimatedExtent={20} overscan={overscan} initialTail={false}
        renderItem={renderRow} scrollerRef={element => { scroller = element; }} />);
      act(() => ref.current!.scrollToIndex(20, 'start'));
      const anchor = document.querySelector<HTMLElement>('[data-virtual-key="item-20"]')!;
      const readPosition = () => {
        expect(document.querySelectorAll('[data-virtual-key="item-20"]')).toHaveLength(1);
        expect(document.querySelector('[data-virtual-key="item-20"]')).toBe(anchor);
        expect(anchor.isConnected).toBe(true);
        if (!scroller) throw new Error('Mounted scroller missing');
        let top = 0;
        for (let n = anchor.previousElementSibling; n; n = n.previousElementSibling) {
          top += n.classList.contains('virtual-transcript__spacer')
            ? Number.parseFloat((n as HTMLElement).style.height) || 0 : heightFromElement(n);
        }
        return top - scroller.scrollTop;
      };
      act(() => fireEvent.touchStart(anchor, {
        touches: [{ identifier: 41, clientX: 20, clientY: 10 }],
        changedTouches: [{ identifier: 41, clientX: 20, clientY: 10 }],
      }));
      const baseline = readPosition();
      const originalTop = scroller!.scrollTop;
      scroller!.scrollTop = originalTop + 12;
      expect(readPosition() - baseline).toBe(-12);
      scroller!.scrollTop = originalTop;
      expect(readPosition()).toBe(baseline);
      for (const height of [50, 280, 20]) {
        const above = document.querySelector<HTMLElement>('[data-virtual-key="item-12"]')!;
        expect(above?.isConnected).toBe(true);
        act(() => {
          above.querySelector('[data-height]')!.setAttribute('data-height', String(height));
          resizeObservers[0]!.triggerEntries([[above, height]]);
        });
        expect(readPosition()).toBe(baseline);
        act(() => vi.advanceTimersByTime(200));
        expect(readPosition()).toBe(baseline);
      }
      act(() => vi.advanceTimersByTime(GESTURE_STALE_MS + 500));
      expect(readPosition()).toBe(baseline);
    } finally { vi.useRealTimers(); }
  });

  it('preserves held anchor with observed zero leading room and immediate compensation write', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;
      render(<VirtualTranscript ariaLabel="Transcript" ref={ref} items={makeItems(30, 20)}
        getKey={item => item.id} estimatedExtent={20} overscan={2000} initialTail={false}
        renderItem={renderRow} scrollerRef={element => { scroller = element; }} />);
      act(() => ref.current!.scrollToIndex(20, 'start'));
      const anchor = document.querySelector<HTMLElement>('[data-virtual-key="item-20"]')!;
      const above = document.querySelector<HTMLElement>('[data-virtual-key="item-12"]')!;
      const leading = scroller!.querySelector<HTMLElement>('.virtual-transcript__spacer')!;
      expect(leading).not.toBeNull();
      expect(Number.parseFloat(leading.style.height)).toBe(0);
      expect(document.querySelectorAll('[data-virtual-key]')).toHaveLength(30);
      expect(above.isConnected).toBe(true);
      const position = () => {
        expect(document.querySelectorAll('[data-virtual-key="item-20"]')).toHaveLength(1);
        expect(document.querySelector('[data-virtual-key="item-20"]')).toBe(anchor);
        expect(anchor.isConnected).toBe(true);
        if (!scroller) throw new Error('Missing scroller');
        let top = 0;
        for (let n = anchor.previousElementSibling; n; n = n.previousElementSibling)
          top += n.classList.contains('virtual-transcript__spacer')
            ? Number.parseFloat((n as HTMLElement).style.height) || 0 : heightFromElement(n);
        return top - scroller.scrollTop;
      };
      act(() => fireEvent.touchStart(anchor, {
        touches: [{ identifier: 51, clientX: 20, clientY: 10 }],
        changedTouches: [{ identifier: 51, clientX: 20, clientY: 10 }],
      }));
      const baseline = position(), before = scroller!.scrollTop;
      expect(position()).toBe(baseline);
      scroller!.scrollTop = before + 12;
      expect(position() - baseline).toBe(-12);
      scroller!.scrollTop = before;
      expect(position()).toBe(baseline);
      act(() => {
        above.querySelector('[data-height]')!.setAttribute('data-height', '50');
        resizeObservers[0]!.triggerEntries([[above, 50]]);
      });
      expect(leading.isConnected).toBe(true);
      expect(scroller!.querySelector('.virtual-transcript__spacer')).toBe(leading);
      expect(Number.parseFloat(leading.style.height)).toBe(0);
      expect(scroller!.scrollTop - before).toBe(30);
      expect(position()).toBe(baseline);
      act(() => vi.advanceTimersByTime(GESTURE_STALE_MS + 500));
      expect(position()).toBe(baseline);
    } finally { vi.useRealTimers(); }
  });

  it('reconciles within the stale bound when a touch lift is unobservable', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;

      render(
        <VirtualTranscript
          ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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

  it('applies the correction as a position, not an offset from wherever the DOM ended up', () => {
    vi.useFakeTimers();
    try {
      const ref = { current: null as VirtualTranscriptHandle | null };
      let scroller: HTMLDivElement | null = null;
      let clampOnPublish = false;

      render(
        <VirtualTranscript
          ariaLabel="Transcript"
          ref={ref}
          items={makeItems(30, 50)}
          getKey={(item) => item.id}
          estimatedExtent={50}
          overscan={2000}
          initialTail={false}
          renderItem={renderRow}
          onRangeChange={() => {
            if (!clampOnPublish) return;
            clampOnPublish = false;
            // Losing the spacer shortens the transcript, and a browser clamps
            // a viewport sitting past the new maximum. A positioning command
            // asking for a snapshot in this window then adopts that position.
            scroller!.scrollTop = 1370;
            ref.current?.physicalSnapshot();
          }}
          scrollerRef={(element) => { scroller = element; }}
        />,
      );

      act(() => {
        scroller!.scrollTop = 1380;
        fireEvent.scroll(scroller!);
      });
      const row = document.querySelector<HTMLElement>('[data-virtual-key="item-27"]')!;
      act(() => {
        fireEvent.touchStart(row, { touches: [{ identifier: 6 }], changedTouches: [{ identifier: 6 }] });
      });
      const above = document.querySelector<HTMLElement>('[data-virtual-key="item-5"]')!;
      act(() => resizeObservers[0]!.triggerEntries([[above, 20]]));

      clampOnPublish = true;
      act(() => {
        fireEvent.touchEnd(row, { touches: [], changedTouches: [{ identifier: 6 }] });
        vi.advanceTimersByTime(GESTURE_STALE_MS + 500);
      });

      // The correction owed was to a position, not a distance. Re-basing it
      // on the clamp applies part of the shift twice and moves the reader.
      expect(scrollTopOf(scroller)).toBe(1350);
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
          ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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

  it.each(['hidden', 'display-none'] as const)('retains cached row extents through ancestor %s and positive reveal observations', (mode) => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const totals: number[] = [];
    const ranges: VirtualTranscriptPhysicalSnapshot[] = [];
    render(
      <div data-testid="retained-wrapper">
        {/* happy-dom does not supply the browser's hidden-attribute UA rule. */}
        <style>{'[hidden] { display: none; }'}</style>
        <VirtualTranscript
          ariaLabel="Transcript"
          ref={ref}
          items={makeItems(5)}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={20}
          initialTail={false}
          header={<div data-height={30}>Header</div>}
          renderItem={renderRow}
          onTotalExtentChange={(extent) => totals.push(extent)}
          onRangeChange={(snapshot) => ranges.push(snapshot)}
        />
      </div>,
    );
    const wrapper = screen.getByTestId('retained-wrapper');
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    const header = document.querySelector<HTMLElement>('[data-virtual-header]')!;
    const rows = virtualRows();
    expect(rows).toHaveLength(5);
    expect(totals.at(-1)).toBe(130);

    if (mode === 'hidden') wrapper.hidden = true;
    else wrapper.style.display = 'none';
    act(() => resizeObservers[0]!.triggerEntries([
      [scroller, 0], [header, 0], ...rows.map((row): [Element, number] => [row, 0]),
    ]));
    act(() => ref.current?.scrollToIndex(0, 'start'));
    expect(ranges.at(-1)?.visibleRange).toBeNull();

    wrapper.hidden = false;
    wrapper.style.display = '';
    // Unmounted rows cannot report their positive sizes on reveal.
    act(() => resizeObservers[0]!.triggerEntries([[scroller, 100], [header, 30]]));
    expect(totals.at(-1)).toBe(130);
    expect(rowIndexes()).toEqual([0, 1, 2, 3, 4]);
    expect(ranges.at(-1)?.visibleRange).not.toBeNull();
    act(() => ref.current?.scrollToIndex(0, 'start'));
    expect(ref.current?.physicalSnapshot(0)).toMatchObject({ targetMeasured: true, targetOffset: 0 });
  });

  it.each(['snapshot-first', 'observer-first', 'snapshot-only'] as const)('positions and remounts a target without hidden measurement poisoning (%s)', (order) => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const ranges: VirtualTranscriptPhysicalSnapshot[] = [];
    render(
      <div data-testid="retained-wrapper">
        {/* happy-dom does not supply the browser's hidden-attribute UA rule. */}
        <style>{'[hidden] { display: none; }'}</style>
        <VirtualTranscript
          ariaLabel="Transcript"
          ref={ref}
          items={makeItems(40)}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={20}
          initialTail={false}
          header={<div data-height={30}>Header</div>}
          renderItem={renderRow}
          onRangeChange={(snapshot) => ranges.push(snapshot)}
        />
      </div>,
    );
    const wrapper = screen.getByTestId('retained-wrapper');
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    const header = document.querySelector<HTMLElement>('[data-virtual-header]')!;
    const hiddenEntries: [Element, number][] = [[scroller, 0], [header, 0], ...virtualRows().map((row): [Element, number] => [row, 0])];
    wrapper.hidden = true;
    act(() => {
      if (order !== 'observer-first') ref.current?.physicalSnapshot();
      if (order !== 'snapshot-only') resizeObservers[0]!.triggerEntries(hiddenEntries);
      if (order === 'observer-first') ref.current?.physicalSnapshot();
      ref.current?.scrollToIndex(20, 'start');
      ref.current?.physicalSnapshot(20);
    });
    // A cached extent is not evidence that a hidden target is physically visible.
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({ visibleRange: null, targetMeasured: false });
    expect(ranges.at(-1)?.visibleRange).toBeNull();

    wrapper.hidden = false;
    act(() => resizeObservers[0]!.triggerEntries([[scroller, 100], [header, 30]]));
    expect(screen.getByTestId('payload-item-20')).toBeInTheDocument();
    act(() => resizeObservers[0]!.triggerEntries(virtualRows().map((row): [Element, number] => [row, 20])));
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({ targetMeasured: true, targetOffset: 0 });
    expect(ranges.at(-1)?.visibleRange).toEqual({ startIndex: 20, endIndex: 24 });
  });

  it.each(['snapshot-first', 'observer-first'] as const)('replays a hidden-issued viewport write when unchanged geometry is revealed (%s)', (order) => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const ranges: VirtualTranscriptPhysicalSnapshot[] = [];
    render(
      <div data-testid="retained-wrapper">
        <VirtualTranscript
          ariaLabel="Transcript"
          ref={ref}
          items={makeItems(40)}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={20}
          initialTail={false}
          header={<div data-height={30}>Header</div>}
          renderItem={renderRow}
          onRangeChange={(snapshot) => ranges.push(snapshot)}
        />
      </div>,
    );
    const wrapper = screen.getByTestId('retained-wrapper');
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    const header = document.querySelector<HTMLElement>('[data-virtual-header]')!;
    let nativeTop = scroller.scrollTop;
    const write = vi.fn((top: number) => {
      // A browser without a layout box may ignore scrollTop assignments.
      if (!isHiddenByAncestor(scroller)) nativeTop = top;
    });
    Object.defineProperty(scroller, 'scrollTop', {
      configurable: true,
      get: () => nativeTop,
      set: write,
    });
    expect(scroller.scrollTop).toBe(0);
    const hiddenEntries: [Element, number][] = [[scroller, 0], [header, 0], ...virtualRows().map((row): [Element, number] => [row, 0])];
    wrapper.style.display = 'none';
    act(() => {
      if (order === 'snapshot-first') ref.current?.physicalSnapshot();
      resizeObservers[0]!.triggerEntries(hiddenEntries);
      ref.current?.scrollToIndex(20, 'start');
    });
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({
      viewportTop: 430, visibleRange: null, targetOffset: null, targetMeasured: false,
    });
    act(() => resizeObservers[0]!.triggerEntries(hiddenEntries));
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({
      viewportTop: 430, visibleRange: null, targetMeasured: false,
    });
    expect(ranges.at(-1)?.visibleRange).toBeNull();
    expect(scroller.scrollTop).toBe(0);

    wrapper.style.display = '';
    act(() => {
      if (order === 'snapshot-first') {
        expect(ref.current?.physicalSnapshot(20)).toMatchObject({ viewportTop: 430, targetOffset: 0 });
        expect(scroller.scrollTop).toBe(430);
      }
      resizeObservers[0]!.triggerEntries([[scroller, 100], [header, 30]]);
    });
    expect(scroller.scrollTop).toBe(430);
    expect(write).toHaveBeenCalledWith(430);
    expect(screen.getByTestId('payload-item-20')).toBeInTheDocument();
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({ viewportTop: 430, targetOffset: 0 });
    act(() => resizeObservers[0]!.triggerEntries(virtualRows().map((row): [Element, number] => [row, 20])));
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({
      viewportTop: 430, targetMeasured: true, targetOffset: 0,
      visibleRange: { startIndex: 20, endIndex: 24 },
    });
    expect(ranges.at(-1)?.visibleRange).toEqual({ startIndex: 20, endIndex: 24 });
    act(() => {
      expect(ref.current?.captureVisibleAnchor()).toEqual({ index: 20, key: 'item-20', offset: 0 });
    });
  });

  it.each(['snapshot-first', 'observer-first'] as const)('recomputes a mounted selector offset before replaying a hidden jump (%s)', (order) => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const items = makeItems(40);
    items[20]!.height = 320;
    render(
      <div data-testid="retained-wrapper">
        <VirtualTranscript
          ariaLabel="Transcript"
          ref={ref}
          items={items}
          getKey={(item) => item.id}
          estimatedExtent={(item) => item.height}
          overscan={20}
          initialTail={false}
          header={<div data-height={30}>Header</div>}
          renderItem={(item) => <div data-height={item.height}>{item.label}<span data-tool={item.id}>Tool</span></div>}
        />
      </div>,
    );
    act(() => ref.current?.scrollToIndex(20, 'start'));
    const wrapper = screen.getByTestId('retained-wrapper');
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    const row = document.querySelector<HTMLElement>('[data-virtual-index="20"]')!;
    const tool = row.querySelector<HTMLElement>('[data-tool="item-20"]')!;
    const rect = tool.getBoundingClientRect();
    tool.getBoundingClientRect = () => isHiddenByAncestor(tool)
      ? { ...rect, top: 0, bottom: 0, height: 0 }
      : { ...rect, top: 200, bottom: 220, height: 20 };
    let nativeTop = 0;
    const write = vi.fn((top: number) => {
      if (!isHiddenByAncestor(scroller)) nativeTop = top;
    });
    Object.defineProperty(scroller, 'scrollTop', { configurable: true, get: () => nativeTop, set: write });
    wrapper.style.display = 'none';
    act(() => {
      resizeObservers[0]!.triggerEntries([[scroller, 0], [row, 0]]);
      ref.current?.scrollToIndex(20, 'start', 0, '[data-tool="item-20"]');
    });
    expect(scroller.scrollTop).toBe(0);
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({ targetMeasured: false, visibleRange: null });
    expect(write).not.toHaveBeenCalled();

    wrapper.style.display = '';
    act(() => {
      if (order === 'snapshot-first') ref.current?.physicalSnapshot(20);
      else resizeObservers[0]!.triggerEntries([[scroller, 100]]);
    });
    expect(scroller.scrollTop).toBe(630);
    expect(write.mock.calls).toEqual([[630]]);
    expect(ref.current?.physicalSnapshot(20, '[data-tool="item-20"]')).toMatchObject({
      viewportTop: 630, targetMeasured: true, targetOffset: 0,
      visibleRange: { startIndex: 20, endIndex: 20 },
    });
  });

  it('denies owned writes without matching authority while generic scrolling remains valid', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const items = makeItems(40);
    const getKey = (item: TestItem) => item.id;
    const mounted = render(<VirtualTranscript ref={ref} ariaLabel="Transcript" items={items} getKey={getKey}
      estimatedExtent={20} initialTail={false} renderItem={renderRow} />);
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    act(() => ref.current!.scrollToOwnedIndex('missing-authority', 20, 'start'));
    expect(scroller.scrollTop).toBe(0);
    act(() => ref.current!.scrollToIndex(10, 'start'));
    expect(scroller.scrollTop).toBe(200);
    mounted.rerender(<VirtualTranscript ref={ref} ariaLabel="Transcript" items={items} getKey={getKey}
      estimatedExtent={20} initialTail={false} renderItem={renderRow} positioningAuthority={{ kind: 'command', commandKey: 'current' }} />);
    act(() => ref.current!.scrollToOwnedIndex('stale', 20, 'start'));
    expect(scroller.scrollTop).toBe(200);
    act(() => ref.current!.scrollToOwnedIndex('current', 20, 'start'));
    expect(scroller.scrollTop).toBe(400);
  });

  it.each(['idle', 'view-reset', 'replacement', 'unchanged'] as const)('fences hidden replay before the parent layout input change (%s)', (transition) => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const items = makeItems(40);
    const getKey = (item: TestItem) => item.id;
    const oldCommand: HistoryScrollCommand = { kind: 'jump_to_message', token: 1, requestToken: 11,
      view: { conversationId: 'conv', generation: 1, transcriptGeneration: 1 }, targetMessageId: 'item-20' };
    const replacement: HistoryScrollCommand = { ...oldCommand, token: 2, targetMessageId: 'item-10' };
    const finished = vi.fn();
    const order: string[] = [];
    // The parent uses the same reducer/effect/layout boundary as MessageList.
    function Parent({ input, hidden }: { input: TranscriptPositioningInput; hidden: boolean }) {
      const state = useRef(initialTranscriptPositioningState(oldCommand.view));
      const dispatch = useCallback((event: TranscriptPositioningEvent): void => {
        const next = reduceTranscriptPositioning(state.current, event);
        state.current = next.state;
        for (const effect of next.effects) {
          if (effect.type === 'resolve_target') dispatch({ type: 'target_resolved', commandKey: effect.commandKey,
            targetIndex: effect.targetMessageId === 'item-20' ? 20 : 10 });
          if (effect.type === 'position') {
            ref.current!.scrollToOwnedIndex(effect.commandKey, effect.targetIndex, 'start');
            const snapshot = ref.current!.physicalSnapshot(effect.targetIndex);
            dispatch({ type: 'position_issued', commandKey: effect.commandKey, targetIndex: effect.targetIndex, layoutRevision: snapshot.layoutRevision });
          }
          if (effect.type === 'finish') {
            if (effect.result === 'superseded') ref.current!.cancelPosition(effect.commandKey);
            finished(effect.command.token, effect.result, effect.command.view);
          }
        }
      }, []);
      useLayoutEffect(() => {
        order.push('parent-input');
        dispatch({ type: 'input_changed', input });
      }, [dispatch, input]);
      return <div data-testid="parent-wrapper" style={{ display: hidden ? 'none' : '' }}>
        <VirtualTranscript ref={ref} ariaLabel="Transcript" items={items} getKey={getKey} estimatedExtent={20}
          initialTail={false} renderItem={renderRow}
          positioningAuthority={input.kind === 'positioning'
            ? { kind: 'command', commandKey: transcriptPositioningCommandKey(input.command) } : { kind: 'absent' }}
          onRangeChange={() => {
            order.push('child-observation');
            const active = state.current.active;
            const phase = state.current.phase;
            if (active && phase?.kind === 'awaiting_physical') {
              const snapshot = ref.current!.physicalSnapshot(phase.targetIndex);
              dispatch({ type: 'physical_observed', commandKey: active.key, range: snapshot.visibleRange,
                actualOffset: null, layoutRevision: snapshot.layoutRevision, targetMeasured: snapshot.targetMeasured ?? false });
            }
          }} />
      </div>;
    }
    const initial: TranscriptPositioningInput = { kind: 'positioning', command: oldCommand };
    const mounted = render(<Parent input={{ kind: 'idle', view: oldCommand.view }} hidden={false} />);
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    let nativeTop = 0;
    const write = vi.fn((top: number) => { order.push(`write:${top}`); if (!isHiddenByAncestor(scroller)) nativeTop = top; });
    Object.defineProperty(scroller, 'scrollTop', { configurable: true, get: () => nativeTop, set: write });
    mounted.rerender(<Parent input={initial} hidden={true} />);
    expect(ref.current!.physicalSnapshot(20).targetMeasured).toBe(false);
    expect(finished).not.toHaveBeenCalled();
    order.length = 0;
    write.mockClear();
    const next: TranscriptPositioningInput = transition === 'unchanged' ? initial
      : transition === 'replacement' ? { kind: 'positioning', command: replacement }
      : { kind: 'idle', view: { ...oldCommand.view, generation: transition === 'view-reset' ? 2 : 1 } };
    mounted.rerender(<Parent input={next} hidden={false} />);
    if (transition === 'unchanged') {
      expect(write.mock.calls).toEqual([[400]]);
      expect(finished.mock.calls).toEqual([[1, 'applied', oldCommand.view]]);
    } else {
      expect(order.indexOf('child-observation')).toBeLessThan(order.indexOf('parent-input'));
      expect(finished.mock.calls).toEqual(transition === 'replacement'
        ? [[1, 'superseded', oldCommand.view], [2, 'applied', replacement.view]]
        : [[1, 'superseded', oldCommand.view]]);
      expect(write.mock.calls).toEqual(transition === 'replacement' ? [[200]] : []);
    }
  });

  it.each(['snapshot-first', 'observer-first'] as const)('retires superseded hidden physical work without cancelling its replacement (%s)', (order) => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const items = makeItems(40);
    items[20]!.height = 320;
    const view = (commandKey: string) => (
      <div data-testid="retained-wrapper">
        <VirtualTranscript ariaLabel="Transcript" ref={ref} items={items} getKey={(item) => item.id}
          positioningAuthority={{ kind: 'command', commandKey }}
          estimatedExtent={(item) => item.height} overscan={20} initialTail={false}
          header={<div data-height={30}>Header</div>}
          renderItem={(item) => <div data-height={item.height}>{item.label}<span data-tool={item.id}>Tool</span></div>} />
      </div>
    );
    const mounted = render(view('old-command'));
    act(() => ref.current?.scrollToIndex(20, 'start'));
    const wrapper = screen.getByTestId('retained-wrapper');
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    const tool = document.querySelector<HTMLElement>('[data-tool="item-20"]')!;
    const rect = tool.getBoundingClientRect();
    tool.getBoundingClientRect = () => isHiddenByAncestor(tool)
      ? { ...rect, top: 0, bottom: 0, height: 0 }
      : { ...rect, top: 200, bottom: 220, height: 20 };
    let nativeTop = 0;
    const write = vi.fn((top: number) => { if (!isHiddenByAncestor(scroller)) nativeTop = top; });
    Object.defineProperty(scroller, 'scrollTop', { configurable: true, get: () => nativeTop, set: write });
    const issue = (owner: string, index: number, selector?: string) => {
      mounted.rerender(view(owner));
      act(() => ref.current!.scrollToOwnedIndex(owner, index, 'start', 0, selector));
    };
    const cancel = (owner: string) => ref.current!.cancelPosition(owner);
    const reveal = () => {
      wrapper.style.display = '';
      act(() => {
        if (order === 'snapshot-first') ref.current?.physicalSnapshot();
        else resizeObservers[0]!.triggerEntries([[scroller, 100]]);
      });
    };
    wrapper.style.display = 'none';
    issue('old-command', 20, '[data-tool="item-20"]');
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({ viewportTop: 430, visibleRange: null, targetMeasured: false });
    act(() => cancel('old-command'));
    reveal();
    expect(write).not.toHaveBeenCalled();
    expect(ref.current?.physicalSnapshot()).toMatchObject({ viewportTop: 0, visibleRange: { startIndex: 0, endIndex: 3 } });
    act(() => expect(ref.current?.captureVisibleAnchor()).toEqual({ index: 0, key: 'item-0', offset: 30 }));

    wrapper.style.display = 'none';
    issue('old-command', 20, '[data-tool="item-20"]');
    issue('new-command', 10);
    act(() => cancel('old-command'));
    reveal();
    expect(write.mock.calls).toEqual([[230]]);
    expect(ref.current?.physicalSnapshot(10)).toMatchObject({ viewportTop: 230, targetOffset: 0 });
  });

  it.each(['snapshot-first', 'observer-first'] as const)('genuine scrolling retires a hidden target and owed write while preserving the reader anchor (%s)', (order) => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    render(<div data-testid="retained-wrapper"><VirtualTranscript ariaLabel="Transcript" ref={ref}
      items={makeItems(40)} getKey={(item) => item.id} estimatedExtent={20} initialTail={false} renderItem={renderRow} /></div>);
    const wrapper = screen.getByTestId('retained-wrapper');
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    let nativeTop = 0;
    const write = vi.fn((top: number) => { if (!isHiddenByAncestor(scroller)) nativeTop = top; });
    Object.defineProperty(scroller, 'scrollTop', { configurable: true, get: () => nativeTop, set: write });
    wrapper.style.display = 'none';
    act(() => ref.current?.scrollToIndex(20, 'start', 0, '[data-missing-tool]'));
    wrapper.style.display = '';
    nativeTop = 80;
    fireEvent.scroll(scroller);
    act(() => {
      if (order === 'snapshot-first') ref.current?.physicalSnapshot();
      else resizeObservers[0]!.triggerEntries([[scroller, 100]]);
    });
    expect(write).not.toHaveBeenCalled();
    expect(ref.current?.physicalSnapshot()).toMatchObject({ viewportTop: 80, visibleRange: { startIndex: 4, endIndex: 8 } });
    act(() => expect(ref.current?.captureVisibleAnchor()).toEqual({ index: 4, key: 'item-4', offset: 0 }));
  });

  it('does not acknowledge an unresolved selector as a measured row and lets an absolute position supersede it', () => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    render(
      <div data-testid="retained-wrapper">
        <VirtualTranscript
          ariaLabel="Transcript"
          ref={ref}
          items={makeItems(40)}
          getKey={(item) => item.id}
          estimatedExtent={20}
          initialTail={false}
          renderItem={renderRow}
        />
      </div>,
    );
    const wrapper = screen.getByTestId('retained-wrapper');
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    wrapper.style.display = 'none';
    act(() => ref.current?.scrollToIndex(20, 'start', 0, '[data-missing-tool]'));
    wrapper.style.display = '';
    act(() => resizeObservers[0]!.triggerEntries([[scroller, 100]]));
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({ targetMeasured: false });
    act(() => ref.current?.scrollToIndex(20, 'start', 10));
    expect(scroller.scrollTop).toBe(390);
    expect(ref.current?.physicalSnapshot(20)).toMatchObject({ targetMeasured: true, targetOffset: 10 });
  });

  it.each(['observer-only', 'snapshot-first'] as const)('remeasures a mounted row that becomes genuinely zero while hidden (%s)', (order) => {
    const ref = { current: null as VirtualTranscriptHandle | null };
    const totals: number[] = [];
    const ranges: VirtualTranscriptPhysicalSnapshot[] = [];
    render(
      <div data-testid="retained-wrapper">
        <VirtualTranscript
          ariaLabel="Transcript"
          ref={ref}
          items={makeItems(20)}
          getKey={(item) => item.id}
          estimatedExtent={20}
          overscan={100}
          initialTail={false}
          renderItem={renderRow}
          onTotalExtentChange={(extent) => totals.push(extent)}
          onRangeChange={(snapshot) => ranges.push(snapshot)}
        />
      </div>,
    );
    act(() => ref.current?.scrollToIndex(3, 'start'));
    const wrapper = screen.getByTestId('retained-wrapper');
    const scroller = document.querySelector<HTMLElement>('.virtual-transcript')!;
    const row = document.querySelector<HTMLElement>('[data-virtual-index="0"]')!;
    expect(scroller.scrollTop).toBe(60);
    expect(totals.at(-1)).toBe(400);
    wrapper.style.display = 'none';
    screen.getByTestId('payload-item-0').setAttribute('data-height', '0');
    act(() => resizeObservers[0]!.triggerEntries([[scroller, 0], [row, 0]]));
    expect(totals.at(-1)).toBe(400);

    wrapper.style.display = '';
    act(() => {
      if (order === 'snapshot-first') {
        expect(ref.current?.physicalSnapshot(3)).toMatchObject({ viewportTop: 40, targetOffset: 0 });
      }
      // The row's last observed size was already zero: only the viewport reports reveal.
      resizeObservers[0]!.triggerEntries([[scroller, 100]]);
    });
    expect(totals.at(-1)).toBe(380);
    expect(document.querySelector<HTMLElement>('.virtual-transcript__inner')!.style.height).toBe('380px');
    expect(scroller.scrollTop).toBe(40);
    expect(ranges.at(-1)?.visibleRange).toEqual({ startIndex: 3, endIndex: 7 });
    expect(ref.current?.physicalSnapshot(3)).toMatchObject({ targetMeasured: true, targetOffset: 0 });
  });

  it('accepts a legitimate zero-height row while its ancestors remain displayed', () => {
    const totals: number[] = [];
    render(
      <VirtualTranscript
        ariaLabel="Transcript"
        items={makeItems(10)}
        getKey={(item) => item.id}
        estimatedExtent={20}
        overscan={20}
        initialTail={false}
        renderItem={renderRow}
        onTotalExtentChange={(extent) => totals.push(extent)}
      />,
    );
    const row = virtualRows()[0]!;
    screen.getByTestId('payload-item-0').setAttribute('data-height', '0');
    act(() => resizeObservers[0]!.triggerEntries([[row, 0]]));
    expect(totals.at(-1)).toBe(180);
  });

  it('updates viewport geometry through ResizeObserver before publishing range', () => {
    const ranges: VirtualTranscriptPhysicalSnapshot[] = [];

    render(
      <VirtualTranscript
        ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
        ariaLabel="Transcript"
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
          ariaLabel="Transcript"
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
