import { useLocation, useNavigate } from 'react-router-dom';
import { memo, useCallback, useRef, useState } from 'react';
import type { VirtualTranscriptRange } from './VirtualTranscript';
import { MessageList, type MessageListHandle } from './MessageList';
import { ConversationNav } from './ConversationNav';
import { resolveActiveUnitIndex } from './conversationNavSpy';
import type { Chapter } from '../conversation/conversationChapters';
import type { RestoreBasis } from '../conversation/historyExpansion';
import type { ConversationHistoryBoundaryView } from '../generated/ConversationHistoryBoundaryView';

/** Everything MessageList accepts, minus the wiring this stack owns
 *  (`onVisibleRangeChange`, `onChaptersChange`, and the imperative ref). */
type StackProps = Omit<
  React.ComponentProps<typeof MessageList>,
  'onVisibleRangeChange' | 'onChaptersChange' | 'ref'
> & { historyBoundaries?: ConversationHistoryBoundaryView[] };

/**
 * Coordinates the conversation nav strip with the virtualized message list.
 * Owns the imperative `MessageListHandle` ref (so a pill click can jump to an
 * off-screen, unmounted row via `scrollToIndex`), the chapter list (built
 * inside MessageList from the canonical `historicalUnits`), and the scroll-spy
 * active index. Renders the nav as a fixed-height strip above the list.
 */
export const ConversationNavStack = memo(function ConversationNavStack(props: StackProps) {
  const { onLoadOlderMessages, onUpdateOlderMessagesRestore, historyBoundaries, ...listProps } = props;
  const location = useLocation();
  const navigate = useNavigate();
  const sourceToolId = new URLSearchParams(location.search).get('source_tool');
  let sourceCallTarget: { messageId: string; toolUseId: string } | null = null;
  if (sourceToolId && location.hash.startsWith('#message-')) {
    try { sourceCallTarget = { messageId: decodeURIComponent(location.hash.slice(9)), toolUseId: sourceToolId }; } catch { /* Invalid external locator is not a navigation target. */ }
  }
  const listRef = useRef<MessageListHandle>(null);
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [activeUnitIndex, setActiveUnitIndex] = useState<number | null>(null);

  // Keep the latest chapters for the range callback without re-binding it.
  const chaptersRef = useRef<Chapter[]>(chapters);
  chaptersRef.current = chapters;

  const handleChaptersChange = useCallback((next: Chapter[]) => {
    setChapters(next);
  }, []);

  const handleVisibleRangeChange = useCallback((range: VirtualTranscriptRange) => {
    const next = resolveActiveUnitIndex(chaptersRef.current, range);
    setActiveUnitIndex((prev) => (prev === next ? prev : next));
  }, []);

  const handleJump = useCallback((unitIndex: number) => {
    onUpdateOlderMessagesRestore?.({ kind: 'reader_viewport' });
    listRef.current?.scrollToUnitIndex(unitIndex);
  }, [onUpdateOlderMessagesRestore]);

  const handleLoadOlderMessages = useCallback(async (restoreBasis?: RestoreBasis) => {
    if (!onLoadOlderMessages) return;
    const list = listRef.current;
    return onLoadOlderMessages(
      restoreBasis ?? (list ? list.captureHistoryRestoreBasis() : undefined),
    );
  }, [onLoadOlderMessages]);

  return (
    <>
      <ConversationNav
        chapters={chapters}
        activeUnitIndex={activeUnitIndex}
        onJump={handleJump}
        historyBoundaries={historyBoundaries}
        onJumpBoundary={(messageId) => {
          const query = new URLSearchParams(location.search);
          query.delete('source_tool');
          const boundary = historyBoundaries?.find((candidate) => candidate.message_id === messageId);
          if (boundary && query.has('source_transcript')) query.set('source_transcript', boundary.predecessor_transcript_row_id);
          navigate({ pathname: location.pathname, search: query.toString(), hash: `#message-${encodeURIComponent(messageId)}` });
        }}
      />
      <MessageList
        ref={listRef}
        {...listProps}
        sourceCallTarget={sourceCallTarget}
        onLoadOlderMessages={onLoadOlderMessages ? handleLoadOlderMessages : undefined}
        onUpdateOlderMessagesRestore={onUpdateOlderMessagesRestore}
        onChaptersChange={handleChaptersChange}
        onVisibleRangeChange={handleVisibleRangeChange}
      />
    </>
  );
});
