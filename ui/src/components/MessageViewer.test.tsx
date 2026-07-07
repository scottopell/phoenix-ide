import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { Message } from '../api';
import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import { MessageViewer } from './MessageViewer';

function agentTextMessage(sequenceId: number, text: string): Message {
  return {
    message_id: `agent-${sequenceId}`,
    sequence_id: sequenceId,
    conversation_id: 'conv-1',
    message_type: 'agent',
    content: [{ type: 'text', text }],
    display_data: null,
    created_at: '2026-01-01T00:00:00Z',
  };
}

function renderViewer(sequenceId: number, messages: Message[]) {
  return render(
    <ReviewNotesProvider>
      <MessageViewer
        sequenceId={sequenceId}
        messages={messages}
        onClose={vi.fn()}
        onSendNotes={vi.fn()}
        inline
      />
    </ReviewNotesProvider>,
  );
}

describe('MessageViewer', () => {
  it('resets an in-progress annotation when switching messages', async () => {
    const messages = [
      agentTextMessage(1, 'First proposal line'),
      agentTextMessage(2, 'Second proposal line'),
    ];
    const view = renderViewer(1, messages);

    fireEvent.click(screen.getByRole('button', { name: 'Add note to line 1' }));
    expect(screen.getByRole('textbox')).toBeInTheDocument();

    view.rerender(
      <ReviewNotesProvider>
        <MessageViewer
          sequenceId={2}
          messages={messages}
          onClose={vi.fn()}
          onSendNotes={vi.fn()}
          inline
        />
      </ReviewNotesProvider>,
    );

    await waitFor(() => {
      expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    });
    expect(screen.getByText('Second proposal line')).toBeInTheDocument();
  });

  it('closes the notes panel when switching messages', async () => {
    const messages = [
      agentTextMessage(1, 'First proposal line'),
      agentTextMessage(2, 'Second proposal line'),
    ];
    const view = renderViewer(1, messages);

    fireEvent.click(screen.getByRole('button', { name: 'Add note to line 1' }));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Check this' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));
    fireEvent.click(screen.getByRole('button', { name: '1 notes' }));
    expect(screen.getByText('Notes (1)')).toBeInTheDocument();

    view.rerender(
      <ReviewNotesProvider>
        <MessageViewer
          sequenceId={2}
          messages={messages}
          onClose={vi.fn()}
          onSendNotes={vi.fn()}
          inline
        />
      </ReviewNotesProvider>,
    );

    await waitFor(() => {
      expect(screen.queryByText(/Notes \(/)).not.toBeInTheDocument();
    });
    expect(screen.getByText('Second proposal line')).toBeInTheDocument();
  });

  it('keeps line refs registered so note jumps can highlight the target line', async () => {
    Element.prototype.scrollIntoView = vi.fn();
    const messages = [agentTextMessage(1, 'First proposal line')];
    renderViewer(1, messages);

    fireEvent.click(screen.getByRole('button', { name: 'Add note to line 1' }));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Check this' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));

    fireEvent.click(screen.getByRole('button', { name: '1 notes' }));
    fireEvent.click(screen.getByRole('button', { name: 'Line 1' }));

    expect(Element.prototype.scrollIntoView).toHaveBeenCalled();
    await waitFor(() => {
      expect(document.querySelector('[data-line="1"]')).toHaveClass('annotatable--highlighted');
    });
  });
});
