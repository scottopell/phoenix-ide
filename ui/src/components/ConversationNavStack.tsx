import { memo, useCallback, useRef, useState } from 'react';
import type { ListRange } from 'react-virtuoso';
import { MessageList, type MessageListHandle } from './MessageList';
import { ConversationNav } from './ConversationNav';
import { resolveActiveUnitIndex } from './conversationNavSpy';
import type { Chapter } from '../conversation/conversationChapters';

/** Everything MessageList accepts, minus the wiring this stack owns
 *  (`onVisibleRangeChange`, `onChaptersChange`, and the imperative ref). */
type StackProps = Omit<
  React.ComponentProps<typeof MessageList>,
  'onVisibleRangeChange' | 'onChaptersChange' | 'ref'
>;

/**
 * Coordinates the conversation nav strip with the virtualized message list.
 * Owns the imperative `MessageListHandle` ref (so a pill click can jump to an
 * off-screen, unmounted row via `scrollToIndex`), the chapter list (built
 * inside MessageList from the canonical `historicalUnits`), and the scroll-spy
 * active index. Renders the nav as a fixed-height strip above the list.
 */
export const ConversationNavStack = memo(function ConversationNavStack(props: StackProps) {
  const { onLoadOlderMessages } = props;
  const listRef = useRef<MessageListHandle>(null);
  const [preservedHistoryAnchorId, setPreservedHistoryAnchorId] = useState<string | null>(null);
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [activeUnitIndex, setActiveUnitIndex] = useState<number | null>(null);

  // Keep the latest chapters for the range callback without re-binding it.
  const chaptersRef = useRef<Chapter[]>(chapters);
  chaptersRef.current = chapters;

  const handleChaptersChange = useCallback((next: Chapter[]) => {
    setChapters(next);
  }, []);

  const handleVisibleRangeChange = useCallback((range: ListRange) => {
    const next = resolveActiveUnitIndex(chaptersRef.current, range);
    setActiveUnitIndex((prev) => (prev === next ? prev : next));
  }, []);

  const handleJump = useCallback((unitIndex: number) => {
    listRef.current?.scrollToUnitIndex(unitIndex);
  }, []);

  const handleLoadOlderMessages = useCallback(() => {
    setPreservedHistoryAnchorId(listRef.current?.getFirstVisibleMessageId() ?? null);
    onLoadOlderMessages?.();
  }, [onLoadOlderMessages]);

  return (
    <>
      <ConversationNav
        chapters={chapters}
        activeUnitIndex={activeUnitIndex}
        onJump={handleJump}
      />
      <MessageList
        ref={listRef}
        {...props}
        onLoadOlderMessages={onLoadOlderMessages ? handleLoadOlderMessages : undefined}
        preservedHistoryAnchorId={preservedHistoryAnchorId}
        onChaptersChange={handleChaptersChange}
        onVisibleRangeChange={handleVisibleRangeChange}
      />
    </>
  );
});
