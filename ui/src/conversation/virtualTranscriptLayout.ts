export interface TranscriptLayoutItem {
  key: string;
  extent: number;
  measured: boolean;
  offset: number;
  end: number;
}

export interface TranscriptOverscanInput {
  viewportOffset: number;
  viewportExtent: number;
  overscanExtent: number;
}

export interface TranscriptRange {
  startIndex: number;
  endIndex: number;
}

export interface TranscriptLayout {
  readonly keys: readonly string[];
  readonly count: number;
  readonly totalExtent: number;
  readonly items: readonly TranscriptLayoutItem[];
  estimatedExtentForKey(key: string): number | undefined;
  measuredExtentForKey(key: string): number | undefined;
  extentForKey(key: string): number | undefined;
  itemAt(index: number): TranscriptLayoutItem | undefined;
  indexForKey(key: string): number | undefined;
  offsetForKey(key: string): number | undefined;
  endForKey(key: string): number | undefined;
  indexAtOffset(offset: number): number;
  rangeForViewport(input: TranscriptOverscanInput): TranscriptRange | null;
}

export interface BuildTranscriptLayoutInput {
  keys: readonly string[];
  estimatedExtent: number | ((key: string, index: number) => number);
  measuredExtents?: ReadonlyMap<string, number>;
}

function clampExtent(extent: number): number {
  return Number.isFinite(extent) && extent >= 0 ? extent : 0;
}

function resolveEstimatedExtent(
  estimatedExtent: BuildTranscriptLayoutInput['estimatedExtent'],
  key: string,
  index: number,
): number {
  return clampExtent(
    typeof estimatedExtent === 'function'
      ? estimatedExtent(key, index)
      : estimatedExtent,
  );
}

function firstEndAfter(prefixEnds: readonly number[], target: number): number {
  let low = 0;
  let high = prefixEnds.length - 1;

  while (low < high) {
    const mid = Math.floor((low + high) / 2);
    if (prefixEnds[mid]! > target) {
      high = mid;
    } else {
      low = mid + 1;
    }
  }

  return low;
}

function firstEndAtOrAfter(prefixEnds: readonly number[], target: number): number {
  let low = 0;
  let high = prefixEnds.length - 1;

  while (low < high) {
    const mid = Math.floor((low + high) / 2);
    if (prefixEnds[mid]! >= target) {
      high = mid;
    } else {
      low = mid + 1;
    }
  }

  return low;
}

export function buildTranscriptLayout(
  input: BuildTranscriptLayoutInput,
): TranscriptLayout {
  const measuredExtents = input.measuredExtents ?? new Map<string, number>();
  const keys = [...input.keys];
  const keyToIndex = new Map<string, number>();
  const estimatedByKey = new Map<string, number>();
  const measuredByKey = new Map<string, number>();
  const items: TranscriptLayoutItem[] = [];
  const prefixEnds: number[] = [];

  let offset = 0;

  keys.forEach((key, index) => {
    keyToIndex.set(key, index);
    const estimated = resolveEstimatedExtent(input.estimatedExtent, key, index);
    estimatedByKey.set(key, estimated);

    const measured = measuredExtents.get(key);
    const hasMeasured = measured !== undefined;
    const extent = hasMeasured ? clampExtent(measured) : estimated;

    if (hasMeasured) {
      measuredByKey.set(key, extent);
    }

    const end = offset + extent;
    items.push({
      key,
      extent,
      measured: hasMeasured,
      offset,
      end,
    });
    prefixEnds.push(end);
    offset = end;
  });

  const totalExtent = offset;

  return {
    keys,
    count: keys.length,
    totalExtent,
    items,
    estimatedExtentForKey(key) {
      return estimatedByKey.get(key);
    },
    measuredExtentForKey(key) {
      return measuredByKey.get(key);
    },
    extentForKey(key) {
      const index = keyToIndex.get(key);
      return index === undefined ? undefined : items[index]?.extent;
    },
    itemAt(index) {
      return items[index];
    },
    indexForKey(key) {
      return keyToIndex.get(key);
    },
    offsetForKey(key) {
      const index = keyToIndex.get(key);
      return index === undefined ? undefined : items[index]?.offset;
    },
    endForKey(key) {
      const index = keyToIndex.get(key);
      return index === undefined ? undefined : items[index]?.end;
    },
    indexAtOffset(rawOffset) {
      if (items.length === 0) return 0;
      if (rawOffset <= 0) return 0;
      if (rawOffset >= totalExtent) return items.length - 1;
      return firstEndAfter(prefixEnds, rawOffset);
    },
    rangeForViewport({ viewportOffset, viewportExtent, overscanExtent }) {
      if (items.length === 0) return null;

      const visibleStart = Math.max(0, viewportOffset);
      const visibleEnd = Math.max(visibleStart, viewportOffset + Math.max(0, viewportExtent));
      const startOffset = Math.max(0, visibleStart - Math.max(0, overscanExtent));
      const endOffset = Math.min(
        totalExtent,
        visibleEnd + Math.max(0, overscanExtent),
      );

      if (startOffset >= endOffset) return null;

      const startIndex = this.indexAtOffset(startOffset);
      const endIndex = firstEndAtOrAfter(prefixEnds, endOffset);

      return { startIndex, endIndex };
    },
  };
}
