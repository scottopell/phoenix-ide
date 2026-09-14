import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { api } from '../../api';
import { AskUserQuestionFixture } from './renderFixture';
import { askUserQuestionScenarios } from './scenarios';

afterEach(cleanup);
describe('AskUserQuestionFixture', () => {
  it('captures the real panel submission and restores API methods and theme', async () => {
    const original = api.respondToQuestion;
    document.documentElement.dataset['theme'] = 'dark';
    const { unmount } = render(<AskUserQuestionFixture scenario={askUserQuestionScenarios[0]!} />);
    fireEvent.click(screen.getByText('Include ancestors'));
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }));
    await screen.findByText('Response sent');
    expect(screen.getByLabelText('Captured response').textContent).toContain('Include ancestors');
    expect(screen.queryByRole('button', { name: 'Submit' })).toBeNull();
    unmount();
    expect(api.respondToQuestion).toBe(original);
    expect(document.documentElement.dataset['theme']).toBe('dark');
  });
  it('retains the real panel and its answer when the fixture endpoint fails', async () => {
    render(<AskUserQuestionFixture scenario={askUserQuestionScenarios.find(s => s.id === 'submit-error')!} />);
    fireEvent.click(screen.getByText('Include ancestors'));
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }));
    await screen.findByText('Fixture: response failed. Please retry.');
    expect(screen.getByRole('button', { name: 'Submit' })).toBeEnabled();
    expect(screen.getByLabelText('Captured response').textContent).toBe('No response sent');
  });
});
