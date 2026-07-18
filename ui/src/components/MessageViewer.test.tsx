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

  it('bounds fullscreen feedback until it is sent or discarded', async () => {
    const onPresentationChange = vi.fn();
    const onSendNotes = vi.fn().mockResolvedValue(undefined);
    render(
      <ReviewNotesProvider>
        <MessageViewer
          sequenceId={1}
          messages={[agentTextMessage(1, 'Review this line')]}
          onClose={vi.fn()}
          onSendNotes={onSendNotes}
          presentation="fullscreen"
          canTogglePresentation
          onPresentationChange={onPresentationChange}
          inline
        />
      </ReviewNotesProvider>,
    );

    expect(screen.getByRole('dialog', { name: /Message viewer/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Add note to line 1' }));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Please revise' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));
    fireEvent.click(screen.getByRole('button', { name: 'Return to pane' }));

    expect(screen.getByRole('dialog', { name: 'Resolve feedback before returning' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Send feedback and return' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Discard notes and return' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Keep reviewing' }));
    expect(onPresentationChange).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Send notes' }));
    await waitFor(() => expect(onSendNotes).toHaveBeenCalledTimes(1));
    expect(onPresentationChange).toHaveBeenCalledWith('pane');
  });

  it('keeps fullscreen close distinct from return-to-pane', () => {
    const onClose = vi.fn();
    const onPresentationChange = vi.fn();
    render(
      <ReviewNotesProvider>
        <MessageViewer
          sequenceId={1}
          messages={[agentTextMessage(1, 'Review this line')]}
          onClose={onClose}
          onSendNotes={vi.fn()}
          presentation="fullscreen"
          canTogglePresentation
          onPresentationChange={onPresentationChange}
        />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Close viewer' }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onPresentationChange).not.toHaveBeenCalled();
  });

  it('protects fullscreen close with pending notes and closes after discard', () => {
    const onClose = vi.fn();
    const onPresentationChange = vi.fn();
    render(
      <ReviewNotesProvider>
        <MessageViewer
          sequenceId={1}
          messages={[agentTextMessage(1, 'Review this line')]}
          onClose={onClose}
          onSendNotes={vi.fn()}
          presentation="fullscreen"
          canTogglePresentation
          onPresentationChange={onPresentationChange}
        />
      </ReviewNotesProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Add note to line 1' }));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Please revise' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));
    fireEvent.click(screen.getByRole('button', { name: 'Close viewer' }));

    expect(screen.getByRole('dialog', { name: 'Resolve feedback before closing' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Discard notes and close' }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onPresentationChange).not.toHaveBeenCalled();
  });

  it('keeps fullscreen notes and announces a send failure', async () => {
    const onPresentationChange = vi.fn();
    render(
      <ReviewNotesProvider>
        <MessageViewer
          sequenceId={1}
          messages={[agentTextMessage(1, 'Review this line')]}
          onClose={vi.fn()}
          onSendNotes={vi.fn().mockRejectedValue(new Error('Network unavailable'))}
          presentation="fullscreen"
          canTogglePresentation
          onPresentationChange={onPresentationChange}
        />
      </ReviewNotesProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Add note to line 1' }));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'Please revise' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));
    fireEvent.click(screen.getByRole('button', { name: 'Send notes' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('Network unavailable');
    expect(screen.getByRole('button', { name: '1 notes' })).toBeInTheDocument();
    expect(onPresentationChange).not.toHaveBeenCalled();
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
