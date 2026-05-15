import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { useRef } from 'react';
import { InputArea } from './InputArea';
import type { InputAreaHandle } from './InputArea';
import type { ConversationState, SkillEntry } from '../api';
import { api } from '../api';

const idleState: ConversationState = { type: 'idle' };

interface InputAreaTestProps {
  conversationId: string | undefined;
  draft?: string;
  onDraftChange?: (text: string) => void;
  focusToken?: number;
}

function renderInput({
  conversationId,
  draft = '',
  onDraftChange = () => {},
  focusToken,
}: InputAreaTestProps) {
  const focusProps = focusToken === undefined ? {} : { focusToken };
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
      {...focusProps}
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

describe('InputArea focusToken contract', () => {
  it('does not steal focus on mount when focusToken is the initial 0', () => {
    // External anchor to verify focus didn't move into the textarea.
    const anchor = document.createElement('button');
    anchor.textContent = 'anchor';
    document.body.appendChild(anchor);
    anchor.focus();
    expect(document.activeElement).toBe(anchor);

    renderInput({ conversationId: 'conv-a', focusToken: 0 });

    expect(document.activeElement).toBe(anchor);
    document.body.removeChild(anchor);
  });

  it('does not steal focus on mount when focusToken is undefined', () => {
    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    anchor.focus();

    renderInput({ conversationId: 'conv-a' });

    expect(document.activeElement).toBe(anchor);
    document.body.removeChild(anchor);
  });

  it('focuses the textarea when focusToken is bumped', () => {
    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    anchor.focus();

    const { rerender } = renderInput({ conversationId: 'conv-a', focusToken: 0 });
    expect(document.activeElement).toBe(anchor);

    rerender(
      <InputArea
        conversationId="conv-a"
        convState={idleState}
        images={[]}
        setImages={() => {}}
        isOffline={false}
        failedMessages={[]}
        draft=""
        onDraftChange={() => {}}
        focusToken={1}
        onSend={() => {}}
        onCancel={() => {}}
        onRetry={() => {}}
      />,
    );

    expect(document.activeElement).toBe(screen.getByRole('textbox'));
    document.body.removeChild(anchor);
  });

  it('exposes an imperative focus() handle that focuses the textarea', () => {
    function Harness() {
      const ref = useRef<InputAreaHandle>(null);
      return (
        <>
          <button onClick={() => ref.current?.focus()}>do-focus</button>
          <InputArea
            ref={ref}
            conversationId="conv-a"
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
          />
        </>
      );
    }

    render(<Harness />);
    const textarea = screen.getByRole('textbox');
    expect(document.activeElement).not.toBe(textarea);

    fireEvent.click(screen.getByText('do-focus'));
    expect(document.activeElement).toBe(textarea);
  });
});
