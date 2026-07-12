import { useMemo } from 'react';
import { PillStrip } from './PillStrip';
import type { PillItem } from './PillStrip';
import type { Chapter } from '../conversation/conversationChapters';

const CHAPTER_TITLES: Record<Chapter['kind'], string> = {
  prompt: 'Your message',
  prose: 'Assistant',
};

interface ConversationNavProps {
  /** Whole-conversation chapters (prompts + significant prose), in render
   *  order. Derived by `buildConversationChapters` from the same
   *  `historicalUnits` array MessageList feeds to VirtualTranscript. */
  chapters: Chapter[];
  /** `unitIndex` of the chapter currently in view (scroll-spy), or null when
   *  none is resolved. Applies the active pill styling. */
  activeUnitIndex: number | null;
  /** Jump the message list to the chapter's render unit. Wired to
   *  `MessageListHandle.scrollToUnitIndex`. */
  onJump: (unitIndex: number) => void;
}

/**
 * Persistent whole-conversation chapter strip. The permanent occupant of the
 * top horizontal slot — same role on cold load and while streaming. Each
 * chapter is a type-styled pill (user prompt vs assistant prose); clicking
 * jumps the virtualized message list to that unit, and the scroll-spy
 * highlights the chapter currently in view.
 */
export function ConversationNav({ chapters, activeUnitIndex, onJump }: ConversationNavProps) {
  // Memoize on the chapter list + active index so the mapped `items` array is
  // referentially stable across the parent's unrelated re-renders (streaming
  // token churn, heartbeat) — this avoids rebuilding the pill list and
  // re-running PillStrip's per-`items` effects when neither the chapters nor
  // the active index changed.
  const items: PillItem[] = useMemo(
    () =>
      chapters.map((chapter) => {
        const isActive = chapter.unitIndex === activeUnitIndex;
        return {
          key: `chapter-${chapter.unitIndex}`,
          label: chapter.label,
          active: isActive,
          className: chapter.kind === 'prose' ? 'assistant' : 'user',
          ariaLabel: `${CHAPTER_TITLES[chapter.kind]}: ${chapter.label}`,
          onClick: () => onJump(chapter.unitIndex),
        };
      }),
    [chapters, activeUnitIndex, onJump],
  );

  if (chapters.length === 0) {
    return null;
  }

  return (
    <PillStrip
      items={items}
      navId="conversation-nav"
      trailId="conversation-nav-trail"
      pillClassName="conversation-nav-item"
      arrowClassName="conversation-nav-arrow"
    />
  );
}
