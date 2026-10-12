import { useMemo } from 'react';
import { PillStrip } from './PillStrip';
import type { PillItem } from './PillStrip';
import type { Chapter } from '../conversation/conversationChapters';
import type { ConversationHistoryBoundaryView } from '../generated/ConversationHistoryBoundaryView';
import './ConversationNav.css';
import { inputOriginPresentation } from '../conversation/inputOriginPresentation';

const PROSE_TITLE = 'Assistant';

interface ConversationNavProps {
  /** Whole-conversation chapters (prompts + significant prose), in render
   *  order. Derived by `buildConversationChapters` from the same
   *  `historicalUnits` array MessageList feeds to VirtualTranscript. */
  chapters: Chapter[];
  historyBoundaries?: ConversationHistoryBoundaryView[] | undefined;
  onJumpBoundary?: ((messageId: string) => void) | undefined;
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
export function ConversationNav({ chapters, activeUnitIndex, onJump, historyBoundaries, onJumpBoundary }: ConversationNavProps) {
  // Memoize on the chapter list + active index so the mapped `items` array is
  // referentially stable across the parent's unrelated re-renders (streaming
  // token churn, heartbeat) — this avoids rebuilding the pill list and
  // re-running PillStrip's per-`items` effects when neither the chapters nor
  // the active index changed.
  const items: PillItem[] = useMemo(
    () => [
      ...(historyBoundaries ?? []).filter((boundary) => !chapters.some((chapter) => chapter.kind === 'boundary' && chapter.messageId === boundary.message_id)).map((boundary): PillItem => ({
        key: boundary.message_id,
        label: `Context ${boundary.segment_ordinal + 1} · load`,
        className: 'boundary',
        ariaLabel: `Load context boundary ${boundary.segment_ordinal + 1}: ${boundary.predecessor_transcript_row_id}`,
        onClick: () => onJumpBoundary?.(boundary.message_id),
      })),
      ...chapters.map((chapter) => {
        const isActive = chapter.unitIndex === activeUnitIndex;
        const origin = chapter.kind === 'prompt' ? inputOriginPresentation(chapter.origin) : null;
        return {
          key: chapter.messageId ?? `chapter-${chapter.unitIndex}`,
          label: origin && origin.className === 'meta' ? `${origin.label} · ${chapter.label}` : chapter.label,
          active: isActive,
          className: chapter.kind === 'boundary' ? 'boundary' : chapter.kind === 'prose' ? 'assistant' : origin?.className,
          ariaLabel: chapter.kind === 'boundary' ? `Context boundary: ${chapter.transcriptId}` : `${origin?.title ?? PROSE_TITLE}: ${chapter.label}`,
          onClick: () => chapter.kind === 'boundary' && chapter.messageId && onJumpBoundary
            ? onJumpBoundary(chapter.messageId) : onJump(chapter.unitIndex),
        };
      }),
    ],
    [chapters, activeUnitIndex, onJump, historyBoundaries, onJumpBoundary],
  );

  if (items.length === 0) {
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
