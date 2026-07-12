import { describe, expect, it } from 'vitest';
import {
  buildTranscriptLayout,
  type TranscriptLayout,
} from './virtualTranscriptLayout';

function layout(
  measuredExtents?: ReadonlyMap<string, number>,
  estimatedExtent: number | ((key: string, index: number) => number) = 10,
): TranscriptLayout {
  return buildTranscriptLayout({
    keys: ['a', 'b', 'c', 'd'],
    estimatedExtent,
    ...(measuredExtents ? { measuredExtents } : {}),
  });
}

describe('buildTranscriptLayout', () => {
  it('builds ordered items with offsets and total extent from estimates', () => {
    const result = layout();

    expect(result.keys).toEqual(['a', 'b', 'c', 'd']);
    expect(result.count).toBe(4);
    expect(result.totalExtent).toBe(40);
    expect(result.items).toEqual([
      { key: 'a', extent: 10, measured: false, offset: 0, end: 10 },
      { key: 'b', extent: 10, measured: false, offset: 10, end: 20 },
      { key: 'c', extent: 10, measured: false, offset: 20, end: 30 },
      { key: 'd', extent: 10, measured: false, offset: 30, end: 40 },
    ]);
  });

  it('prefers measured extents per key and keeps estimates available', () => {
    const result = layout(
      new Map([
        ['b', 25],
        ['d', 5],
      ]),
    );

    expect(result.extentForKey('a')).toBe(10);
    expect(result.extentForKey('b')).toBe(25);
    expect(result.extentForKey('c')).toBe(10);
    expect(result.extentForKey('d')).toBe(5);
    expect(result.measuredExtentForKey('a')).toBeUndefined();
    expect(result.measuredExtentForKey('b')).toBe(25);
    expect(result.estimatedExtentForKey('b')).toBe(10);
    expect(result.offsetForKey('c')).toBe(35);
    expect(result.endForKey('d')).toBe(50);
    expect(result.totalExtent).toBe(50);
  });

  it('supports functional estimated extents', () => {
    const result = layout(undefined, (_key, index) => (index + 1) * 3);

    expect(result.items.map((item) => item.extent)).toEqual([3, 6, 9, 12]);
    expect(result.offsetForKey('c')).toBe(9);
    expect(result.totalExtent).toBe(30);
  });

  it('clamps invalid extents to zero', () => {
    const result = buildTranscriptLayout({
      keys: ['a', 'b', 'c'],
      estimatedExtent: (_key, index) => (index === 1 ? Number.NaN : -5),
      measuredExtents: new Map([['c', Number.POSITIVE_INFINITY]]),
    });

    expect(result.items).toEqual([
      { key: 'a', extent: 0, measured: false, offset: 0, end: 0 },
      { key: 'b', extent: 0, measured: false, offset: 0, end: 0 },
      { key: 'c', extent: 0, measured: true, offset: 0, end: 0 },
    ]);
    expect(result.totalExtent).toBe(0);
  });

  it('looks up indexes by offset across boundaries and outside the range', () => {
    const result = layout(
      new Map([
        ['a', 5],
        ['b', 15],
        ['c', 20],
        ['d', 10],
      ]),
    );

    expect(result.indexAtOffset(-100)).toBe(0);
    expect(result.indexAtOffset(0)).toBe(0);
    expect(result.indexAtOffset(4.999)).toBe(0);
    expect(result.indexAtOffset(5)).toBe(0);
    expect(result.indexAtOffset(5.001)).toBe(1);
    expect(result.indexAtOffset(19.999)).toBe(1);
    expect(result.indexAtOffset(20)).toBe(1);
    expect(result.indexAtOffset(20.001)).toBe(2);
    expect(result.indexAtOffset(1000)).toBe(3);
  });

  it('returns contiguous overscanned ranges for a viewport in the middle', () => {
    const result = layout(
      new Map([
        ['a', 10],
        ['b', 20],
        ['c', 30],
        ['d', 40],
      ]),
    );

    expect(
      result.rangeForViewport({
        viewportOffset: 25,
        viewportExtent: 20,
        overscanExtent: 10,
      }),
    ).toEqual({ startIndex: 1, endIndex: 2 });
  });

  it('expands overscan to surrounding items and clamps at transcript edges', () => {
    const result = layout(
      new Map([
        ['a', 10],
        ['b', 20],
        ['c', 30],
        ['d', 40],
      ]),
    );

    expect(
      result.rangeForViewport({
        viewportOffset: -50,
        viewportExtent: 5,
        overscanExtent: 30,
      }),
    ).toEqual({ startIndex: 0, endIndex: 1 });

    expect(
      result.rangeForViewport({
        viewportOffset: 95,
        viewportExtent: 100,
        overscanExtent: 0,
      }),
    ).toEqual({ startIndex: 3, endIndex: 3 });
  });

  it('returns null ranges for an empty layout', () => {
    const result = buildTranscriptLayout({
      keys: [],
      estimatedExtent: 10,
    });

    expect(result.count).toBe(0);
    expect(result.totalExtent).toBe(0);
    expect(result.indexAtOffset(0)).toBe(0);
    expect(result.rangeForViewport({
      viewportOffset: 0,
      viewportExtent: 100,
      overscanExtent: 20,
    })).toBeNull();
  });

  it('rebuilds immutably when measurements change', () => {
    const first = layout();
    const second = layout(new Map([['b', 50]]));

    expect(first.extentForKey('b')).toBe(10);
    expect(first.offsetForKey('c')).toBe(20);
    expect(second.extentForKey('b')).toBe(50);
    expect(second.offsetForKey('c')).toBe(60);
  });
});
