import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from '../api';
import type { CoordinatorQuestion } from '../api';
import {
  receiveCoordinatorQuestions,
  resetCoordinatorQuestionsForTest,
} from '../coordinator/coordinatorQuestions';
import { CoordinatorQuestionDock } from './CoordinatorQuestionDock';
import { EmberEntry } from './EmberEntry';

function open(requestId: string, text: string): CoordinatorQuestion {
  return {
    request_id: requestId,
    coordinator_conversation_id: 'ember-1',
    asked_at: new Date().toISOString(),
    questions: [{
      question: text,
      header: 'Choice',
      options: [
        { label: 'First', description: 'First option' },
        { label: 'Second', description: 'Second option' },
      ],
      multiSelect: false,
    }],
  };
}

describe('Ember questions (REQ-AUQ-011)', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    resetCoordinatorQuestionsForTest();
  });

  it('shows the oldest open question and sends the answer to Ember, not to a conversation wait', async () => {
    const answer = vi.spyOn(api, 'answerCoordinatorQuestion').mockResolvedValue({ success: true });
    const respond = vi.spyOn(api, 'respondToQuestion');
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?'), open('q-2', 'Deploy tonight?')]));
    render(<CoordinatorQuestionDock />);

    expect(screen.getByText('Merge now?')).toBeInTheDocument();
    expect(screen.getByText('1 more waiting')).toBeInTheDocument();
    expect(screen.getByText(/Ember keeps working while you decide/)).toBeInTheDocument();

    fireEvent.click(screen.getByTitle('Select First'));
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }));

    await waitFor(() => expect(answer).toHaveBeenCalledWith('q-1', { 'Merge now?': 'First' }, undefined));
    expect(respond).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByText('Deploy tonight?')).toBeInTheDocument());
    expect(screen.queryByText('1 more waiting')).not.toBeInTheDocument();
  });

  it('dismisses through the Ember question endpoint', async () => {
    const dismiss = vi.spyOn(api, 'dismissCoordinatorQuestion').mockResolvedValue({ success: true });
    const dismissWait = vi.spyOn(api, 'dismissQuestion');
    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?')]));
    render(<CoordinatorQuestionDock />);

    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.getByText(/Ember is told you dismissed it/)).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole('button', { name: 'Dismiss' })[1]!);

    await waitFor(() => expect(dismiss).toHaveBeenCalledWith('q-1'));
    expect(dismissWait).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByText('Merge now?')).not.toBeInTheDocument());
  });

  it('marks the Ember entry while a question waits', () => {
    const { rerender } = render(<EmberEntry onClick={() => {}} />);
    expect(screen.getByRole('button', { name: 'Open Ember' })).toBeInTheDocument();

    act(() => receiveCoordinatorQuestions([open('q-1', 'Merge now?'), open('q-2', 'Deploy?')]));
    rerender(<EmberEntry onClick={() => {}} />);
    expect(screen.getByRole('button', { name: 'Open Ember, 2 questions for you' })).toBeInTheDocument();
    expect(screen.getByText('2 questions for you')).toBeInTheDocument();
  });
});
