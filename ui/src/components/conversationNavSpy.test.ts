// Unit tests for resolveActiveUnitIndex — the scroll-spy rule that maps
// virtuoso's visible item range to the active chapter's unitIndex.

import { describe, it, expect } from 'vitest';
import { resolveActiveUnitIndex } from './conversationNavSpy';
import type { Chapter } from '../conversation/conversationChapters';

function chapter(unitIndex: number): Chapter {
  return { unitIndex, kind: 'prompt', label: `c${unitIndex}`, sequenceId: unitIndex };
}

describe('resolveActiveUnitIndex', () => {
  const chapters = [chapter(0), chapter(3), chapter(7), chapter(12)];

  it('returns null when there are no chapters', () => {
    expect(resolveActiveUnitIndex([], { startIndex: 0, endIndex: 5 })).toBeNull();
  });

  it('picks the deepest chapter at or above the top of the viewport', () => {
    // Top of viewport is item 8: chapters 0,3,7 are at/above it; 7 is deepest.
    expect(resolveActiveUnitIndex(chapters, { startIndex: 8, endIndex: 11 })).toBe(7);
  });

  it('treats a chapter exactly at the top as active', () => {
    expect(resolveActiveUnitIndex(chapters, { startIndex: 3, endIndex: 6 })).toBe(3);
  });

  it('falls back to the first visible chapter when scrolled above all of them', () => {
    // start=0 sits above chapter 3; only chapter 0 is at/above the top.
    expect(resolveActiveUnitIndex(chapters, { startIndex: 0, endIndex: 2 })).toBe(0);
  });

  it('falls back to the first partially-visible chapter when none is above the top', () => {
    const later = [chapter(5), chapter(9)];
    // Viewport spans items 2..6: no chapter is <= start(2), but chapter 5 is
    // <= end(6) and thus partially visible.
    expect(resolveActiveUnitIndex(later, { startIndex: 2, endIndex: 6 })).toBe(5);
  });

  it('returns null when every chapter is below the viewport', () => {
    const later = [chapter(20), chapter(25)];
    expect(resolveActiveUnitIndex(later, { startIndex: 2, endIndex: 6 })).toBeNull();
  });

  it('activates the last chapter once scrolled to the bottom', () => {
    expect(resolveActiveUnitIndex(chapters, { startIndex: 13, endIndex: 18 })).toBe(12);
  });
});
