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
    fireEvent.click(screen.getByRole('button', { name: 'Send answer' }));
    await screen.findByText('Answers sent');
    expect(screen.getByLabelText('Captured response').textContent).toContain('Include ancestors');
    expect(screen.queryByRole('button', { name: 'Send answer' })).toBeNull();
    unmount();
    expect(api.respondToQuestion).toBe(original);
    expect(document.documentElement.dataset['theme']).toBe('dark');
  });
  it('retains the real panel and its answer when the fixture endpoint fails', async () => {
    render(<AskUserQuestionFixture scenario={askUserQuestionScenarios.find(s => s.id === 'submit-error')!} />);
    fireEvent.click(screen.getByText('Include ancestors'));
    fireEvent.click(screen.getByRole('button', { name: 'Send answer' }));
    await screen.findByText('Fixture: response rejected. Please retry.');
    expect(screen.getByRole('button', { name: 'Send answer' })).toBeEnabled();
    expect(screen.getByLabelText('Captured response').textContent).toBe('No response sent');
  });
});

describe('AUQ product layout mutation fixture', () => {
  it('keeps answer/dismiss operations local and checks the originating identity', async () => {
    const {installProductConversationFixtureApi} = await import('../productConversation/mockApi');
    const {productConversationScenarios} = await import('../productConversation/scenarios');
    const base = productConversationScenarios[0]!;
    const restore = installProductConversationFixtureApi({...base,latestConversationState:{type:'awaiting_user_response',tool_use_id:'fixture-request',questions:askUserQuestionScenarios[0]!.questions}});
    try {
      const id=base.snapshot!.latest_transcript_row_id!;
      await expect(api.respondToQuestion(id,'wrong',{'Scope':'A'})).rejects.toThrow(/no longer pending/);
      await expect(api.respondToQuestion(id,'fixture-request',{'Scope':'A'})).resolves.toEqual({success:true});
      expect(document.documentElement.dataset['productConversationFixtureQuestionResponse']).toContain('fixture-request');
      await expect(api.dismissQuestion(id,'fixture-request')).rejects.toThrow(/no longer pending/);
    } finally { restore(); }
  });
});
