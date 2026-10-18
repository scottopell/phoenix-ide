import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ConversationNav } from './ConversationNav';
import type { Chapter } from '../conversation/conversationChapters';
import { buildConversationChapters } from '../conversation/conversationChapters';
import type { HistoricalUnit } from '../conversation/renderUnits';

const chapters: Chapter[] = [
  { unitIndex: 0, kind: 'prompt', target: { kind: 'persisted', messageId: 'chapter-0', transcriptId: 'fixture' }, label: 'API prompt', sequenceId: 1, origin: { kind: 'user_api' } },
  { unitIndex: 1, kind: 'prose', target: { kind: 'persisted', messageId: 'chapter-1', transcriptId: 'fixture' }, label: 'Response', sequenceId: 2 },
  { unitIndex: 2, kind: 'prompt', target: { kind: 'persisted', messageId: 'chapter-2', transcriptId: 'fixture' }, label: 'Forwarded prompt', sequenceId: 3, origin: {
    kind: 'internal_conversation', source_call: null, product_conversation_id: 'source-product', transcript_id: 'source-row',
  } },
  { unitIndex: 3, kind: 'prompt', target: { kind: 'persisted', messageId: 'chapter-3', transcriptId: 'fixture' }, label: 'Event prompt', sequenceId: 4, origin: { kind: 'subscription_event', event_id: 'event-1' } },
  { unitIndex: 4, kind: 'prompt', target: { kind: 'persisted', messageId: 'chapter-4', transcriptId: 'fixture' }, label: 'Automatic prompt', sequenceId: 5, origin: { kind: 'system_generated' } },
  { unitIndex: 5, kind: 'prompt', target: { kind: 'persisted', messageId: 'chapter-5', transcriptId: 'fixture' }, label: 'Old prompt', sequenceId: 6 },
];

describe('ConversationNav input provenance', () => {
  it('keeps the latest real prompt navigable across event bursts and reload', () => {
    const units: HistoricalUnit[] = ['human', 'event-1', 'event-2'].map((id, index) => ({
      kind: 'user', key: id, message: {
        event_presentation: null,
        message_id: id, conversation_id: 'continued-transcript', sequence_id: index + 1,
        message_type: 'user', content: { text: index === 0 ? 'Conversation event quoted by a human' : 'Watch notification' },
        created_at: '', origin: index === 0 ? { kind: 'user_api' } : { kind: 'subscription_event', event_id: id },
      },
    }));
    const onJump = vi.fn();
    const { rerender } = render(<ConversationNav chapters={buildConversationChapters(units)} activeUnitIndex={0} onJump={onJump} />);
    expect(screen.getAllByRole('button')).toHaveLength(1);
    expect(screen.queryByText('Watch notification')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /User · API: Conversation event/ }));
    expect(onJump).toHaveBeenCalledWith(expect.objectContaining({ unitIndex: 0 }));
    rerender(<ConversationNav chapters={buildConversationChapters(JSON.parse(JSON.stringify(units)))} activeUnitIndex={0} onJump={onJump} />);
    expect(screen.getAllByRole('button')).toHaveLength(1);
    expect(units).toHaveLength(3);
  });

  it('distinguishes channels visually and accessibly without shifting the unit jump target', () => {
    const onJump = vi.fn();
    render(<ConversationNav chapters={chapters} activeUnitIndex={2} onJump={onJump} />);
    const api = screen.getByRole('button', { name: 'User · API: API prompt' });
    const forwarded = screen.getByRole('button', { name: 'From conversation ID source-product · transcript ID source-row: Forwarded prompt' });
    expect(api).toHaveClass('user');
    expect(forwarded).toHaveClass('meta', 'active');
    expect(forwarded).toHaveTextContent('From conversation · Forwarded prompt');
    expect(screen.getByRole('button', { name: 'Conversation event: Event prompt' })).toHaveClass('meta');
    expect(screen.getByRole('button', { name: 'System input: Automatic prompt' })).toHaveClass('meta');
    expect(screen.getByRole('button', { name: 'Unknown input: Old prompt' })).toHaveClass('meta');
    expect(screen.getByRole('button', { name: 'Assistant: Response' })).toHaveClass('assistant');
    fireEvent.keyDown(forwarded, { key: 'Enter' });
    expect(onJump).toHaveBeenCalledWith(expect.objectContaining({ unitIndex: 2 }));
  });
});

describe('aggregate boundary navigation', () => {
  it('keeps an unloaded middle boundary after acquired prefix chapters and before the loaded tail', () => {
    const boundaries = [
      { represented_successor_opening_message_id: null, message_id: 'boundary-a', predecessor_transcript_row_id: 'a', successor_transcript_row_id: 'b', segment_ordinal: 0 },
      { represented_successor_opening_message_id: null, message_id: 'boundary-b', predecessor_transcript_row_id: 'b', successor_transcript_row_id: 'c', segment_ordinal: 1 },
    ];
    const chapters: Chapter[] = [
      { kind: 'prompt', label: 'Prefix A', unitIndex: 0, sequenceId: 1, target: { kind: 'persisted', messageId: 'prompt-a', transcriptId: 'a' } },
      { kind: 'boundary', label: 'Boundary A', unitIndex: 1, sequenceId: 2, messageId: 'boundary-a', transcriptId: 'a' },
      { kind: 'prompt', label: 'Prefix B', unitIndex: 2, sequenceId: 1, target: { kind: 'persisted', messageId: 'prompt-b', transcriptId: 'b' } },
      { kind: 'prompt', label: 'Tail C', unitIndex: 3, sequenceId: 20, target: { kind: 'persisted', messageId: 'prompt-c', transcriptId: 'c' } },
    ];
    render(<ConversationNav chapters={chapters} historyBoundaries={boundaries} activeUnitIndex={null} onJump={vi.fn()} />);
    expect(screen.getAllByRole('button').map(button => button.textContent)).toEqual(['Unknown input · Prefix A', 'Boundary A', 'Unknown input · Prefix B', 'Context 2 · load', 'Unknown input · Tail C']);
  });

  it('uses one exact boundary identity before and after its chronological unit loads', () => {
    const boundary = { represented_successor_opening_message_id: null, message_id: 'exact-continuation', predecessor_transcript_row_id: 'predecessor', successor_transcript_row_id: 'successor', segment_ordinal: 0 };
    const jump = vi.fn();
    const boundaryJump = vi.fn();
    const { rerender } = render(<ConversationNav chapters={[]} activeUnitIndex={null} onJump={jump} historyBoundaries={[boundary]} onJumpBoundary={boundaryJump} />);
    fireEvent.keyDown(screen.getByRole('button', { name: /Load context boundary/ }), { key: 'Enter' });
    expect(boundaryJump).toHaveBeenLastCalledWith('exact-continuation');
    const loaded = buildConversationChapters([{ kind: 'continuation', key: 'exact-continuation', message: {
      message_id: 'exact-continuation', conversation_id: 'predecessor', sequence_id: 42,
      message_type: 'continuation', content: { summary: 'summary stays in the transcript disclosure' }, created_at: '', event_presentation: null,
    } }]);
    rerender(<ConversationNav chapters={loaded} activeUnitIndex={0} onJump={jump} historyBoundaries={[boundary]} onJumpBoundary={boundaryJump} />);
    expect(screen.getAllByRole('button')).toHaveLength(1);
    const marker = screen.getByRole('button', { name: /Context boundary 1/ });
    expect(marker).toHaveClass('boundary', 'active');
    expect(marker).not.toHaveAccessibleName(/predecessor|successor|exact-continuation/);
    expect(marker).not.toHaveTextContent('summary stays');
    fireEvent.click(marker);
    expect(boundaryJump).toHaveBeenLastCalledWith('exact-continuation');
    expect(jump).not.toHaveBeenCalled();
  });
});
