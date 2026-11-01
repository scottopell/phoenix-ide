import { useEffect, useMemo } from 'react';
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
  onChaptersReady?: ((count: number) => void) | undefined;
  /** `unitIndex` of the chapter currently in view (scroll-spy), or null when
   *  none is resolved. Applies the active pill styling. */
  activeUnitIndex: number | null;
  onJump: (chapter: Chapter) => void;
}

/**
 * Persistent whole-conversation chapter strip. The permanent occupant of the
 * top horizontal slot — same role on cold load and while streaming. Each
 * chapter is a type-styled pill (user prompt vs assistant prose); clicking
 * jumps the virtualized message list to that unit, and the scroll-spy
 * highlights the chapter currently in view.
 */
export function ConversationNav({ chapters, activeUnitIndex, onJump, historyBoundaries, onJumpBoundary, onChaptersReady }: ConversationNavProps) {
  // Memoize on the chapter list + active index so the mapped `items` array is
  // referentially stable across the parent's unrelated re-renders (streaming
  // token churn, heartbeat) — this avoids rebuilding the pill list and
  // re-running PillStrip's per-`items` effects when neither the chapters nor
  // the active index changed.
  const orderByKey = useMemo(() => {
    const ordinals = new Map<string, number>();
    const order = new Map<string, number>();
    for (const boundary of historyBoundaries ?? []) {
      ordinals.set(boundary.predecessor_transcript_row_id, boundary.segment_ordinal);
      ordinals.set(boundary.successor_transcript_row_id, boundary.segment_ordinal + 1);
      order.set(boundary.message_id, boundary.segment_ordinal * 2 + 1);
    }
    for (const chapter of chapters) {
      if (chapter.kind === 'boundary' || chapter.target.kind === 'pending') continue;
      const ordinal = chapter.target.transcriptId ? ordinals.get(chapter.target.transcriptId) : undefined;
      if (ordinal !== undefined) order.set(chapter.target.messageId, ordinal * 2);
    }
    return order;
  }, [chapters, historyBoundaries]);
  const items: PillItem[] = useMemo(
    () => [
      ...(historyBoundaries ?? []).filter((boundary) => !chapters.some((chapter) => chapter.kind === 'boundary' && chapter.messageId === boundary.message_id)).map((boundary): PillItem => ({
        key: boundary.message_id,
        label: `Context ${boundary.segment_ordinal + 1} · load`,
        className: 'boundary',
        ariaLabel: `Load context boundary ${boundary.segment_ordinal + 1}`,
        onClick: () => onJumpBoundary?.(boundary.message_id),
      })),
      ...chapters.map((chapter) => {
        const isActive = chapter.unitIndex === activeUnitIndex;
        const origin = chapter.kind === 'prompt' ? inputOriginPresentation(chapter.origin) : null;
        return {
          key: chapter.kind === 'boundary' ? chapter.messageId : chapter.target.kind === 'persisted' ? chapter.target.messageId : `pending-${chapter.unitIndex}`,
          label: origin && origin.className === 'meta' ? `${origin.label} · ${chapter.label}` : chapter.label,
          active: isActive,
          className: chapter.kind === 'boundary' ? 'boundary' : chapter.kind === 'prose' ? 'assistant' : origin?.className,
          ariaLabel: chapter.kind === 'boundary' ? `Context boundary ${(historyBoundaries?.find(boundary => boundary.message_id === chapter.messageId)?.segment_ordinal ?? chapters.filter(candidate => candidate.kind === 'boundary' && candidate.unitIndex < chapter.unitIndex).length) + 1}` : `${origin?.title ?? PROSE_TITLE}: ${chapter.label}`,
          onClick: () => chapter.kind === 'boundary' && chapter.messageId && onJumpBoundary
            ? onJumpBoundary(chapter.messageId) : onJump(chapter),
        };
      }),
    ].sort((left, right) => (orderByKey.get(left.key) ?? Number.MAX_SAFE_INTEGER) - (orderByKey.get(right.key) ?? Number.MAX_SAFE_INTEGER)),
    [chapters, activeUnitIndex, onJump, historyBoundaries, onJumpBoundary, orderByKey],
  );

  useEffect(() => { onChaptersReady?.(items.length); }, [items.length, onChaptersReady]);

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
