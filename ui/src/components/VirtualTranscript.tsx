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

export interface VirtualTranscriptPhysicalSnapshot {
  /** Inclusive rows mounted in the DOM, including overscan-only rows. */
  renderedRange: VirtualTranscriptRange | null;
  /** Inclusive rows with positive-area intersection with the viewport. */
  visibleRange: VirtualTranscriptRange | null;
  viewportTop: number;
  layoutRevision: number;
  targetIndex?: number;
  targetOffset?: number | null;
}

export type VirtualTranscriptRangeChange = VirtualTranscriptPhysicalSnapshot;

export interface VirtualTranscriptAnchor {
  index: number;
  key: string;
  offset: number;
}

export interface VirtualTranscriptHandle {
  scrollToIndex(index: number, align: 'start' | 'end', viewportStartOffset?: number): void;
  scrollToTail(): void;
  captureVisibleAnchor(): VirtualTranscriptAnchor | null;
  measureOffsetForIndex(index: number): number | null;
  measureOffsetForIndexAtSnapshot(index: number, snapshot: VirtualTranscriptPhysicalSnapshot): number | null;
  layoutRevision(): number;
  physicalSnapshot(targetIndex?: number): VirtualTranscriptPhysicalSnapshot;
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
  onRangeChange?: (snapshot: VirtualTranscriptRangeChange) => void;
  onTotalExtentChange?: (totalExtent: number) => void;
  onPinnedChange?: (pinned: boolean) => void;
}

interface PhysicalStore<T> {
  items: readonly T[];
  keys: string[];
  getKey: (item: T, index: number) => string;
  estimatedExtent: VirtualTranscriptProps<T>['estimatedExtent'];
  measuredExtents: Map<string, number>;
  headerExtent: number;
  layout: TranscriptLayout;
  range: TranscriptRange | null;
  viewportTop: number;
  viewportExtent: number;
  overscan: number;
  activeAnchor: VirtualTranscriptAnchor | null;
  scroller: HTMLDivElement | null;
  headerElement: HTMLDivElement | null;
  rowElements: Map<string, HTMLDivElement>;
  resizeObserver: ResizeObserver | null;
  initialTailPending: boolean;
  pinned: boolean;
  revision: number;
}

interface StorePublisher<T> {
  store: PhysicalStore<T>;
  publish: () => void;
}

const DEFAULT_ESTIMATED_EXTENT = 1;
const PINNED_EPSILON = 1;

function clampNonNegative(value: number): number {
  return Number.isFinite(value) && value > 0 ? value : 0;
}

function normalizeRange(range: TranscriptRange | null): VirtualTranscriptRange | null {
  return range ? { startIndex: range.startIndex, endIndex: range.endIndex } : null;
}

function computeVisibleRange<T>(store: PhysicalStore<T>): TranscriptRange | null {
  const viewportStart = Math.max(store.viewportTop, store.headerExtent);
  const viewportEnd = Math.min(store.viewportTop + store.viewportExtent, totalPhysicalExtent(store));
  const clippedExtent = viewportEnd - viewportStart;
  if (clippedExtent <= 0) return null;
  return store.layout.rangeForViewport({
    viewportOffset: viewportStart - store.headerExtent,
    viewportExtent: clippedExtent,
    overscanExtent: 0,
  });
}

function buildPhysicalSnapshot<T>(store: PhysicalStore<T>, targetIndex?: number): VirtualTranscriptPhysicalSnapshot {
  const visibleRange = computeVisibleRange(store);
  const baseSnapshot = {
    renderedRange: normalizeRange(store.range),
    visibleRange: normalizeRange(visibleRange),
    viewportTop: store.viewportTop,
    layoutRevision: store.revision,
  } satisfies Omit<VirtualTranscriptPhysicalSnapshot, 'targetIndex' | 'targetOffset'>;
  if (targetIndex === undefined) return baseSnapshot;
  const offset = itemPhysicalOffset(store, targetIndex);
  return {
    ...baseSnapshot,
    targetIndex,
    targetOffset: offset === undefined ? null : offset - store.viewportTop,
  };
}

function synchronizedPhysicalSnapshot<T>(store: PhysicalStore<T>, targetIndex?: number): VirtualTranscriptPhysicalSnapshot {
  store.viewportTop = store.scroller?.scrollTop ?? store.viewportTop;
  store.viewportExtent = store.scroller?.clientHeight ?? store.viewportExtent;
  recompute(store);
  return buildPhysicalSnapshot(store, targetIndex);
}

function measureOffsetForIndexInStore<T>(store: PhysicalStore<T>, index: number): number | null {
  const snapshot = synchronizedPhysicalSnapshot(store, index);
  return snapshot.targetIndex === index ? snapshot.targetOffset ?? null : null;
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

function totalPhysicalExtent<T>(store: PhysicalStore<T>): number {
  return store.headerExtent + store.layout.totalExtent;
}

function rowViewportOffset<T>(store: PhysicalStore<T>): number {
  return Math.max(0, store.viewportTop - store.headerExtent);
}

function itemPhysicalOffset<T>(store: PhysicalStore<T>, index: number): number | undefined {
  const item = store.layout.itemAt(index);
  return item ? store.headerExtent + item.offset : undefined;
}

function itemPhysicalEnd<T>(store: PhysicalStore<T>, index: number): number | undefined {
  const item = store.layout.itemAt(index);
  return item ? store.headerExtent + item.end : undefined;
}

function computePinned<T>(store: PhysicalStore<T>): boolean {
  const maxScrollTop = Math.max(0, totalPhysicalExtent(store) - store.viewportExtent);
  return maxScrollTop - store.viewportTop <= PINNED_EPSILON;
}

function computeRange<T>(store: PhysicalStore<T>): TranscriptRange | null {
  return store.layout.rangeForViewport({
    viewportOffset: rowViewportOffset(store),
    viewportExtent: store.viewportExtent,
    overscanExtent: store.overscan,
  });
}

function setScrollerScrollTop<T>(store: PhysicalStore<T>, nextTop: number): void {
  const scroller = store.scroller;
  const maxScrollTop = Math.max(0, totalPhysicalExtent(store) - store.viewportExtent);
  const scrollTop = Math.max(0, Math.min(nextTop, maxScrollTop));
  store.viewportTop = scrollTop;
  if (scroller && scroller.scrollTop !== scrollTop) {
    scroller.scrollTop = scrollTop;
  }
}

function captureTopAnchor<T>(store: PhysicalStore<T>): VirtualTranscriptAnchor | null {
  if (store.layout.count === 0) return null;
  const index = store.layout.indexAtOffset(rowViewportOffset(store));
  const unit = store.layout.itemAt(index);
  if (!unit) return null;
  return {
    index,
    key: unit.key,
    offset: store.headerExtent + unit.offset - store.viewportTop,
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
  setScrollerScrollTop(store, store.headerExtent + nextOffset - anchor.offset);
}

function recompute<T>(store: PhysicalStore<T>): void {
  store.layout = buildStoreLayout(store);
  store.range = computeRange(store);
  store.pinned = computePinned(store);
  store.revision += 1;
}

function measureElementExtent(element: Element): number {
  return clampNonNegative(element.getBoundingClientRect().height);
}

function updateMeasuredExtent<T>(store: PhysicalStore<T>, key: string, nextExtent: number): boolean {
  if (store.measuredExtents.get(key) === nextExtent) return false;
  store.measuredExtents.set(key, nextExtent);
  return true;
}

function applyPhysicalChange<T>(store: PhysicalStore<T>, anchor: VirtualTranscriptAnchor | null, wasPinned: boolean): void {
  store.layout = buildStoreLayout(store);
  if (store.scroller && (wasPinned || store.initialTailPending)) {
    store.initialTailPending = false;
    setScrollerScrollTop(store, totalPhysicalExtent(store));
  } else if (!store.initialTailPending) {
    applyAnchor(store, anchor);
  }
  store.activeAnchor = anchor;
  recompute(store);
}

function handleResizeEntries<T>({ store, publish }: StorePublisher<T>, entries: ResizeObserverEntry[]): void {
  let physicalChanged = false;
  let viewportChanged = false;
  const anchor = store.pinned ? null : (store.activeAnchor ?? captureTopAnchor(store));
  const wasPinned = store.pinned;

  for (const entry of entries) {
    const target = entry.target;
    const entryHeight = clampNonNegative(entry.contentRect.height);
    if (target === store.scroller) {
      const nextExtent = entryHeight || store.scroller?.clientHeight || 0;
      if (store.viewportExtent !== nextExtent) {
        store.viewportExtent = clampNonNegative(nextExtent);
        viewportChanged = true;
      }
      const nextTop = store.scroller?.scrollTop ?? store.viewportTop;
      if (store.viewportTop !== nextTop) {
        store.viewportTop = nextTop;
        viewportChanged = true;
      }
      continue;
    }

    if (target === store.headerElement) {
      const nextExtent = entryHeight || measureElementExtent(target);
      if (store.headerExtent !== nextExtent) {
        store.headerExtent = nextExtent;
        physicalChanged = true;
      }
      continue;
    }

    if (target instanceof HTMLElement) {
      const key = target.dataset['virtualKey'];
      if (!key) continue;
      const nextExtent = entryHeight || measureElementExtent(target);
      physicalChanged = updateMeasuredExtent(store, key, nextExtent) || physicalChanged;
    }
  }

  if (physicalChanged) {
    applyPhysicalChange(store, anchor, wasPinned);
    publish();
    return;
  }

  if (viewportChanged) {
    recompute(store);
    publish();
  }
}

function ensureResizeObserver<T>(store: PhysicalStore<T>, publish: () => void): ResizeObserver | null {
  if (store.resizeObserver) return store.resizeObserver;
  if (typeof ResizeObserver === 'undefined') return null;
  store.resizeObserver = new ResizeObserver((entries) => handleResizeEntries({ store, publish }, entries));
  return store.resizeObserver;
}

function observeElement<T>(store: PhysicalStore<T>, publish: () => void, element: Element): void {
  ensureResizeObserver(store, publish)?.observe(element);
}

function unobserveElement<T>(store: PhysicalStore<T>, element: Element | null): void {
  if (element) store.resizeObserver?.unobserve(element);
}

function createStore<T>(props: VirtualTranscriptProps<T>): PhysicalStore<T> {
  const keys = resolveKeys(props.items, props.getKey);
  const store: PhysicalStore<T> = {
    items: props.items,
    keys,
    getKey: props.getKey,
    estimatedExtent: props.estimatedExtent,
    measuredExtents: new Map(),
    headerExtent: 0,
    layout: buildTranscriptLayout({ keys: [], estimatedExtent: DEFAULT_ESTIMATED_EXTENT }),
    range: null,
    viewportTop: 0,
    viewportExtent: 0,
    overscan: clampNonNegative(props.overscan ?? 0),
    activeAnchor: null,
    scroller: null,
    headerElement: null,
    rowElements: new Map(),
    resizeObserver: null,
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
    store.items = items;
    store.getKey = getKey;
    store.keys = resolveKeys(items, getKey);
    store.estimatedExtent = estimatedExtent;
    store.overscan = clampNonNegative(overscan);
    const presentKeys = new Set(store.keys);
    for (const key of store.measuredExtents.keys()) {
      if (!presentKeys.has(key)) store.measuredExtents.delete(key);
    }
    applyPhysicalChange(store, anchor, wasPinned);
  }

  const publish = useCallback(() => {
    publishRevision();
  }, []);

  const rowRefCallbacks = useRef(new Map<string, (element: HTMLDivElement | null) => void>());

  const getRowRef = useCallback((key: string) => {
    let callback = rowRefCallbacks.current.get(key);
    if (callback) return callback;
    callback = (element: HTMLDivElement | null) => {
      const current = storeRef.current;
      if (!current) return;
      const previous = current.rowElements.get(key) ?? null;
      if (previous && previous !== element) {
        unobserveElement(current, previous);
        current.rowElements.delete(key);
      }
      if (!element) return;
      current.rowElements.set(key, element);
      const anchor = current.pinned ? null : (current.activeAnchor ?? captureTopAnchor(current));
      const wasPinned = current.pinned;
      const changed = updateMeasuredExtent(current, key, measureElementExtent(element));
      observeElement(current, publish, element);
      if (changed) {
        applyPhysicalChange(current, anchor, wasPinned);
        publish();
      }
    };
    rowRefCallbacks.current.set(key, callback);
    return callback;
  }, [publish]);

  const headerCallback = useCallback((element: HTMLDivElement | null) => {
    const current = storeRef.current;
    if (!current) return;
    if (current.headerElement && current.headerElement !== element) {
      unobserveElement(current, current.headerElement);
    }
    current.headerElement = element;
    const anchor = current.pinned ? null : captureTopAnchor(current);
    const wasPinned = current.pinned;
    const nextExtent = element ? measureElementExtent(element) : 0;
    const changed = current.headerExtent !== nextExtent;
    current.headerExtent = nextExtent;
    if (element) observeElement(current, publish, element);
    if (changed) {
      applyPhysicalChange(current, anchor, wasPinned);
      publish();
    }
  }, [publish]);

  const scrollerCallback = useCallback((element: HTMLDivElement | null) => {
    const current = storeRef.current;
    if (!current) return;
    if (current.scroller && current.scroller !== element) {
      unobserveElement(current, current.scroller);
    }
    current.scroller = element;
    if (element) {
      current.viewportTop = element.scrollTop;
      current.viewportExtent = element.clientHeight;
      observeElement(current, publish, element);
      if (current.initialTailPending && current.layout.count > 0) {
        current.initialTailPending = false;
        setScrollerScrollTop(current, totalPhysicalExtent(current));
      }
    }
    recompute(current);
    scrollerRef?.(element);
  }, [publish, scrollerRef]);


  useLayoutEffect(() => {
    const current = storeRef.current;
    if (!current) return;
    const anchor = current.pinned ? null : captureTopAnchor(current);
    const wasPinned = current.pinned;
    current.items = items;
    current.getKey = getKey;
    current.keys = resolveKeys(items, getKey);
    current.estimatedExtent = estimatedExtent;
    current.overscan = clampNonNegative(overscan);
    const presentKeys = new Set(current.keys);
    for (const key of current.measuredExtents.keys()) {
      if (!presentKeys.has(key)) current.measuredExtents.delete(key);
    }
    applyPhysicalChange(current, anchor, wasPinned);
    publish();
  }, [estimatedExtent, getKey, items, overscan, publish]);

  useLayoutEffect(() => {
    const current = storeRef.current;
    return () => {
      current?.resizeObserver?.disconnect();
      current?.rowElements.clear();
      if (current) {
        current.headerElement = null;
        current.scroller = null;
        current.resizeObserver = null;
      }
    };
  }, []);

  useLayoutEffect(() => {
    onRangeChange?.(buildPhysicalSnapshot(store));
  }, [onRangeChange, store, store.range, store.revision]);

  useLayoutEffect(() => {
    onTotalExtentChange?.(totalPhysicalExtent(store));
  }, [onTotalExtentChange, store, store.headerExtent, store.layout.totalExtent, store.revision]);

  useLayoutEffect(() => {
    onPinnedChange?.(store.pinned);
  }, [onPinnedChange, store.pinned, store.revision]);

  useImperativeHandle(ref, () => ({
    scrollToIndex(index, align, viewportStartOffset = 0) {
      const current = storeRef.current;
      if (!current) return;
      const unit = current.layout.itemAt(index);
      if (!unit) return;
      const physicalOffset = current.headerExtent + unit.offset;
      const physicalEnd = current.headerExtent + unit.end;
      const target = align === 'end'
        ? physicalEnd - current.viewportExtent + viewportStartOffset
        : physicalOffset - viewportStartOffset;
      current.activeAnchor = { index, key: unit.key, offset: physicalOffset - target };
      setScrollerScrollTop(current, target);
      recompute(current);
      publish();
    },
    scrollToTail() {
      const current = storeRef.current;
      if (!current) return;
      current.activeAnchor = null;
      setScrollerScrollTop(current, totalPhysicalExtent(current));
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
    measureOffsetForIndex(index) {
      const current = storeRef.current;
      if (!current) return null;
      return measureOffsetForIndexInStore(current, index);
    },
    measureOffsetForIndexAtSnapshot(index, snapshot) {
      if (snapshot.targetIndex !== index) return null;
      return snapshot.targetOffset ?? null;
    },
    layoutRevision() {
      return storeRef.current?.revision ?? 0;
    },
    physicalSnapshot(targetIndex) {
      const current = storeRef.current;
      return current
        ? synchronizedPhysicalSnapshot(current, targetIndex)
        : {
            renderedRange: null,
            visibleRange: null,
            viewportTop: 0,
            layoutRevision: 0,
            ...(targetIndex === undefined ? {} : { targetIndex, targetOffset: null }),
          };
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

  const range = store.range;
  const visibleItems = range
    ? items.slice(range.startIndex, range.endIndex + 1)
    : [];
  const topSpacer = range ? store.layout.itemAt(range.startIndex)?.offset ?? 0 : 0;
  const rangePhysicalEnd = range ? itemPhysicalEnd(store, range.endIndex) ?? store.headerExtent : store.headerExtent;
  const bottomSpacer = range
    ? Math.max(0, totalPhysicalExtent(store) - rangePhysicalEnd)
    : Math.max(0, totalPhysicalExtent(store) - store.headerExtent);
  const rootClassName = className ? `virtual-transcript ${className}` : 'virtual-transcript';

  return (
    <div
      ref={scrollerCallback}
      className={rootClassName}
      style={{ overflowAnchor: 'none' }}
      onScroll={handleScroll}
    >
      <div className="virtual-transcript__inner" style={{ height: totalPhysicalExtent(store) }}>
        {header ? (
          <div ref={headerCallback} className="virtual-transcript__header" data-virtual-header="true">
            {header}
          </div>
        ) : null}
        {items.length === 0 ? (
          <div className="virtual-transcript__empty">{empty}</div>
        ) : (
          <>
            <div className="virtual-transcript__spacer" style={{ height: topSpacer }} />
          {visibleItems.map((item, offset) => {
            const index = (range?.startIndex ?? 0) + offset;
            const key = getKey(item, index);
            return (
              <div
                key={key}
                ref={getRowRef(key)}
                className="virtual-transcript__row"
                data-virtual-index={index}
                data-virtual-key={key}
              >
                {renderItem(item, index)}
              </div>
            );
          })}
            <div className="virtual-transcript__spacer" style={{ height: bottomSpacer }} />
          </>
        )}
      </div>
    </div>
  );
}

export const VirtualTranscript = forwardRef(VirtualTranscriptInner) as <T>(
  props: VirtualTranscriptProps<T> & { ref?: React.ForwardedRef<VirtualTranscriptHandle> },
) => ReturnType<typeof VirtualTranscriptInner>;
