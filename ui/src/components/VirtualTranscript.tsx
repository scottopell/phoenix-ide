import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  type ReactNode,
} from 'react';
import {
  buildTranscriptLayout,
  type TranscriptLayout,
  type TranscriptRange,
} from '../conversation/virtualTranscriptLayout';
import { flushSync } from 'react-dom';
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
  targetMeasured?: boolean;
}

export type VirtualTranscriptRangeChange = VirtualTranscriptPhysicalSnapshot;

export interface VirtualTranscriptAnchor {
  index: number;
  key: string;
  offset: number;
}

type PhysicalAnchor = VirtualTranscriptAnchor & { intent: 'directional_start' | 'continuity' };

export type VirtualTranscriptTarget = string | ((row: HTMLElement) => Range | null);

function resolveTarget(row: HTMLElement | undefined, target: VirtualTranscriptTarget | undefined): Element | Range | null {
  if (!row || !target) return null;
  return typeof target === 'string' ? row.querySelector(target) : target(row);
}

export interface VirtualTranscriptHandle {
  scrollToIndex(
    index: number,
    align: 'start' | 'end',
    viewportStartOffset?: number,
    targetSelector?: VirtualTranscriptTarget,
    startPolicy?: 'directional' | 'continuity',
  ): void;
  scrollToOwnedIndex(
    commandKey: string,
    index: number,
    align: 'start' | 'end',
    viewportStartOffset?: number,
    targetSelector?: VirtualTranscriptTarget,
    startPolicy?: 'directional' | 'continuity',
  ): void;
  completePosition(commandKey: string): void;
  interruptPosition(): void;
  cancelPosition(commandKey: string): void;
  scrollToTail(): void;
  scrollBy(delta: number): void;
  /** Grant or withdraw tail-following. The scroll policy owns this intent;
   *  the physical layer only executes it when the viewport is at the tail. */
  setTailFollowAllowed(allowed: boolean): void;
  /** Whether a scroll event at this scrollTop is the echo of a write this
   *  component made (anchor compensation, drift reconcile, tail snap) rather
   *  than user movement. */
  isProgrammaticScroll(scrollTop: number): boolean;
  captureVisibleAnchor(): VirtualTranscriptAnchor | null;
  preserveViewportOnNextItemsChange(): void;
  measureOffsetForIndex(index: number): number | null;
  measureOffsetForIndexAtSnapshot(index: number, snapshot: VirtualTranscriptPhysicalSnapshot): number | null;
  layoutRevision(): number;
  physicalSnapshot(targetIndex?: number, targetSelector?: string): VirtualTranscriptPhysicalSnapshot;
}

export type VirtualTranscriptPositioningAuthority =
  | { readonly kind: 'absent' }
  | { readonly kind: 'command'; readonly commandKey: string };

const NO_POSITIONING_AUTHORITY: VirtualTranscriptPositioningAuthority = { kind: 'absent' };

export interface VirtualTranscriptProps<T> {
  items: readonly T[];
  getKey: (item: T, index: number) => string;
  renderItem: (item: T, index: number) => ReactNode;
  /** Omission denies owned writes; generic scroll operations remain unowned. */
  readonly positioningAuthority?: VirtualTranscriptPositioningAuthority;
  positioningViewKey?: string;
  header?: ReactNode;
  empty?: ReactNode;
  overscan?: number;
  initialTail?: boolean;
  estimatedExtent: number | ((item: T, index: number) => number);
  className?: string;
  scrollerId?: string;
  /** Names the scroll region. Required: it is a landmark and a focus stop, and
   *  an unnamed one is an anonymous stop (REQ-VT-013). */
  ariaLabel: string;
  scrollerRef?: (element: HTMLDivElement | null) => void;
  onRangeChange?: (snapshot: VirtualTranscriptRangeChange) => void;
  onTotalExtentChange?: (totalExtent: number) => void;
  onPinnedChange?: (pinned: boolean) => void;
}

type PositionOwner = { kind: 'unowned' }
  | { kind: 'command'; commandKey: string }
  | { kind: 'settled'; commandKey: string; viewKey: string | null };

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
  revealPublicationPending: boolean;
  activeAnchor: PhysicalAnchor | null;
  scroller: HTMLDivElement | null;
  headerElement: HTMLDivElement | null;
  rowElements: Map<string, HTMLDivElement>;
  resizeObserver: ResizeObserver | null;
  initialTailPending: boolean;
  preservedViewport: { top: number; firstKey: string | null } | null;
  positionOwner: PositionOwner;
  positioningAuthority: VirtualTranscriptPositioningAuthority;
  positioningViewKey: string | null;
  pendingTarget: {
    key: string;
    align: 'start' | 'end';
    viewportStartOffset: number;
    startPolicy: 'directional' | 'continuity';
    selector: VirtualTranscriptTarget | undefined;
  } | null;
  pinned: boolean;
  revision: number;
  /** Physical-minus-layout offset of the rendered block, rendered through
   *  the top spacer (mid-scroll anchor compensation, REQ-VT-004). */
  drift: number;
  lastScrollAtMs: number;
  /** Identifiers of touches that began inside this scroller and have not
   *  been reported up. No event source is fully reliable here: a touch whose
   *  row is unmounted mid-gesture has its touchend dispatched at a detached
   *  node that nothing observes, while pointer events are cancelled the
   *  moment a native pan takes over and then never report the lift at all.
   *  Touch events are therefore the primary signal, pruned against each
   *  event's live list and bounded by GESTURE_STALE_MS so an unobservable
   *  lift cannot hold reconciliation open forever. */
  activeTouchIds: Set<number>;
  /** Last touch activity (down or move) on this scroller. */
  lastGestureAtMs: number;
  /** Whether the scroll policy currently grants tail-following. The physical
   *  layer does not infer this: `pinned` describes where the viewport is, not
   *  who owns it, and the two disagree whenever a reader is holding a
   *  position that happens to still be at the tail. */
  tailFollowAllowed: boolean;
  /** Absolute position a computed correction still owes the viewport, applied
   *  in a layout effect so the spacer change and the write land in one paint. Stored
   *  as a position, not an offset: anything that resynchronizes geometry from
   *  the DOM in the interval — or a browser clamping the viewport as the
   *  spacer shrinks — would otherwise re-base the offset and apply part of
   *  the shift twice (REQ-VT-004). */
  pendingScrollTop: number | null;
  /** ScrollTop value this component last wrote; cleared by the first
   *  non-matching scroll event. */
  pendingProgrammaticTop: number | null;
}

interface StorePublisher<T> {
  store: PhysicalStore<T>;
  publish: () => void;
}

const DEFAULT_ESTIMATED_EXTENT = 1;
const PINNED_EPSILON = 1;
export const SCROLL_SETTLE_MS = 150;
/** Bound on how long a touch alone may defer reconciliation (REQ-VT-004). */
export const GESTURE_STALE_MS = 2000;

function scrollIsActive<T>(store: PhysicalStore<T>): boolean {
  const now = Date.now();
  if (now - store.lastScrollAtMs < SCROLL_SETTLE_MS) return true;
  return store.activeTouchIds.size > 0 && now - store.lastGestureAtMs < GESTURE_STALE_MS;
}

/** Whether a change should follow the tail rather than hold position: the
 *  viewport is at the tail and the policy still owns it. */
function tailFollows<T>(store: PhysicalStore<T>): boolean {
  return store.pinned && store.tailFollowAllowed;
}

/** Null selects tail-following; an anchor preserves the current position. */
function anchorForChange<T>(store: PhysicalStore<T>): PhysicalAnchor | null {
  return tailFollows(store) ? null : (store.activeAnchor ?? captureTopAnchor(store));
}

function topAnchorForChange<T>(store: PhysicalStore<T>): PhysicalAnchor | null {
  if (tailFollows(store)) return null;
  const captured = captureTopAnchor(store);
  const active = store.activeAnchor;
  return captured && active?.intent === 'directional_start' && active.key === captured.key
    ? { ...captured, intent: 'directional_start' }
    : captured;
}

function clampNonNegative(value: number): number {
  return Number.isFinite(value) && value > 0 ? value : 0;
}

function isLayoutHidden(element: Element | null): boolean {
  for (let ancestor = element; ancestor; ancestor = ancestor.parentElement) {
    if (getComputedStyle(ancestor).display === 'none') return true;
  }
  return false;
}

function viewportExtentForScroller(scroller: HTMLElement, measuredExtent = scroller.clientHeight): number {
  const styles = getComputedStyle(scroller);
  const reserved = (Number.parseFloat(styles.paddingTop) || 0) + (Number.parseFloat(styles.paddingBottom) || 0);
  return clampNonNegative(measuredExtent - reserved);
}

function normalizeRange(range: TranscriptRange | null): VirtualTranscriptRange | null {
  return range ? { startIndex: range.startIndex, endIndex: range.endIndex } : null;
}

function computeVisibleRange<T>(store: PhysicalStore<T>): TranscriptRange | null {
  const settledTop = settledViewportTop(store);
  const viewportStart = Math.max(settledTop, store.headerExtent + store.drift);
  const viewportEnd = Math.min(settledTop + store.viewportExtent, totalPhysicalExtent(store));
  const clippedExtent = viewportEnd - viewportStart;
  if (clippedExtent <= 0) return null;
  return store.layout.rangeForViewport({
    viewportOffset: viewportStart - store.headerExtent - store.drift,
    viewportExtent: clippedExtent,
    overscanExtent: 0,
  });
}

function buildPhysicalSnapshot<T>(
  store: PhysicalStore<T>,
  targetIndex?: number,
  targetSelector?: string,
): VirtualTranscriptPhysicalSnapshot {
  const hidden = isLayoutHidden(store.scroller);
  if (hidden) store.revealPublicationPending = true;
  const visibleRange = hidden ? null : computeVisibleRange(store);
  const baseSnapshot = {
    renderedRange: normalizeRange(store.range),
    visibleRange: normalizeRange(visibleRange),
    viewportTop: store.viewportTop,
    layoutRevision: store.revision,
  } satisfies Omit<VirtualTranscriptPhysicalSnapshot, 'targetIndex' | 'targetOffset' | 'targetMeasured'>;
  if (targetIndex === undefined) return baseSnapshot;
  const key = store.keys[targetIndex] ?? '';
  const row = store.rowElements.get(key);
  const targetElement = resolveTarget(row, targetSelector);
  const rowOffset = itemPhysicalOffset(store, targetIndex);
  const intraRowOffset = targetElement && row
    ? targetElement.getBoundingClientRect().top - row.getBoundingClientRect().top
    : targetSelector
      ? undefined
      : 0;
  const offset = rowOffset === undefined || intraRowOffset === undefined
    ? undefined
    : rowOffset + intraRowOffset;
  return {
    ...baseSnapshot,
    targetIndex,
    targetOffset: hidden || offset === undefined ? null : offset - settledViewportTop(store),
    targetMeasured: !hidden && store.pendingTarget?.key !== key && store.measuredExtents.has(key) && (!targetSelector || targetElement !== null),
  };
}

function synchronizedPhysicalSnapshot<T>(
  store: PhysicalStore<T>,
  targetIndex?: number,
  targetSelector?: string,
): VirtualTranscriptPhysicalSnapshot {
  retireUnauthorizedPosition(store);
  if (!isLayoutHidden(store.scroller)) {
    store.viewportExtent = store.scroller ? viewportExtentForScroller(store.scroller) : store.viewportExtent;
    if (store.revealPublicationPending) {
      const anchor = anchorForChange(store);
      const wasPinned = store.pinned;
      if (measureMountedExtents(store)) applyPhysicalChange(store, anchor, wasPinned);
    }
    resolvePendingTarget(store);
    if (store.revealPublicationPending && store.pendingScrollTop !== null) {
      setScrollerScrollTop(store, store.pendingScrollTop);
    }
    store.viewportTop = store.scroller?.scrollTop ?? store.viewportTop;
    recompute(store);
  }
  return buildPhysicalSnapshot(store, targetIndex, targetSelector);
}

function measureOffsetForIndexInStore<T>(store: PhysicalStore<T>, index: number): number | null {
  const snapshot = synchronizedPhysicalSnapshot(store, index);
  return snapshot.targetIndex === index ? snapshot.targetOffset ?? null : null;
}

interface ResolvedPhysicalKeys {
  keys: string[];
  duplicateKeys: string[];
}

function resolvePhysicalKeys<T>(
  items: readonly T[],
  getKey: (item: T, index: number) => string,
): ResolvedPhysicalKeys {
  const semanticKeys = items.map(getKey);
  const reservedKeys = new Set(semanticKeys);
  const allocatedKeys = new Set<string>();
  const occurrences = new Map<string, number>();
  const duplicateKeys = new Set<string>();
  const keys = semanticKeys.map((semanticKey) => {
    const occurrence = occurrences.get(semanticKey) ?? 0;
    occurrences.set(semanticKey, occurrence + 1);
    if (occurrence === 0) {
      allocatedKeys.add(semanticKey);
      return semanticKey;
    }

    duplicateKeys.add(semanticKey);
    let discriminator = occurrence;
    let physicalKey = `${semanticKey}\u0000duplicate:${discriminator}`;
    while (reservedKeys.has(physicalKey) || allocatedKeys.has(physicalKey)) {
      discriminator += 1;
      physicalKey = `${semanticKey}\u0000duplicate:${discriminator}`;
    }
    allocatedKeys.add(physicalKey);
    return physicalKey;
  });
  return { keys, duplicateKeys: [...duplicateKeys] };
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
  return store.headerExtent + store.layout.totalExtent + store.drift;
}

/** Viewport position with any correction still owed counted as applied. Read
 *  this, not `viewportTop`, for anything describing where the reader sits
 *  relative to content (REQ-VT-004). */
function settledViewportTop<T>(store: PhysicalStore<T>): number {
  return store.pendingScrollTop ?? store.viewportTop;
}

function rowViewportOffset<T>(store: PhysicalStore<T>): number {
  return Math.max(0, settledViewportTop(store) - store.headerExtent - store.drift);
}

function itemPhysicalOffset<T>(store: PhysicalStore<T>, index: number): number | undefined {
  const item = store.layout.itemAt(index);
  return item ? store.headerExtent + item.offset + store.drift : undefined;
}

function itemPhysicalEnd<T>(store: PhysicalStore<T>, index: number): number | undefined {
  const item = store.layout.itemAt(index);
  return item ? store.headerExtent + item.end + store.drift : undefined;
}

function computePinned<T>(store: PhysicalStore<T>): boolean {
  const maxScrollTop = Math.max(0, totalPhysicalExtent(store) - store.viewportExtent);
  return maxScrollTop - settledViewportTop(store) <= PINNED_EPSILON;
}

function computeRange<T>(store: PhysicalStore<T>): TranscriptRange | null {
  return store.layout.rangeForViewport({
    viewportOffset: rowViewportOffset(store),
    viewportExtent: store.viewportExtent,
    overscanExtent: store.overscan,
  });
}

function setScrollerScrollTop<T>(store: PhysicalStore<T>, nextTop: number): boolean {
  if (retireUnauthorizedPosition(store)) return false;
  const scroller = store.scroller;
  const maxScrollTop = Math.max(0, totalPhysicalExtent(store) - store.viewportExtent);
  const scrollTop = Math.max(0, Math.min(nextTop, maxScrollTop));
  // A pending correction is relative to the position it was computed against.
  // Placing the viewport somewhere absolute retires that position, so the
  // correction goes with it — applied afterwards it would displace the
  // caller's chosen position by the drift (REQ-VT-004).
  store.viewportTop = scrollTop;
  if (isLayoutHidden(scroller)) {
    store.pendingScrollTop = scrollTop;
    store.revealPublicationPending = true;
    return false;
  }
  store.pendingScrollTop = null;
  if (scroller && scroller.scrollTop !== scrollTop) {
    store.pendingProgrammaticTop = scrollTop;
    scroller.scrollTop = scrollTop;
    return true;
  }
  return false;
}

function retirePosition<T>(current: PhysicalStore<T>): void {
  current.positionOwner = { kind: 'unowned' };
  current.pendingTarget = null;
  current.pendingScrollTop = null;
  current.pendingProgrammaticTop = null;
}

function positionIsAuthorized<T>(store: PhysicalStore<T>, owner: PositionOwner = store.positionOwner): boolean {
  return owner.kind === 'unowned'
    || (owner.kind === 'settled' && owner.viewKey === store.positioningViewKey && store.positioningAuthority.kind === 'absent')
    || (store.positioningAuthority.kind === 'command' && store.positioningAuthority.commandKey === owner.commandKey
      && (owner.kind !== 'settled' || owner.viewKey === store.positioningViewKey));
}

function retireUnauthorizedPosition<T>(store: PhysicalStore<T>): boolean {
  if (positionIsAuthorized(store)) return false;
  retirePosition(store);
  store.viewportTop = store.scroller?.scrollTop ?? store.viewportTop;
  store.activeAnchor = captureTopAnchor(store);
  recompute(store);
  return true;
}

function applyAlignedPosition<T>(
  current: PhysicalStore<T>, index: number, key: string, physicalOffset: number,
  target: number, align: 'start' | 'end', startPolicy: 'directional' | 'continuity',
): void {
  const intent = align === 'start' && startPolicy === 'directional' ? 'directional_start' : 'continuity';
  current.activeAnchor = { index, key, offset: physicalOffset - target, intent };
  let wrotePosition = setScrollerScrollTop(current, target);
  if (align !== 'start' || startPolicy === 'continuity' || !current.scroller || isLayoutHidden(current.scroller)) return;
  const ceiling = Math.max(0, Math.min(target, Math.max(0, totalPhysicalExtent(current) - current.viewportExtent)));
  const applied = current.scroller.scrollTop;
  if (applied > ceiling && applied - ceiling <= 1) {
    wrotePosition = setScrollerScrollTop(current, Math.max(0, ceiling - 1)) || wrotePosition;
  }
  const finalTop = current.scroller.scrollTop;
  current.viewportTop = finalTop;
  if (wrotePosition) current.pendingProgrammaticTop = finalTop;
  current.activeAnchor = { index, key, offset: physicalOffset - finalTop, intent };
}

function positionAtIndex<T>(
  current: PhysicalStore<T>,
  owner: PositionOwner,
  index: number,
  align: 'start' | 'end',
  viewportStartOffset: number,
  targetSelector: VirtualTranscriptTarget | undefined,
  startPolicy: 'directional' | 'continuity',
): void {
  if (!positionIsAuthorized(current, owner)) return;
  const unit = current.layout.itemAt(index);
  if (!unit) return;
  current.positionOwner = owner;
  const row = current.rowElements.get(unit.key);
  const targetElement = resolveTarget(row, targetSelector);
  const intraRowOffset = targetElement && row
    ? targetElement.getBoundingClientRect().top - row.getBoundingClientRect().top
    : 0;
  current.pendingTarget = isLayoutHidden(current.scroller) || (targetSelector && !targetElement)
    ? { key: unit.key, align, viewportStartOffset, selector: targetSelector, startPolicy }
    : null;
  const physicalOffset = current.headerExtent + unit.offset + current.drift;
  const physicalEnd = current.headerExtent + unit.end + current.drift;
  const target = align === 'end'
    ? physicalEnd - current.viewportExtent + viewportStartOffset
    : physicalOffset + intraRowOffset - viewportStartOffset;
  applyAlignedPosition(current, index, unit.key, physicalOffset, target, align, startPolicy);
  recompute(current);
}

function resolvePendingTarget<T>(current: PhysicalStore<T>): boolean {
  if (retireUnauthorizedPosition(current)) return false;
  const pending = current.pendingTarget;
  if (!pending || isLayoutHidden(current.scroller)) return false;
  const key = pending.key;
  const element = current.rowElements.get(key);
  const index = current.keys.indexOf(key);
  const targetElement = pending.selector ? resolveTarget(element, pending.selector) : element;
  const unit = current.layout.itemAt(index);
  if (!element || !targetElement || !unit) return false;
  current.pendingTarget = null;
  const intraRowOffset = targetElement.getBoundingClientRect().top - element.getBoundingClientRect().top;
  const physicalOffset = current.headerExtent + unit.offset + current.drift;
  const physicalEnd = current.headerExtent + unit.end + current.drift;
  const target = pending.align === 'end'
    ? physicalEnd - current.viewportExtent + pending.viewportStartOffset
    : physicalOffset + intraRowOffset - pending.viewportStartOffset;
  applyAlignedPosition(current, index, key, physicalOffset, target, pending.align, pending.startPolicy);
  return true;
}

function isProgrammaticScrollTop<T>(store: PhysicalStore<T>, scrollTop: number): boolean {
  return store.pendingProgrammaticTop !== null
    && Math.abs(scrollTop - store.pendingProgrammaticTop) <= 1;
}

function captureTopAnchor<T>(store: PhysicalStore<T>): PhysicalAnchor | null {
  if (store.layout.count === 0) return null;
  const index = store.layout.indexAtOffset(rowViewportOffset(store));
  const unit = store.layout.itemAt(index);
  if (!unit) return null;
  return {
    index,
    key: unit.key,
    offset: store.headerExtent + unit.offset + store.drift - settledViewportTop(store),
    intent: 'continuity',
  };
}

function applyAnchor<T>(store: PhysicalStore<T>, anchor: PhysicalAnchor | null): PhysicalAnchor | null {
  if (!anchor) {
    setScrollerScrollTop(store, settledViewportTop(store));
    return null;
  }
  const nextOffset = store.layout.offsetForKey(anchor.key);
  if (nextOffset === undefined) {
    setScrollerScrollTop(store, settledViewportTop(store));
    return null;
  }
  const target = store.headerExtent + nextOffset + store.drift - anchor.offset;
  if (anchor.intent === 'directional_start') {
    const index = store.keys.indexOf(anchor.key);
    applyAlignedPosition(store, index, anchor.key, store.headerExtent + nextOffset + store.drift, target, 'start', 'directional');
    return store.activeAnchor;
  }
  const delta = target - settledViewportTop(store);
  // Mid-scroll absorption (REQ-VT-004); requires leading-spacer room and no
  // correction already owed as a write — a correction lives in the spacer or
  // in a pending write, never both.
  if (delta !== 0 && scrollIsActive(store) && store.pendingScrollTop === null) {
    const driftNext = store.drift - delta;
    const firstOffset = store.range ? store.layout.itemAt(store.range.startIndex)?.offset ?? 0 : 0;
    if (firstOffset + driftNext >= 0) {
      store.drift = driftNext;
      return anchor;
    }
  }
  setScrollerScrollTop(store, target);
  return anchor;
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

function measureMountedExtents<T>(store: PhysicalStore<T>): boolean {
  let changed = false;
  if (store.headerElement && !isLayoutHidden(store.headerElement)) {
    const extent = measureElementExtent(store.headerElement);
    changed = store.headerExtent !== extent;
    store.headerExtent = extent;
  }
  for (const [key, row] of store.rowElements) {
    if (!isLayoutHidden(row)) changed = updateMeasuredExtent(store, key, measureElementExtent(row)) || changed;
  }
  return changed;
}

function applyPhysicalChange<T>(store: PhysicalStore<T>, anchor: PhysicalAnchor | null, wasPinned: boolean): void {
  store.layout = buildStoreLayout(store);
  const followTail = wasPinned && store.tailFollowAllowed;
  if (resolvePendingTarget(store)) {
    // The resolved selector supplies the new physical anchor.
    anchor = store.activeAnchor;
  } else if (store.scroller && (followTail || store.initialTailPending)) {
    store.initialTailPending = false;
    setScrollerScrollTop(store, totalPhysicalExtent(store));
    anchor = null;
  } else if (!store.initialTailPending) {
    anchor = applyAnchor(store, anchor);
  }
  store.activeAnchor = anchor;
  recompute(store);
}

function handleResizeEntries<T>({ store, publish }: StorePublisher<T>, entries: ResizeObserverEntry[]): void {
  if (isLayoutHidden(store.scroller)) {
    store.revealPublicationPending = true;
    return;
  }
  const revealed = store.revealPublicationPending;
  store.revealPublicationPending = false;
  let physicalChanged = false;
  let viewportChanged = false;
  const anchor = anchorForChange(store);
  const wasPinned = store.pinned;

  for (const entry of entries) {
    const target = entry.target;
    const entryHeight = clampNonNegative(entry.contentRect.height);
    if (target === store.scroller) {
      const nextExtent = entryHeight || (store.scroller ? viewportExtentForScroller(store.scroller) : 0);
      if (store.viewportExtent !== nextExtent) {
        store.viewportExtent = clampNonNegative(nextExtent);
        viewportChanged = true;
      }
      const nextTop = (revealed ? store.pendingScrollTop : null) ?? store.scroller?.scrollTop ?? store.viewportTop;
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

  if (revealed) physicalChanged = measureMountedExtents(store) || physicalChanged;
  if (physicalChanged) {
    const previousDrift = store.drift;
    applyPhysicalChange(store, anchor, wasPinned);
    if (store.drift !== previousDrift) flushSync(publish);
    else publish();
    return;
  }

  if (revealed) resolvePendingTarget(store);
  if (revealed && store.pendingScrollTop !== null) {
    setScrollerScrollTop(store, store.pendingScrollTop);
  }

  if (viewportChanged || revealed) {
    if (viewportChanged && wasPinned && (store.tailFollowAllowed || store.scroller?.classList.contains('reaction-dock-reserved'))) {
      setScrollerScrollTop(store, totalPhysicalExtent(store));
    }
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
  const resolvedKeys = resolvePhysicalKeys(props.items, props.getKey);
  const keys = resolvedKeys.keys;
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
    revealPublicationPending: false,
    activeAnchor: null,
    scroller: null,
    headerElement: null,
    rowElements: new Map(),
    resizeObserver: null,
    initialTailPending: props.initialTail ?? true,
    preservedViewport: null,
    positionOwner: { kind: 'unowned' },
    positioningAuthority: props.positioningAuthority ?? NO_POSITIONING_AUTHORITY,
    positioningViewKey: props.positioningViewKey ?? null,
    pendingTarget: null,
    pinned: true,
    revision: 0,
    drift: 0,
    lastScrollAtMs: Number.NEGATIVE_INFINITY,
    activeTouchIds: new Set(),
    lastGestureAtMs: Number.NEGATIVE_INFINITY,
    tailFollowAllowed: true,
    pendingScrollTop: null,
    pendingProgrammaticTop: null,
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
    scrollerId,
    ariaLabel,
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
  store.positioningAuthority = props.positioningAuthority ?? NO_POSITIONING_AUTHORITY;
  store.positioningViewKey = props.positioningViewKey ?? null;
  retireUnauthorizedPosition(store);
  const resolvedPhysicalKeys = useMemo(
    () => resolvePhysicalKeys(items, getKey),
    [getKey, items],
  );
  const duplicateKeySignature = JSON.stringify(resolvedPhysicalKeys.duplicateKeys);
  const lastReportedDuplicateKeySignature = useRef('[]');
  const [, publishRevision] = useReducer((revision: number) => revision + 1, 0);

  if (
    store.items !== items ||
    store.getKey !== getKey ||
    store.estimatedExtent !== estimatedExtent ||
    store.overscan !== clampNonNegative(overscan)
  ) {
    const anchor = topAnchorForChange(store);
    const wasPinned = store.pinned;
    store.items = items;
    store.getKey = getKey;
    store.keys = resolvedPhysicalKeys.keys;
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

  useEffect(() => {
    if (duplicateKeySignature === lastReportedDuplicateKeySignature.current) return;
    lastReportedDuplicateKeySignature.current = duplicateKeySignature;
    if (resolvedPhysicalKeys.duplicateKeys.length === 0) return;
    console.error('[VirtualTranscript] duplicate semantic keys quarantined', {
      duplicateKeys: resolvedPhysicalKeys.duplicateKeys,
    });
  }, [duplicateKeySignature, resolvedPhysicalKeys]);

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
      const anchor = anchorForChange(current);
      const wasPinned = current.pinned;
      const changed = !isLayoutHidden(element)
        && updateMeasuredExtent(current, key, measureElementExtent(element));
      observeElement(current, publish, element);
      if (changed) {
        applyPhysicalChange(current, anchor, wasPinned);
        publish();
      }
      if (resolvePendingTarget(current)) {
        recompute(current);
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
    const anchor = topAnchorForChange(current);
    const wasPinned = current.pinned;
    const nextExtent = element
      ? isLayoutHidden(element) ? current.headerExtent : measureElementExtent(element)
      : 0;
    const changed = current.headerExtent !== nextExtent;
    current.headerExtent = nextExtent;
    if (element) observeElement(current, publish, element);
    if (changed) {
      applyPhysicalChange(current, anchor, wasPinned);
      publish();
    }
  }, [publish]);

  const detachTouchListenersRef = useRef<(() => void) | null>(null);

  const scrollerCallback = useCallback((element: HTMLDivElement | null) => {
    const current = storeRef.current;
    if (!current) return;
    if (current.scroller && current.scroller !== element) {
      unobserveElement(current, current.scroller);
    }
    detachTouchListenersRef.current?.();
    detachTouchListenersRef.current = null;
    current.activeTouchIds.clear();

    current.scroller = element;
    if (element) {
      if (!isLayoutHidden(element)) {
        current.viewportTop = element.scrollTop;
        current.viewportExtent = viewportExtentForScroller(element);
      }
      observeElement(current, publish, element);
      if (current.initialTailPending && current.layout.count > 0) {
        current.initialTailPending = false;
        setScrollerScrollTop(current, totalPhysicalExtent(current));
      }
      // Anything the browser no longer lists as down is released, whether or
      // not its own end event ever reached a listener.
      const pruneToLive = (store: PhysicalStore<T>, event: TouchEvent) => {
        const live = new Set(Array.from(event.touches).map((touch) => touch.identifier));
        for (const id of Array.from(store.activeTouchIds)) {
          if (!live.has(id)) store.activeTouchIds.delete(id);
        }
      };
      const onTouchStart = (event: TouchEvent) => {
        const store = storeRef.current;
        if (!store) return;
        pruneToLive(store, event);
        for (const touch of Array.from(event.changedTouches)) {
          store.activeTouchIds.add(touch.identifier);
        }
        store.lastGestureAtMs = Date.now();
      };
      const onTouchMove = () => {
        const store = storeRef.current;
        if (!store) return;
        store.lastGestureAtMs = Date.now();
      };
      const onTouchStop = (event: TouchEvent) => {
        const store = storeRef.current;
        if (!store) return;
        for (const touch of Array.from(event.changedTouches)) {
          store.activeTouchIds.delete(touch.identifier);
        }
        pruneToLive(store, event);
        store.lastGestureAtMs = Date.now();
      };
      element.addEventListener('touchstart', onTouchStart, { passive: true });
      element.addEventListener('touchmove', onTouchMove, { passive: true });
      element.addEventListener('touchend', onTouchStop, { passive: true });
      element.addEventListener('touchcancel', onTouchStop, { passive: true });
      detachTouchListenersRef.current = () => {
        element.removeEventListener('touchstart', onTouchStart);
        element.removeEventListener('touchmove', onTouchMove);
        element.removeEventListener('touchend', onTouchStop);
        element.removeEventListener('touchcancel', onTouchStop);
      };
    }
    recompute(current);
    scrollerRef?.(element);
  }, [publish, scrollerRef]);


  useLayoutEffect(() => {
    const current = storeRef.current;
    if (!current) return;
    const nextKeys = resolvedPhysicalKeys.keys;
    const preserved = current.preservedViewport;
    const prefixInserted = preserved !== null
      && preserved.firstKey !== null
      && nextKeys.indexOf(preserved.firstKey) > 0;
    if (prefixInserted) {
      current.preservedViewport = null;
      current.viewportTop = preserved.top;
    }
    const anchor = prefixInserted ? null : topAnchorForChange(current);
    const wasPinned = !prefixInserted && current.pinned;
    current.items = items;
    current.getKey = getKey;
    current.keys = nextKeys;
    current.estimatedExtent = estimatedExtent;
    current.overscan = clampNonNegative(overscan);
    const presentKeys = new Set(current.keys);
    for (const key of current.measuredExtents.keys()) {
      if (!presentKeys.has(key)) current.measuredExtents.delete(key);
    }
    applyPhysicalChange(current, anchor, wasPinned);
    publish();
  }, [estimatedExtent, getKey, items, overscan, publish, resolvedPhysicalKeys]);

  useLayoutEffect(() => {
    const current = storeRef.current;
    return () => {
      if (reconcileTimerRef.current !== 0) {
        clearTimeout(reconcileTimerRef.current);
        reconcileTimerRef.current = 0;
      }
      detachTouchListenersRef.current?.();
      detachTouchListenersRef.current = null;
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

  const totalExtent = totalPhysicalExtent(store);
  const pinned = store.pinned;

  useLayoutEffect(() => {
    onTotalExtentChange?.(totalExtent);
  }, [onTotalExtentChange, totalExtent]);

  useLayoutEffect(() => {
    onPinnedChange?.(pinned);
  }, [onPinnedChange, pinned]);

  useImperativeHandle(ref, () => ({
    scrollToIndex(index, align, viewportStartOffset = 0, targetSelector, startPolicy = 'directional') {
      const current = storeRef.current;
      if (!current) return;
      positionAtIndex(current, { kind: 'unowned' }, index, align, viewportStartOffset, targetSelector, startPolicy);
      publish();
    },
    scrollToOwnedIndex(commandKey, index, align, viewportStartOffset = 0, targetSelector, startPolicy = 'directional') {
      const current = storeRef.current;
      if (!current) return;
      positionAtIndex(current, { kind: 'command', commandKey }, index, align, viewportStartOffset, targetSelector, startPolicy);
      publish();
    },
    completePosition(commandKey) {
      const current = storeRef.current;
      if (!current || current.positionOwner.kind !== 'command' || current.positionOwner.commandKey !== commandKey
        || !positionIsAuthorized(current, current.positionOwner)) return;
      current.pendingTarget = null;
      current.positionOwner = { kind: 'settled', commandKey, viewKey: current.positioningViewKey };
    },
    interruptPosition() {
      const current = storeRef.current;
      if (!current) return;
      retirePosition(current);
      current.viewportTop = current.scroller?.scrollTop ?? current.viewportTop;
      current.activeAnchor = captureTopAnchor(current);
      recompute(current);
    },
    cancelPosition(commandKey) {
      const current = storeRef.current;
      if (!current || current.positionOwner.kind === 'unowned' || current.positionOwner.commandKey !== commandKey) return;
      retirePosition(current);
      current.viewportTop = current.scroller?.scrollTop ?? current.viewportTop;
      current.activeAnchor = captureTopAnchor(current);
      recompute(current);
      publish();
    },
    scrollToTail() {
      const current = storeRef.current;
      if (!current) return;
      current.positionOwner = { kind: 'unowned' };
      current.pendingTarget = null;
      current.activeAnchor = null;
      setScrollerScrollTop(current, totalPhysicalExtent(current));
      recompute(current);
      publish();
    },
    scrollBy(delta) {
      const current = storeRef.current;
      if (!current?.scroller || !Number.isFinite(delta) || delta === 0) return;
      current.positionOwner = { kind: 'unowned' };
      current.pendingTarget = null;
      current.viewportExtent = viewportExtentForScroller(current.scroller);
      setScrollerScrollTop(current, current.scroller.scrollTop + delta);
      current.activeAnchor = captureTopAnchor(current);
      recompute(current);
      publish();
    },
    setTailFollowAllowed(allowed) {
      const current = storeRef.current;
      if (current) current.tailFollowAllowed = allowed;
    },
    isProgrammaticScroll(scrollTop) {
      const current = storeRef.current;
      return current ? isProgrammaticScrollTop(current, scrollTop) : false;
    },
    captureVisibleAnchor() {
      const current = storeRef.current;
      if (!current) return null;
      current.viewportTop = current.scroller?.scrollTop ?? current.viewportTop;
      const anchor = captureTopAnchor(current);
      current.activeAnchor = anchor;
      recompute(current);
      publish();
      return anchor ? { index: anchor.index, key: anchor.key, offset: anchor.offset } : null;
    },
    preserveViewportOnNextItemsChange() {
      const current = storeRef.current;
      if (!current) return;
      current.preservedViewport = {
        top: current.scroller?.scrollTop ?? current.viewportTop,
        firstKey: current.keys[0] ?? null,
      };
      current.activeAnchor = null;
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
    physicalSnapshot(targetIndex, targetSelector) {
      const current = storeRef.current;
      return current
        ? synchronizedPhysicalSnapshot(current, targetIndex, targetSelector)
        : {
            renderedRange: null,
            visibleRange: null,
            viewportTop: 0,
            layoutRevision: 0,
            ...(targetIndex === undefined ? {} : { targetIndex, targetOffset: null, targetMeasured: false }),
          };
    },
  }), [publish]);

  const handleScroll = useCallback(() => {
    const current = storeRef.current;
    if (!current?.scroller) return;
    // Own-write echo (REQ-VT-004): geometry only, no scroll-active marking.
    if (isProgrammaticScrollTop(current, current.scroller.scrollTop)) {
      current.viewportTop = current.scroller.scrollTop;
      if (current.preservedViewport) current.preservedViewport.top = current.viewportTop;
      recompute(current);
      publish();
      return;
    }
    retirePosition(current);
    current.lastScrollAtMs = Date.now();
    current.viewportTop = current.scroller.scrollTop;
    if (current.preservedViewport) current.preservedViewport.top = current.viewportTop;
    current.viewportExtent = viewportExtentForScroller(current.scroller);
    current.activeAnchor = captureTopAnchor(current);
    recompute(current);
    publish();
  }, [publish]);

  const reconcileTimerRef = useRef(0);

  // Drift reconciliation (REQ-VT-004). The owed position is written in a
  // layout effect: spacer change and scrollTop write must share one paint.
  useLayoutEffect(() => {
    const current = storeRef.current;
    if (!current) return;
    if (current.pendingScrollTop !== null && !isLayoutHidden(current.scroller)) {
      if (current.revealPublicationPending) synchronizedPhysicalSnapshot(current);
      else if (!resolvePendingTarget(current)) setScrollerScrollTop(current, current.pendingScrollTop);
      recompute(current);
      publish();
      return;
    }
    if (current.drift !== 0) {
      // Spacer exhausted: immediate direct-write fallback (REQ-VT-004).
      const firstOffset = current.range
        ? current.layout.itemAt(current.range.startIndex)?.offset ?? 0
        : 0;
      if (firstOffset + current.drift < 0) {
        const drift = current.drift;
        current.drift = 0;
        current.pendingScrollTop = current.viewportTop - drift;
        recompute(current);
        publish();
        return;
      }
    }
    if (current.drift === 0 || reconcileTimerRef.current !== 0) return;
    const arm = () => {
      reconcileTimerRef.current = window.setTimeout(() => {
        reconcileTimerRef.current = 0;
        const store = storeRef.current;
        if (!store || store.drift === 0) return;
        if (scrollIsActive(store)) {
          arm();
          return;
        }
        const drift = store.drift;
        store.drift = 0;
        store.pendingScrollTop = store.viewportTop - drift;
        recompute(store);
        publish();
      }, SCROLL_SETTLE_MS);
    };
    arm();
  });

  const range = store.range;
  const visibleItems = range
    ? items.slice(range.startIndex, range.endIndex + 1)
    : [];
  const topSpacer = range
    ? Math.max(0, (store.layout.itemAt(range.startIndex)?.offset ?? 0) + store.drift)
    : 0;
  const rangePhysicalEnd = range ? itemPhysicalEnd(store, range.endIndex) ?? store.headerExtent : store.headerExtent;
  const bottomSpacer = range
    ? Math.max(0, totalPhysicalExtent(store) - rangePhysicalEnd)
    : Math.max(0, totalPhysicalExtent(store) - store.headerExtent);
  const rootClassName = className ? `virtual-transcript ${className}` : 'virtual-transcript';

  // tabIndex: keys scroll the nearest scrollable ancestor of the focused
  // element, so without a tab stop here the transcript is unreachable from the
  // keyboard (REQ-VT-013).
  return (
    <div
      ref={scrollerCallback}
      id={scrollerId}
      className={rootClassName}
      style={{ overflowAnchor: 'none' }}
      role="region"
      aria-label={ariaLabel}
      tabIndex={0}
      onScroll={handleScroll}
    >
      <div className="virtual-transcript__inner" style={{ height: totalExtent }}>
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
            const key = store.keys[index]!;
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
