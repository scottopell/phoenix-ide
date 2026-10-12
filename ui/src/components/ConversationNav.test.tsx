import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ConversationNav } from './ConversationNav';
import type { Chapter } from '../conversation/conversationChapters';
import { buildConversationChapters } from '../conversation/conversationChapters';
import type { HistoricalUnit } from '../conversation/renderUnits';

const chapters: Chapter[] = [
  { unitIndex: 0, kind: 'prompt', label: 'API prompt', sequenceId: 1, origin: { kind: 'user_api' } },
  { unitIndex: 1, kind: 'prose', label: 'Response', sequenceId: 2 },
  { unitIndex: 2, kind: 'prompt', label: 'Forwarded prompt', sequenceId: 3, origin: {
    kind: 'internal_conversation', source_call: null, product_conversation_id: 'source-product', transcript_id: 'source-row',
  } },
  { unitIndex: 3, kind: 'prompt', label: 'Event prompt', sequenceId: 4, origin: { kind: 'subscription_event', event_id: 'event-1' } },
  { unitIndex: 4, kind: 'prompt', label: 'Automatic prompt', sequenceId: 5, origin: { kind: 'system_generated' } },
  { unitIndex: 5, kind: 'prompt', label: 'Old prompt', sequenceId: 6 },
];

describe('ConversationNav input provenance', () => {
  it('keeps the latest real prompt navigable across event bursts and reload', () => {
    const units: HistoricalUnit[] = ['human', 'event-1', 'event-2'].map((id, index) => ({
      kind: 'user', key: id, message: {
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
    expect(onJump).toHaveBeenCalledWith(0);
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
    expect(onJump).toHaveBeenCalledWith(2);
  });
});

describe('aggregate boundary navigation', () => {
  it('uses one exact boundary identity before and after its chronological unit loads', () => {
    const boundary = { message_id: 'exact-continuation', predecessor_transcript_row_id: 'predecessor', successor_transcript_row_id: 'successor', segment_ordinal: 0 };
    const jump = vi.fn();
    const boundaryJump = vi.fn();
    const { rerender } = render(<ConversationNav chapters={[]} activeUnitIndex={null} onJump={jump} historyBoundaries={[boundary]} onJumpBoundary={boundaryJump} />);
    fireEvent.keyDown(screen.getByRole('button', { name: /Load context boundary/ }), { key: 'Enter' });
    expect(boundaryJump).toHaveBeenLastCalledWith('exact-continuation');
    const loaded = buildConversationChapters([{ kind: 'continuation', key: 'exact-continuation', message: {
      message_id: 'exact-continuation', conversation_id: 'predecessor', sequence_id: 42,
      message_type: 'continuation', content: { summary: 'summary stays in the transcript disclosure' }, created_at: '',
    } }]);
    rerender(<ConversationNav chapters={loaded} activeUnitIndex={0} onJump={jump} historyBoundaries={[boundary]} onJumpBoundary={boundaryJump} />);
    expect(screen.getAllByRole('button')).toHaveLength(1);
    const marker = screen.getByRole('button', { name: /Context boundary: predecessor/ });
    expect(marker).toHaveClass('boundary', 'active');
    expect(marker).not.toHaveTextContent('summary stays');
    fireEvent.click(marker);
    expect(boundaryJump).toHaveBeenLastCalledWith('exact-continuation');
    expect(jump).not.toHaveBeenCalled();
  });
});
