import React from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import type { UserQuestion } from '../api';
import { QuestionPanel } from './QuestionPanel';
import { blockingQuestionActions } from './questionPanelActions';

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
      actions={blockingQuestionActions('conversation', requestId)}
      showToast={() => {}}
      onAnswered={() => setClosed(true)}
      onDismissed={() => setClosed(true)}
    />
  );
}

describe('QuestionPanel focus lifecycle', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  function content(questions = [question('Q1?'), question('Q2?')]) {
    return <>
      <button>Outside</button>
      <QuestionPanel
        questions={questions}
        actions={blockingQuestionActions('conversation', 'request-q1')}
        showToast={() => {}}
        onAnswered={() => {}}
        onDismissed={() => {}}
      />
    </>;
  }

  function flushFrame() {
    act(() => vi.advanceTimersByTime(20));
  }

  it('preserves outside focus across same-value background refreshes', () => {
    const view = render(content());
    flushFrame();
    const outside = screen.getByRole('button', { name: 'Outside' });
    outside.focus();

    view.rerender(content());
    flushFrame();

    expect(outside).toHaveFocus();
  });

  it('preserves confirmation focus across same-value background refreshes', () => {
    const view = render(content());
    flushFrame();
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    expect(cancel).toHaveFocus();

    view.rerender(content());
    flushFrame();

    expect(cancel).toHaveFocus();
  });

  it('does not let a pending mount frame steal confirmation focus', () => {
    render(content());
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    flushFrame();
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  });

  it('preserves the confirmation opener restore without scheduling panel focus on close', () => {
    const view = render(content());
    flushFrame();
    const dismiss = screen.getByRole('button', { name: 'Dismiss' });
    dismiss.focus();
    fireEvent.click(dismiss);
    flushFrame();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(dismiss).toHaveFocus();
    flushFrame();
    expect(dismiss).toHaveFocus();

    const outside = screen.getByRole('button', { name: 'Outside' });
    outside.focus();
    view.rerender(content());
    flushFrame();
    expect(outside).toHaveFocus();
  });

  it('lets confirmation own focus when it opens before a step frame runs', () => {
    render(content());
    flushFrame();
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    flushFrame();
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  });

  it('focuses a newly mounted request even with same-value questions', () => {
    const view = render(<RequestBoundPanel key="first" requestId="first" text="Q1?" />);
    flushFrame();
    view.rerender(<RequestBoundPanel key="second" requestId="second" text="Q1?" />);
    flushFrame();
    expect(view.container.querySelector('.question-panel')).toHaveFocus();
  });

  it('focuses on appearance and explicit step navigation for keyboard interaction', () => {
    const view = render(content());
    const panel = view.container.querySelector<HTMLDivElement>('.question-panel')!;
    flushFrame();
    expect(panel).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: 'Enter' });
    expect(view.container.querySelector('input[name="Q1?"]')).toBeChecked();

    const next = screen.getByRole('button', { name: 'Next' });
    next.focus();
    fireEvent.click(next);
    flushFrame();
    expect(panel).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
    fireEvent.keyDown(document.activeElement!, { key: 'Enter' });
    expect(view.container.querySelectorAll('input[name="Q2?"]')[1]).toBeChecked();
    fireEvent.keyDown(document.activeElement!, { key: 'Tab', shiftKey: true });
    flushFrame();
    expect(panel).toHaveFocus();
    expect(screen.getByText('Q1?')).toBeInTheDocument();
  });

  it('focuses when an absent question first appears', () => {
    const view = render(content([]));
    flushFrame();
    screen.getByRole('button', { name: 'Outside' }).focus();
    view.rerender(content());
    flushFrame();
    expect(view.container.querySelector('.question-panel')).toHaveFocus();
  });

  it('cancels the superseded frame when the step changes before focus runs', () => {
    const request = vi.spyOn(window, 'requestAnimationFrame');
    const cancel = vi.spyOn(window, 'cancelAnimationFrame');
    const view = render(content());
    const frame = request.mock.results[0]!.value;
    const panel = view.container.querySelector<HTMLDivElement>('.question-panel')!;
    const focus = vi.spyOn(panel, 'focus');
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(cancel).toHaveBeenCalledWith(frame);
    flushFrame();
    expect(focus).toHaveBeenCalledOnce();
    expect(panel).toHaveFocus();
  });

  it('cancels a pending focus frame on unmount', () => {
    const request = vi.spyOn(window, 'requestAnimationFrame');
    const cancel = vi.spyOn(window, 'cancelAnimationFrame');
    const view = render(content());
    const frame = request.mock.results[0]!.value;
    const panel = view.container.querySelector<HTMLDivElement>('.question-panel')!;
    const focus = vi.spyOn(panel, 'focus');
    view.unmount();
    expect(cancel).toHaveBeenCalledWith(frame);
    flushFrame();
    expect(focus).not.toHaveBeenCalled();
  });
});

describe('QuestionPanel request identity', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('keeps keyboard selection on the question until explicit navigation', () => {
    vi.useFakeTimers();
    const respond = vi.spyOn(api, 'respondToQuestion');
    const { container } = render(
      <QuestionPanel
        questions={[question('Q1?'), question('Q2?'), question('Q3?')]}
        actions={blockingQuestionActions('conversation', 'request-q1')}
        showToast={() => {}}
        onAnswered={() => {}}
        onDismissed={() => {}}
      />,
    );
    const panel = container.querySelector('.question-panel');
    expect(panel).not.toBeNull();

    fireEvent.keyDown(panel!, { key: 'Enter' });

    expect(screen.getByText('Q1?')).toBeInTheDocument();
    expect(container.querySelector('input[name="Q1?"]')).toBeChecked();
    expect(respond).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('Q2?')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('Q3?')).toBeInTheDocument();

    act(() => vi.advanceTimersByTime(200));

    expect(screen.getByText('Q3?')).toBeInTheDocument();
    expect(respond).not.toHaveBeenCalled();
  });

  it('requires an explicit submit action after keyboard selection', async () => {
    const respond = vi.spyOn(api, 'respondToQuestion').mockResolvedValue({ success: true });
    const { container } = render(
      <QuestionPanel
        questions={[question('Q1?')]}
        actions={blockingQuestionActions('conversation', 'request-q1')}
        showToast={() => {}}
        onAnswered={() => {}}
        onDismissed={() => {}}
      />,
    );
    const panel = container.querySelector('.question-panel');
    expect(panel).not.toBeNull();

    fireEvent.keyDown(panel!, { key: 'Enter' });
    fireEvent.keyDown(panel!, { key: 'Enter' });

    expect(screen.getByText('Q1?')).toBeInTheDocument();
    expect(respond).not.toHaveBeenCalled();

    fireEvent.keyDown(panel!, { key: 'Enter', ctrlKey: true });

    await waitFor(() => expect(respond).toHaveBeenCalledOnce());
  });

  it('sends the pending identity with an answer', async () => {
    const respond = vi.spyOn(api, 'respondToQuestion').mockResolvedValue({ success: true });
    const onAnswered = vi.fn();
    render(
      <QuestionPanel
        questions={[question('Q1?')]}
        actions={blockingQuestionActions('conversation', 'request-q1')}
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
        actions={blockingQuestionActions('conversation', 'request-q1')}
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
        actions={blockingQuestionActions('conversation', 'request-q1')}
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
