import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ConversationNav } from './ConversationNav';
import type { Chapter } from '../conversation/conversationChapters';

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
