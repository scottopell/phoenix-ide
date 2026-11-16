import { useState } from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { Message } from '../../api';
import { ReviewNotesProvider } from '../../contexts/ReviewNotesContext';
import { ThemeContext } from '../../hooks/useTheme';
import { MessageViewer } from '../MessageViewer';

const mermaidState = vi.hoisted(() => ({ mounts: 0 }));

vi.mock('../MermaidDiagram', () => ({
  MermaidDiagram: ({ code }: { code: string }) => {
    const [instance] = useState(() => ++mermaidState.mounts);
    return <div data-testid="mermaid" data-instance={instance}>{code}</div>;
  },
}));

function agentMessage(text: string): Message {
  return {
    event_presentation: null,
    message_id: 'agent-1',
    sequence_id: 1,
    conversation_id: 'conv-1',
    message_type: 'agent',
    content: [{ type: 'text', text }],
    display_data: null,
    created_at: '2026-01-01T00:00:00Z',
  };
}

function viewer(messages: Message[], onSendNotes?: (notes: string) => void) {
  return (
    <ThemeContext.Provider value={{ theme: 'dark', toggleTheme: () => {} }}>
      <ReviewNotesProvider>
        <MessageViewer
          sequenceId={1}
          messages={messages}
          onClose={vi.fn()}
          onSendNotes={onSendNotes}
          inline
        />
      </ReviewNotesProvider>
    </ThemeContext.Provider>
  );
}

describe('MessageViewer callback capability identity', () => {
  it('keeps Mermaid mounted for callback replacement while using the latest callback', async () => {
    mermaidState.mounts = 0;
    const originalCallback = vi.fn();
    const latestCallback = vi.fn();
    const messages = [agentMessage('# Original\n\n```mermaid\nflowchart TD\n  A --> B\n```')];
    const { rerender } = render(viewer(messages, originalCallback));
    const originalDiagram = screen.getByTestId('mermaid');

    rerender(viewer(messages, latestCallback));
    expect(screen.getByTestId('mermaid')).toBe(originalDiagram);
    expect(mermaidState.mounts).toBe(1);

    fireEvent.click(screen.getByRole('button', { name: 'Add note to line 1' }));
    fireEvent.change(screen.getByPlaceholderText('Add your note… (Cmd/Ctrl+Enter to save)'), {
      target: { value: 'Please revise this section' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add Note' }));
    fireEvent.click(screen.getByRole('button', { name: 'Send notes' }));

    await waitFor(() => expect(latestCallback).toHaveBeenCalledTimes(1));
    expect(latestCallback).toHaveBeenCalledWith(expect.stringContaining('Please revise this section'));
    expect(originalCallback).not.toHaveBeenCalled();

    rerender(viewer(messages));
    expect(screen.queryByRole('button', { name: 'Add note to line 1' })).not.toBeInTheDocument();
    rerender(viewer(messages, latestCallback));
    expect(screen.getByRole('button', { name: 'Add note to line 1' })).toBeInTheDocument();

    const updatedMessages = [agentMessage('# Updated content\n\n```mermaid\nflowchart LR\n  C --> D\n```')];
    rerender(viewer(updatedMessages, latestCallback));
    expect(screen.getByRole('heading', { name: /^Updated content/ })).toBeInTheDocument();
    expect(screen.getByTestId('mermaid')).toHaveTextContent('flowchart LR');
    fireEvent.click(screen.getByRole('button', { name: 'Add note to line 1' }));
    expect(screen.getByPlaceholderText('Add your note… (Cmd/Ctrl+Enter to save)')).toBeInTheDocument();
    expect(originalCallback).not.toHaveBeenCalled();
  });
});
