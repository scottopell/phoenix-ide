import { act, render } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import type { Chapter } from '../conversation/conversationChapters';
import { ConversationNavStack } from './ConversationNavStack';

const publication = vi.hoisted(() => ({ publish: null as ((chapters: Chapter[]) => void) | null }));
vi.mock('./MessageList', async () => {
  const { forwardRef } = await import('react');
  return { MessageList: forwardRef<HTMLDivElement, { onChaptersChange: (chapters: Chapter[]) => void }>(function MockList({ onChaptersChange }, ref) {
    publication.publish = onChaptersChange;
    return <div ref={ref} />;
  }) };
});
vi.mock('../hooks/useFloatingNavStack', () => ({ useFloatingNavStack: () => {} }));

describe('rendered chapter readiness', () => {
  it('publishes the deduplicated rendered union only after chapter authority arrives, including boundary-only and callback changes', () => {
    const first = vi.fn();
    const replacement = vi.fn();
    const boundary = { message_id: 'boundary', predecessor_transcript_row_id: 'a', successor_transcript_row_id: 'b', represented_successor_opening_message_id: null, segment_ordinal: 0 };
    const props = { messages: [], pendingMessages: [], streamingRequestId: null, convState: { type: 'idle' as const }, onRetry: vi.fn(), onOpenFile: undefined, transcriptPositioning: { kind: 'idle' as const, view: { conversationId: 'a', generation: 1, transcriptGeneration: 1 } } };
    const { rerender } = render(<MemoryRouter><ConversationNavStack {...props} historyBoundaries={[boundary]} onViewChaptersReady={first} /></MemoryRouter>);
    expect(first).not.toHaveBeenCalled();
    act(() => publication.publish!([]));
    expect(first).toHaveBeenLastCalledWith(1);
    act(() => publication.publish!([{ kind: 'boundary', messageId: 'boundary', transcriptId: 'a', unitIndex: 0, label: 'Loaded', sequenceId: 1 }]));
    expect(first).toHaveBeenLastCalledWith(1);
    rerender(<MemoryRouter><ConversationNavStack {...props} historyBoundaries={[boundary, { ...boundary, message_id: 'another', segment_ordinal: 1 }]} onViewChaptersReady={replacement} /></MemoryRouter>);
    expect(replacement).toHaveBeenLastCalledWith(2);
    const sameCountCallback = vi.fn();
    rerender(<MemoryRouter><ConversationNavStack {...props} historyBoundaries={[boundary, { ...boundary, message_id: 'another', segment_ordinal: 1 }]} onViewChaptersReady={sameCountCallback} /></MemoryRouter>);
    expect(sameCountCallback).toHaveBeenLastCalledWith(2);
    rerender(<MemoryRouter><ConversationNavStack {...props} historyBoundaries={[]} onViewChaptersReady={replacement} /></MemoryRouter>);
    expect(replacement).toHaveBeenLastCalledWith(1);
    act(() => publication.publish!([]));
    expect(replacement).toHaveBeenLastCalledWith(0);
  });
});
