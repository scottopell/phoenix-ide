import {
  forwardRef,
  useCallback,
  useImperativeHandle,
  useLayoutEffect,
  useReducer,
  useRef,
  type ReactNode,
} from 'react';
import {
  buildTranscriptLayout,
  type TranscriptLayout,
  type TranscriptRange,
} from '../conversation/virtualTranscriptLayout';
import './VirtualTranscript.css';

export interface VirtualTranscriptRange {
  startIndex: number;
  endIndex: number;
}

export interface VirtualTranscriptAnchor {
  index: number;
  key: string;
  offset: number;
}

export interface VirtualTranscriptHandle {
  scrollToIndex(index: number, align: 'start' | 'end', viewportStartOffset?: number): void;
  scrollToTail(): void;
  captureVisibleAnchor(): VirtualTranscriptAnchor | null;
}

export interface VirtualTranscriptProps<T> {
  items: readonly T[];
  getKey: (item: T, index: number) => string;
  renderItem: (item: T, index: number) => ReactNode;
  header?: ReactNode;
  empty?: ReactNode;
  overscan?: number;
  initialTail?: boolean;
  estimatedExtent: number | ((item: T, index: number) => number);
  className?: string;
  scrollerRef?: (element: HTMLDivElement | null) => void;
  onRangeChange?: (range: VirtualTranscriptRange | null) => void;
  onTotalExtentChange?: (totalExtent: number) => void;
  onPinnedChange?: (pinned: boolean) => void;
}

interface PhysicalStore<T> {
  items: readonly T[];
  keys: string[];
  getKey: (item: T, index: number) => string;
  estimatedExtent: VirtualTranscriptProps<T>['estimatedExtent'];
  measuredExtents: Map<string, number>;
  layout: TranscriptLayout;
  range: TranscriptRange | null;
  viewportTop: number;
  viewportExtent: number;
  overscan: number;
  activeAnchor: VirtualTranscriptAnchor | null;
  scroller: HTMLDivElement | null;
  initialTailPending: boolean;
  pinned: boolean;
  revision: number;
}

const DEFAULT_ESTIMATED_EXTENT = 1;
const PINNED_EPSILON = 1;

function clampNonNegative(value: number): number {
  return Number.isFinite(value) && value > 0 ? value : 0;
}

function normalizeRange(range: TranscriptRange | null): VirtualTranscriptRange | null {
  return range ? { startIndex: range.startIndex, endIndex: range.endIndex } : null;
}

function resolveKeys<T>(
  items: readonly T[],
  getKey: (item: T, index: number) => string,
): string[] {
  return items.map((item, index) => getKey(item, index));
}

function estimatedExtentForKey<T>(store: PhysicalStore<T>) {
  return (_key: string, index: number) => {
    const item = store.items[index];
    if (item === undefined) return DEFAULT_ESTIMATED_EXTENT;
    return typeof store.estimatedExtent === 'function'
      ? store.estimatedExtent(item, index)
      : store.estimatedExtent;
  };
}

function buildStoreLayout<T>(store: PhysicalStore<T>): TranscriptLayout {
  return buildTranscriptLayout({
    keys: store.keys,
    estimatedExtent: estimatedExtentForKey(store),
    measuredExtents: store.measuredExtents,
  });
}

function computePinned<T>(store: PhysicalStore<T>): boolean {
  const maxScrollTop = Math.max(0, store.layout.totalExtent - store.viewportExtent);
  return maxScrollTop - store.viewportTop <= PINNED_EPSILON;
}

function computeRange<T>(store: PhysicalStore<T>): TranscriptRange | null {
  return store.layout.rangeForViewport({
    viewportOffset: store.viewportTop,
    viewportExtent: store.viewportExtent,
    overscanExtent: store.overscan,
  });
}

function setScrollerScrollTop<T>(store: PhysicalStore<T>, nextTop: number): void {
  const scroller = store.scroller;
  const maxScrollTop = Math.max(0, store.layout.totalExtent - store.viewportExtent);
  const scrollTop = Math.max(0, Math.min(nextTop, maxScrollTop));
  store.viewportTop = scrollTop;
  if (scroller && scroller.scrollTop !== scrollTop) {
    scroller.scrollTop = scrollTop;
  }
}

function captureTopAnchor<T>(store: PhysicalStore<T>): VirtualTranscriptAnchor | null {
  if (store.layout.count === 0) return null;
  const index = store.layout.indexAtOffset(store.viewportTop);
  const unit = store.layout.itemAt(index);
  if (!unit) return null;
  return {
    index,
    key: unit.key,
    offset: unit.offset - store.viewportTop,
  };
}

function applyAnchor<T>(store: PhysicalStore<T>, anchor: VirtualTranscriptAnchor | null): void {
  if (!anchor) {
    setScrollerScrollTop(store, store.viewportTop);
    return;
  }
  const nextOffset = store.layout.offsetForKey(anchor.key);
  if (nextOffset === undefined) {
    setScrollerScrollTop(store, store.viewportTop);
    return;
  }
  setScrollerScrollTop(store, nextOffset - anchor.offset);
}

function recompute<T>(store: PhysicalStore<T>): void {
  store.layout = buildStoreLayout(store);
  store.range = computeRange(store);
  store.pinned = computePinned(store);
  store.revision += 1;
}

function createStore<T>(props: VirtualTranscriptProps<T>): PhysicalStore<T> {
  const keys = resolveKeys(props.items, props.getKey);
  const store: PhysicalStore<T> = {
    items: props.items,
    keys,
    getKey: props.getKey,
    estimatedExtent: props.estimatedExtent,
    measuredExtents: new Map(),
    layout: buildTranscriptLayout({ keys: [], estimatedExtent: DEFAULT_ESTIMATED_EXTENT }),
    range: null,
    viewportTop: 0,
    viewportExtent: 0,
    overscan: clampNonNegative(props.overscan ?? 0),
    activeAnchor: null,
    scroller: null,
    initialTailPending: props.initialTail ?? true,
    pinned: true,
    revision: 0,
  };
  recompute(store);
  return store;
}

function VirtualTranscriptInner<T>(
  props: VirtualTranscriptProps<T>,
  ref: React.ForwardedRef<VirtualTranscriptHandle>,
) {
  const {
    items,
    getKey,
    renderItem,
    header,
    empty,
    overscan = 0,
    initialTail = true,
    estimatedExtent,
    className,
    scrollerRef,
    onRangeChange,
    onTotalExtentChange,
    onPinnedChange,
  } = props;
  const storeRef = useRef<PhysicalStore<T> | null>(null);
  if (!storeRef.current) {
    storeRef.current = createStore(props);
  }
  const store = storeRef.current;
  const [, publishRevision] = useReducer((revision: number) => revision + 1, 0);

  if (
    store.items !== items ||
    store.getKey !== getKey ||
    store.estimatedExtent !== estimatedExtent ||
    store.overscan !== clampNonNegative(overscan)
  ) {
    const anchor = store.pinned ? null : captureTopAnchor(store);
    const wasPinned = store.pinned;
    const previousItems = store.items;
    store.items = items;
    store.getKey = getKey;
    store.keys = resolveKeys(items, getKey);
    store.estimatedExtent = estimatedExtent;
    store.overscan = clampNonNegative(overscan);
    items.forEach((item, index) => {
      if (previousItems[index] !== item) {
        store.measuredExtents.delete(store.keys[index]!);
      }
    });
    store.layout = buildStoreLayout(store);
    if (wasPinned) {
      setScrollerScrollTop(store, store.layout.totalExtent);
    } else {
      applyAnchor(store, anchor);
    }
    store.activeAnchor = anchor;
    recompute(store);
  }

  const publish = useCallback(() => {
    publishRevision();
  }, []);

  const scrollerCallback = useCallback((element: HTMLDivElement | null) => {
    const current = storeRef.current;
    if (!current) return;
    current.scroller = element;
    if (element) {
      current.viewportTop = element.scrollTop;
      current.viewportExtent = element.clientHeight;
      if (current.initialTailPending && current.layout.count > 0) {
        current.initialTailPending = false;
        setScrollerScrollTop(current, current.layout.totalExtent);
      }
    }
    recompute(current);
    scrollerRef?.(element);
  }, [scrollerRef]);

  useLayoutEffect(() => {
    const current = storeRef.current;
    if (!current) return;
    const anchor = current.pinned ? null : captureTopAnchor(current);
    current.items = items;
    current.getKey = getKey;
    current.keys = resolveKeys(items, getKey);
    current.estimatedExtent = estimatedExtent;
    current.overscan = clampNonNegative(overscan);
    current.initialTailPending = current.initialTailPending || initialTail;
    current.activeAnchor = anchor;
    current.layout = buildStoreLayout(current);
    if (current.pinned || (initialTail && current.initialTailPending)) {
      current.initialTailPending = false;
      setScrollerScrollTop(current, current.layout.totalExtent);
    } else {
      applyAnchor(current, anchor);
    }
    recompute(current);
    publish();
  }, [estimatedExtent, getKey, initialTail, items, overscan, publish]);

  useLayoutEffect(() => {
    const current = storeRef.current;
    if (!current?.scroller) return;
    const observer = new ResizeObserver((entries) => {
      const nextExtent = entries[0]?.contentRect.height ?? current.scroller?.clientHeight ?? 0;
      current.viewportExtent = clampNonNegative(nextExtent);
      current.viewportTop = current.scroller?.scrollTop ?? current.viewportTop;
      recompute(current);
      publish();
    });
    observer.observe(current.scroller);
    return () => observer.disconnect();
  }, [publish, store.scroller]);

  useLayoutEffect(() => {
    onRangeChange?.(normalizeRange(store.range));
  }, [onRangeChange, store.range, store.revision]);

  useLayoutEffect(() => {
    onTotalExtentChange?.(store.layout.totalExtent);
  }, [onTotalExtentChange, store.layout.totalExtent, store.revision]);

  useLayoutEffect(() => {
    onPinnedChange?.(store.pinned);
  }, [onPinnedChange, store.pinned, store.revision]);

  useLayoutEffect(() => {
    const current = storeRef.current;
    if (!current?.scroller) return;
    const rows = Array.from(
      current.scroller.querySelectorAll<HTMLElement>('[data-virtual-key]'),
    );
    let changed = false;
    const anchor = current.activeAnchor ?? captureTopAnchor(current);
    for (const row of rows) {
      const key = row.dataset['virtualKey'];
      if (!key) continue;
      const nextExtent = clampNonNegative(row.getBoundingClientRect().height);
      if (current.measuredExtents.get(key) !== nextExtent) {
        current.measuredExtents.set(key, nextExtent);
        changed = true;
      }
    }
    if (!changed) return;
    current.layout = buildStoreLayout(current);
    applyAnchor(current, anchor);
    current.activeAnchor = anchor;
    recompute(current);
    publish();
  });

  useImperativeHandle(ref, () => ({
    scrollToIndex(index, align, viewportStartOffset = 0) {
      const current = storeRef.current;
      if (!current) return;
      const unit = current.layout.itemAt(index);
      if (!unit) return;
      const target = align === 'end'
        ? unit.end - current.viewportExtent + viewportStartOffset
        : unit.offset - viewportStartOffset;
      current.activeAnchor = { index, key: unit.key, offset: unit.offset - target };
      setScrollerScrollTop(current, target);
      recompute(current);
      publish();
    },
    scrollToTail() {
      const current = storeRef.current;
      if (!current) return;
      current.activeAnchor = null;
      setScrollerScrollTop(current, current.layout.totalExtent);
      recompute(current);
      publish();
    },
    captureVisibleAnchor() {
      const current = storeRef.current;
      if (!current) return null;
      current.viewportTop = current.scroller?.scrollTop ?? current.viewportTop;
      const anchor = captureTopAnchor(current);
      current.activeAnchor = anchor;
      recompute(current);
      publish();
      return anchor;
    },
  }), [publish]);

  const handleScroll = useCallback(() => {
    const current = storeRef.current;
    if (!current?.scroller) return;
    current.viewportTop = current.scroller.scrollTop;
    current.viewportExtent = current.scroller.clientHeight;
    current.activeAnchor = captureTopAnchor(current);
    recompute(current);
    publish();
  }, [publish]);

  const setRowRef = useCallback((key: string) => (element: HTMLDivElement | null) => {
    const current = storeRef.current;
    if (!current || !element) return;
    const rawExtent = element.getBoundingClientRect().height;
    const nextExtent = clampNonNegative(rawExtent);
    if (current.measuredExtents.get(key) === nextExtent) return;
    const anchor = current.activeAnchor ?? captureTopAnchor(current);
    current.measuredExtents.set(key, nextExtent);
    current.layout = buildStoreLayout(current);
    applyAnchor(current, anchor);
    current.activeAnchor = anchor;
    recompute(current);
    publish();
  }, [publish]);

  const range = store.range;
  const visibleItems = range
    ? items.slice(range.startIndex, range.endIndex + 1)
    : [];
  const topSpacer = range ? store.layout.itemAt(range.startIndex)?.offset ?? 0 : 0;
  const bottomSpacer = range
    ? Math.max(0, store.layout.totalExtent - (store.layout.itemAt(range.endIndex)?.end ?? 0))
    : 0;
  const rootClassName = className ? `virtual-transcript ${className}` : 'virtual-transcript';

  return (
    <div
      ref={scrollerCallback}
      className={rootClassName}
      style={{ overflowAnchor: 'none' }}
      onScroll={handleScroll}
    >
      {items.length === 0 ? (
        <div className="virtual-transcript__empty">{empty}</div>
      ) : (
        <div className="virtual-transcript__inner" style={{ height: store.layout.totalExtent }}>
          {header}
          <div className="virtual-transcript__spacer" style={{ height: topSpacer }} />
          {visibleItems.map((item, offset) => {
            const index = (range?.startIndex ?? 0) + offset;
            const key = getKey(item, index);
            return (
              <div
                key={key}
                ref={setRowRef(key)}
                className="virtual-transcript__row"
                data-virtual-index={index}
                data-virtual-key={key}
              >
                {renderItem(item, index)}
              </div>
            );
          })}
          <div className="virtual-transcript__spacer" style={{ height: bottomSpacer }} />
        </div>
      )}
    </div>
  );
}

export const VirtualTranscript = forwardRef(VirtualTranscriptInner) as <T>(
  props: VirtualTranscriptProps<T> & { ref?: React.ForwardedRef<VirtualTranscriptHandle> },
) => ReturnType<typeof VirtualTranscriptInner>;
