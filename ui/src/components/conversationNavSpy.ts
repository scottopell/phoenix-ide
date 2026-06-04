// Scroll-spy rule for the conversation nav strip.
//
// Pure mapping from virtuoso's visible item range to the active chapter's
// `unitIndex`. Kept in its own module so the component file only exports the
// component (react-refresh) and the rule stays unit-testable in isolation.

import type { ListRange } from 'react-virtuoso';
import type { Chapter } from '../conversation/conversationChapters';

/**
 * Resolve the active chapter (scroll-spy) from virtuoso's visible item range.
 * Table-of-contents semantics: the active chapter is the deepest one whose
 * heading sits at or above the top of the viewport (`unitIndex <= startIndex`).
 * When the user is scrolled above the first chapter, fall back to the first
 * chapter that is at least partially visible (`unitIndex <= endIndex`).
 * Returns the chapter's `unitIndex`, or null when no chapter is in range.
 */
export function resolveActiveUnitIndex(
  chapters: Chapter[],
  range: ListRange,
): number | null {
  if (chapters.length === 0) return null;
  let atOrAboveTop: number | null = null;
  let firstVisible: number | null = null;
  for (const chapter of chapters) {
    if (chapter.unitIndex <= range.startIndex) {
      atOrAboveTop = chapter.unitIndex;
    }
    if (firstVisible === null && chapter.unitIndex <= range.endIndex) {
      firstVisible = chapter.unitIndex;
    }
  }
  return atOrAboveTop ?? firstVisible;
}
