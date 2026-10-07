import React from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import type { UserQuestion } from '../api';
import { QuestionPanel } from './QuestionPanel';

function question(text: string): UserQuestion {
  return {
    question: text,
    header: 'Choice',
    options: [
      { label: 'First', description: 'First option' },
      { label: 'Second', description: 'Second option' },
    ],
    multiSelect: false,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function RequestBoundPanel({ requestId, text }: { requestId: string; text: string }) {
  const [closed, setClosed] = React.useState(false);
  if (closed) return <p>closed</p>;
  return (
    <QuestionPanel
      key={requestId}
      questions={[question(text)]}
      conversationId="conversation"
      requestId={requestId}
      showToast={() => {}}
      onAnswered={() => setClosed(true)}
      onDismissed={() => setClosed(true)}
    />
  );
}

describe('QuestionPanel request identity', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('sends the pending identity with an answer', async () => {
    const respond = vi.spyOn(api, 'respondToQuestion').mockResolvedValue({ success: true });
    const onAnswered = vi.fn();
    render(
      <QuestionPanel
        questions={[question('Q1?')]}
        conversationId="conversation"
        requestId="request-q1"
        showToast={() => {}}
        onAnswered={onAnswered}
        onDismissed={() => {}}
      />,
    );

    fireEvent.click(screen.getByTitle('Select First'));
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }));

    await waitFor(() => expect(respond).toHaveBeenCalledWith(
      'conversation',
      'request-q1',
      { 'Q1?': 'First' },
      undefined,
    ));
    await waitFor(() => expect(onAnswered).toHaveBeenCalledOnce());
  });

  it('does not let a completed Q1 request close authoritative Q2', async () => {
    const pending = deferred<{ success: boolean }>();
    vi.spyOn(api, 'respondToQuestion').mockReturnValue(pending.promise);
    const view = render(<RequestBoundPanel requestId="request-q1" text="Q1?" />);

    fireEvent.click(screen.getByTitle('Select First'));
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }));
    view.rerender(<RequestBoundPanel requestId="request-q2" text="Q2?" />);
    expect(screen.getByText('Q2?')).toBeInTheDocument();

    await act(async () => pending.resolve({ success: true }));

    expect(screen.getByText('Q2?')).toBeInTheDocument();
    expect(screen.queryByText('closed')).not.toBeInTheDocument();
  });

  it('keeps the panel open when answer processing fails', async () => {
    vi.spyOn(api, 'respondToQuestion').mockRejectedValue(new Error('stale question'));
    const onAnswered = vi.fn();
    render(
      <QuestionPanel
        questions={[question('Q1?')]}
        conversationId="conversation"
        requestId="request-q1"
        showToast={() => {}}
        onAnswered={onAnswered}
        onDismissed={() => {}}
      />,
    );

    fireEvent.click(screen.getByTitle('Select First'));
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }));

    expect(await screen.findByText('stale question')).toBeInTheDocument();
    expect(onAnswered).not.toHaveBeenCalled();
    expect(screen.getByText('Q1?')).toBeInTheDocument();
  });

  it('sends the pending identity on dismiss and stays open on failure', async () => {
    const dismiss = vi.spyOn(api, 'dismissQuestion').mockRejectedValue(new Error('stale question'));
    const onDismissed = vi.fn();
    render(
      <QuestionPanel
        questions={[question('Q1?')]}
        conversationId="conversation"
        requestId="request-q1"
        showToast={() => {}}
        onAnswered={() => {}}
        onDismissed={onDismissed}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss' })[1]!);

    await waitFor(() => expect(dismiss).toHaveBeenCalledWith('conversation', 'request-q1'));
    expect(await screen.findByText('stale question')).toBeInTheDocument();
    expect(onDismissed).not.toHaveBeenCalled();
    expect(screen.getByText('Q1?')).toBeInTheDocument();
  });
});
