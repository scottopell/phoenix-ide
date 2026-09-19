import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { QuestionPanel } from './QuestionPanel';
import { api, type UserQuestion } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      respondToQuestion: vi.fn(),
      dismissQuestion: vi.fn(),
    },
  };
});

const question = (secondLabel: string): UserQuestion => ({
  question: 'Choose path?',
  header: 'Choice',
  options: [
    { label: 'Alpha', description: 'Alpha path' },
    { label: secondLabel, description: `${secondLabel} path` },
  ],
  multiSelect: false,
});

const panel = (toolUseId: string, q: UserQuestion) => (
  <QuestionPanel
    key={toolUseId}
    questions={[q]}
    conversationId="conv-1"
    toolUseId={toolUseId}
    showToast={() => {}}
    onAnswered={() => {}}
    onDismissed={() => {}}
  />
);

describe('QuestionPanel', () => {
  it('remounts by tool_use_id so repeated question text cannot submit stale answers', async () => {
    vi.mocked(api.respondToQuestion).mockResolvedValue({ success: true });

    const { rerender } = render(panel('q1', question('Beta')));

    fireEvent.click(screen.getByTitle('Select Beta'));
    expect(screen.getByRole('button', { name: /submit/i })).toBeEnabled();

    rerender(panel('q2', question('Gamma')));

    expect(screen.getByTitle('Select Alpha').querySelector('input')).not.toBeChecked();
    expect(screen.getByTitle('Select Gamma').querySelector('input')).not.toBeChecked();
    expect(screen.getByRole('button', { name: /submit/i })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: /submit/i }));
    expect(api.respondToQuestion).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTitle('Select Gamma'));
    fireEvent.click(screen.getByRole('button', { name: /submit/i }));

    await waitFor(() => expect(api.respondToQuestion).toHaveBeenCalledTimes(1));
    expect(api.respondToQuestion).toHaveBeenCalledWith(
      'conv-1',
      'q2',
      { 'Choose path?': 'Gamma' },
      undefined
    );
  });
});
