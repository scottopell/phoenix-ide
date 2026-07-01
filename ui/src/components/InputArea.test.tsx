import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { useRef } from 'react';
import { InputArea } from './InputArea';
import { AgentMessage } from './MessageComponents';
import type { InputAreaHandle } from './InputArea';
import type { ConversationState, Message, SkillEntry } from '../api';
import { api } from '../api';

const idleState: ConversationState = { type: 'idle' };

interface InputAreaTestProps {
  cwd: string | undefined;
  scopeKey?: string | undefined;
  convState?: ConversationState;
  draft?: string;
  onDraftChange?: (text: string) => void;
  onCancel?: () => void;
  focusToken?: number;
}

function renderInput({
  cwd,
  scopeKey,
  convState = idleState,
  draft = '',
  onDraftChange = () => {},
  onCancel = () => {},
  focusToken,
}: InputAreaTestProps) {
  const focusProps = focusToken === undefined ? {} : { focusToken };
  return render(
    <InputArea
      cwd={cwd}
      scopeKey={scopeKey ?? cwd}
      convState={convState}
      images={[]}
      setImages={() => {}}
      isOffline={false}
      failedMessages={[]}
      draft={draft}
      onDraftChange={onDraftChange}
      {...focusProps}
      onSend={() => {}}
      onCancel={onCancel}
      onRetry={() => {}}
    />,
  );
}

describe('InputArea controlled-draft contract', () => {
  it('renders the draft prop and re-renders when the prop changes', () => {
    const { rerender } = renderInput({ cwd: 'conv-a', draft: 'draft A' });
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    expect(textarea.value).toBe('draft A');

    rerender(
      <InputArea
        cwd="conv-b"
        scopeKey="conv-b"
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

  it('clears autocomplete and skill-hint state when the composer scope changes within one cwd', async () => {
    // Two conversations in the same repo share a `cwd` but have distinct
    // `scopeKey`s. Switching between them must reset transient autocomplete /
    // skill-hint state rather than leak it across conversations.
    const skills: SkillEntry[] = [
      {
        name: 'review',
        description: 'Review changes',
        argument_hint: '<path>',
        source: 'project',
        path: '/repo/.agents/skills/review/SKILL.md',
      },
    ];
    vi.spyOn(api, 'listProjectSkills').mockResolvedValue({ skills });

    let currentDraft = '';
    const onDraftChange = (text: string) => {
      currentDraft = text;
    };

    const { rerender } = renderInput({
      cwd: '/repo',
      scopeKey: 'conv-a',
      draft: currentDraft,
      onDraftChange,
    });
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;

    fireEvent.change(textarea, { target: { value: '/r' } });
    rerender(
      <InputArea
        cwd="/repo"
        scopeKey="conv-a"
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

    // Same cwd, different conversation: transient state must reset.
    rerender(
      <InputArea
        cwd="/repo"
        scopeKey="conv-b"
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

    renderInput({ cwd: 'conv-a', focusToken: 0 });

    expect(document.activeElement).toBe(anchor);
    document.body.removeChild(anchor);
  });

  it('does not steal focus on mount when focusToken is undefined', () => {
    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    anchor.focus();

    renderInput({ cwd: 'conv-a' });

    expect(document.activeElement).toBe(anchor);
    document.body.removeChild(anchor);
  });

  it('focuses the textarea when focusToken is bumped', () => {
    const anchor = document.createElement('button');
    document.body.appendChild(anchor);
    anchor.focus();

    const { rerender } = renderInput({ cwd: 'conv-a', focusToken: 0 });
    expect(document.activeElement).toBe(anchor);

    rerender(
      <InputArea
        cwd="conv-a"
        scopeKey="conv-a"
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
            cwd="conv-a"
            scopeKey="conv-a"
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

describe('InputArea cancellation affordance', () => {
  it('shows Stop and calls onCancel for cancellable working states', () => {
    const onCancel = vi.fn();
    renderInput({
      cwd: 'conv-cancel',
      convState: { type: 'llm_requesting', attempt: 1 },
      onCancel,
    });

    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it('renders continuation progress without a Stop button', () => {
    const onCancel = vi.fn();
    renderInput({
      cwd: 'conv-continuation',
      convState: { type: 'awaiting_continuation', attempt: 1 },
      onCancel,
    });

    expect(screen.getByRole('status')).toHaveTextContent('Compacting conversation...');
    expect(screen.getByText(/preserve context for a new conversation/i)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /stop/i })).not.toBeInTheDocument();
  });

  it('does not send from click or Enter while awaiting continuation', () => {
    const onSend = vi.fn();
    render(
      <InputArea
        cwd="conv-continuation"
        scopeKey="conv-continuation"
        convState={{ type: 'awaiting_continuation', attempt: 1 }}
        images={[]}
        setImages={() => {}}
        isOffline={false}
        failedMessages={[]}
        draft="queued follow-up"
        onDraftChange={() => {}}
        onSend={onSend}
        onCancel={() => {}}
        onRetry={() => {}}
      />,
    );

    const send = screen.getByRole('button', { name: 'Send' });
    expect(send).toBeDisabled();
    fireEvent.click(send);
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
    expect(onSend).not.toHaveBeenCalled();
  });

  it('keeps Stopping disabled and blocks sends while cancellation is pending', () => {
    const onSend = vi.fn();
    render(
      <InputArea
        cwd="conv-cancelling"
        scopeKey="conv-cancelling"
        convState={{ type: 'cancelling_tool', current_tool: { id: 't', name: 'bash', input: {} } }}
        images={[]}
        setImages={() => {}}
        isOffline={false}
        failedMessages={[]}
        draft="do not send"
        onDraftChange={() => {}}
        onSend={onSend}
        onCancel={() => {}}
        onRetry={() => {}}
      />,
    );

    expect(screen.getByRole('button', { name: 'Stopping...' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Send' })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
    expect(onSend).not.toHaveBeenCalled();
  });

  it('blocks sends while the first send is still optimistic', () => {
    const onSend = vi.fn();
    render(
      <InputArea
        cwd="conv-awaiting-llm"
        scopeKey="conv-awaiting-llm"
        convState={{ type: 'awaiting_llm' }}
        images={[]}
        setImages={() => {}}
        isOffline={false}
        failedMessages={[]}
        draft="too soon"
        onDraftChange={() => {}}
        onSend={onSend}
        onCancel={() => {}}
        onRetry={() => {}}
      />,
    );

    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
    expect(onSend).not.toHaveBeenCalled();
  });

  it('does not call onCancel for Escape while awaiting continuation', () => {
    const onCancel = vi.fn();
    renderInput({
      cwd: 'conv-continuation',
      convState: { type: 'awaiting_continuation', attempt: 1 },
      onCancel,
    });

    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Escape' });
    expect(onCancel).not.toHaveBeenCalled();
  });
});

function renderBashTool(input: Record<string, unknown>) {
  const agentMessage: Message = {
    message_id: `agent-${JSON.stringify(input)}`,
    sequence_id: 1,
    conversation_id: 'conv-bash',
    message_type: 'agent',
    content: [
      {
        type: 'tool_use',
        id: 'tool-1',
        name: 'bash',
        input,
      },
    ],
    created_at: '2026-01-01T00:00:00Z',
  };

  render(<AgentMessage message={agentMessage} toolResults={new Map()} />);
}

describe('bash tool rendering', () => {
  it('renders modern bash handle operations from op and handle', () => {
    renderBashTool({ op: 'peek', handle: 'b-13', lines: 20 });
    expect(screen.getByText('peek b-13 · last 20 lines')).toBeInTheDocument();

    renderBashTool({ op: 'wait', handle: 'b-13', wait_seconds: 60, since: 42 });
    expect(screen.getByText('wait b-13 (up to 60s) · since 42')).toBeInTheDocument();

    renderBashTool({ op: 'kill', handle: 'b-13', signal: 'KILL' });
    expect(screen.getByText('kill b-13 (KILL)')).toBeInTheDocument();
  });

  it('renders malformed bash input as explicit JSON instead of placeholder text', () => {
    renderBashTool({ bogus: true });
    expect(screen.getByText('bash {"bogus":true}')).toBeInTheDocument();

    renderBashTool({ op: 'peek', handle: 'b-13', since: 0 });
    expect(screen.getByText('peek b-13')).toBeInTheDocument();

    renderBashTool({ op: 'kill', handle: 'b-13', signal: 'NOPE' });
    expect(screen.getByText('bash {"op":"kill","handle":"b-13","signal":"NOPE"}')).toBeInTheDocument();

    renderBashTool({ op: 'run', cmd: 'echo hi', command: 'legacy extra' });
    expect(screen.getByText('bash {"op":"run","cmd":"echo hi","command":"legacy extra"}')).toBeInTheDocument();

    expect(screen.queryByText('$ <bash>')).not.toBeInTheDocument();
  });

  it('renders legacy cmd-only bash input as a command', () => {
    renderBashTool({ cmd: 'echo legacy' });

    expect(screen.getByText('$ echo legacy')).toBeInTheDocument();
  });

  it('does not render a peek draft mutation action for running output', () => {
    const agentMessage: Message = {
      message_id: 'agent-1',
      sequence_id: 1,
      conversation_id: 'conv-bash',
      message_type: 'agent',
      content: [
        {
          type: 'tool_use',
          id: 'tool-1',
          name: 'bash',
          input: { op: 'run', cmd: 'sleep 100' },
        },
      ],
      created_at: '2026-01-01T00:00:00Z',
    };
    const resultMessage: Message = {
      message_id: 'tool-result-1',
      sequence_id: 2,
      conversation_id: 'conv-bash',
      message_type: 'tool',
      content: {
        tool_use_id: 'tool-1',
        content: JSON.stringify({ status: 'running', handle: 'b-1', lines: [] }),
      },
      created_at: '2026-01-01T00:00:01Z',
    };

    render(<AgentMessage message={agentMessage} toolResults={new Map([['tool-1', resultMessage]])} />);

    expect(screen.getByText('running')).toBeInTheDocument();
    expect(screen.getByText('b-1')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /peek/i })).not.toBeInTheDocument();
  });
});
describe('InputArea file-tree drag-and-drop', () => {
  const FILE_TREE_DRAG_TYPE = 'application/x-phoenix-file-path';

  function renderForDnd() {
    return renderInput({ cwd: '/repo', scopeKey: 'conv-dnd' });
  }

  function makeDropEvent(types: string[], data: Record<string, string> = {}, files: File[] = []) {
    return {
      preventDefault: vi.fn(),
      dataTransfer: {
        types,
        getData: (type: string) => data[type] ?? '',
        files,
      },
    };
  }

  it('inserts @file reference for text files on custom-type drop', () => {
    const handler = vi.fn();
    window.addEventListener('phoenix:insert-draft', handler);
    renderForDnd();

    const footer = document.getElementById('input-area')!;
    const dropEvent = makeDropEvent(
      [FILE_TREE_DRAG_TYPE],
      { [FILE_TREE_DRAG_TYPE]: JSON.stringify({ relativePath: 'src/main.rs', isDirectory: false, isText: true }) },
    );
    fireEvent.drop(footer, dropEvent as unknown as React.DragEvent);

    expect(handler).toHaveBeenCalledWith(
      expect.objectContaining({ detail: { text: '@src/main.rs ' } }),
    );
    window.removeEventListener('phoenix:insert-draft', handler);
  });

  it('inserts ./path for directories on custom-type drop', () => {
    const handler = vi.fn();
    window.addEventListener('phoenix:insert-draft', handler);
    renderForDnd();

    const footer = document.getElementById('input-area')!;
    const dropEvent = makeDropEvent(
      [FILE_TREE_DRAG_TYPE],
      { [FILE_TREE_DRAG_TYPE]: JSON.stringify({ relativePath: 'src', isDirectory: true, isText: false }) },
    );
    fireEvent.drop(footer, dropEvent as unknown as React.DragEvent);

    expect(handler).toHaveBeenCalledWith(
      expect.objectContaining({ detail: { text: './src ' } }),
    );
    window.removeEventListener('phoenix:insert-draft', handler);
  });

  it('inserts ./path for non-text files on custom-type drop', () => {
    const handler = vi.fn();
    window.addEventListener('phoenix:insert-draft', handler);
    renderForDnd();

    const footer = document.getElementById('input-area')!;
    const dropEvent = makeDropEvent(
      [FILE_TREE_DRAG_TYPE],
      { [FILE_TREE_DRAG_TYPE]: JSON.stringify({ relativePath: 'assets/logo.png', isDirectory: false, isText: false }) },
    );
    fireEvent.drop(footer, dropEvent as unknown as React.DragEvent);

    expect(handler).toHaveBeenCalledWith(
      expect.objectContaining({ detail: { text: './assets/logo.png ' } }),
    );
    window.removeEventListener('phoenix:insert-draft', handler);
  });

  it('inserts ./path for paths with whitespace on custom-type drop', () => {
    const handler = vi.fn();
    window.addEventListener('phoenix:insert-draft', handler);
    renderForDnd();

    const footer = document.getElementById('input-area')!;
    const dropEvent = makeDropEvent(
      [FILE_TREE_DRAG_TYPE],
      { [FILE_TREE_DRAG_TYPE]: JSON.stringify({ relativePath: 'src/test data/input.ts', isDirectory: false, isText: true }) },
    );
    fireEvent.drop(footer, dropEvent as unknown as React.DragEvent);

    expect(handler).toHaveBeenCalledWith(
      expect.objectContaining({ detail: { text: './src/test data/input.ts ' } }),
    );
    window.removeEventListener('phoenix:insert-draft', handler);
  });

  it('activates drop highlight for custom-type drag', () => {
    renderForDnd();
    const footer = document.getElementById('input-area')!;

    fireEvent.dragEnter(footer, makeDropEvent([FILE_TREE_DRAG_TYPE]) as unknown as React.DragEvent);
    expect(footer.className).toContain('input-area--drag-over');
  });

  it('ignores drops without Files or custom type', () => {
    const handler = vi.fn();
    window.addEventListener('phoenix:insert-draft', handler);
    renderForDnd();

    const footer = document.getElementById('input-area')!;
    const dropEvent = makeDropEvent(['text/plain'], { 'text/plain': 'hello' });
    fireEvent.drop(footer, dropEvent as unknown as React.DragEvent);

    expect(handler).not.toHaveBeenCalled();
    window.removeEventListener('phoenix:insert-draft', handler);
  });
});
