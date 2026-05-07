import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { InputArea } from './InputArea';
import type { ConversationState, SkillEntry } from '../api';
import { api } from '../api';

const idleState: ConversationState = { type: 'idle' };

function renderInput(conversationId: string | undefined) {
  return render(
    <InputArea
      conversationId={conversationId}
      convState={idleState}
      images={[]}
      setImages={() => {}}
      isOffline={false}
      failedMessages={[]}
      onSend={() => {}}
      onCancel={() => {}}
      onRetry={() => {}}
    />,
  );
}

describe('InputArea conversation scope', () => {
  it('switches synchronously to the new conversation draft', () => {
    localStorage.setItem('phoenix:draft:conv-a', 'draft A');
    localStorage.setItem('phoenix:draft:conv-b', 'draft B');

    const { rerender } = renderInput('conv-a');
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    expect(textarea.value).toBe('draft A');

    rerender(
      <InputArea
        conversationId="conv-b"
        convState={idleState}
        images={[]}
        setImages={() => {}}
        isOffline={false}
        failedMessages={[]}
        onSend={() => {}}
        onCancel={() => {}}
        onRetry={() => {}}
      />,
    );

    expect(textarea.value).toBe('draft B');
  });

  it('clears autocomplete and skill-hint state when conversation changes', async () => {
    const skills: SkillEntry[] = [
      {
        name: 'review',
        description: 'Review changes',
        argument_hint: '<path>',
        source: 'project',
        path: '/repo/.agents/skills/review/SKILL.md',
      },
    ];
    vi.spyOn(api, 'listConversationSkills').mockResolvedValue({ skills });

    const { rerender } = renderInput('conv-a');
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;

    fireEvent.change(textarea, { target: { value: '/r' } });
    expect(await screen.findByRole('listbox', { name: '/ autocomplete' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('option', { name: /review/ }));
    expect(screen.getByText('<path>')).toBeInTheDocument();

    rerender(
      <InputArea
        conversationId="conv-b"
        convState={idleState}
        images={[]}
        setImages={() => {}}
        isOffline={false}
        failedMessages={[]}
        onSend={() => {}}
        onCancel={() => {}}
        onRetry={() => {}}
      />,
    );

    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(screen.queryByText('<path>')).not.toBeInTheDocument();
  });
});
