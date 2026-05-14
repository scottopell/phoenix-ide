import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { InputArea } from './InputArea';
import type { ConversationState, SkillEntry } from '../api';
import { api } from '../api';

const idleState: ConversationState = { type: 'idle' };

interface InputAreaTestProps {
  conversationId: string | undefined;
  draft?: string;
  onDraftChange?: (text: string) => void;
}

function renderInput({
  conversationId,
  draft = '',
  onDraftChange = () => {},
}: InputAreaTestProps) {
  return render(
    <InputArea
      conversationId={conversationId}
      convState={idleState}
      images={[]}
      setImages={() => {}}
      isOffline={false}
      failedMessages={[]}
      draft={draft}
      onDraftChange={onDraftChange}
      onSend={() => {}}
      onCancel={() => {}}
      onRetry={() => {}}
    />,
  );
}

describe('InputArea controlled-draft contract', () => {
  it('renders the draft prop and re-renders when the prop changes', () => {
    const { rerender } = renderInput({ conversationId: 'conv-a', draft: 'draft A' });
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
        draft="draft B"
        onDraftChange={() => {}}
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

    let currentDraft = '';
    const onDraftChange = (text: string) => {
      currentDraft = text;
    };

    const { rerender } = renderInput({
      conversationId: 'conv-a',
      draft: currentDraft,
      onDraftChange,
    });
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;

    fireEvent.change(textarea, { target: { value: '/r' } });
    rerender(
      <InputArea
        conversationId="conv-a"
        convState={idleState}
        images={[]}
        setImages={() => {}}
        isOffline={false}
        failedMessages={[]}
        draft={currentDraft}
        onDraftChange={onDraftChange}
        onSend={() => {}}
        onCancel={() => {}}
        onRetry={() => {}}
      />,
    );
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
        draft=""
        onDraftChange={() => {}}
        onSend={() => {}}
        onCancel={() => {}}
        onRetry={() => {}}
      />,
    );

    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(screen.queryByText('<path>')).not.toBeInTheDocument();
  });
});
