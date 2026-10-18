import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { Conversation } from '../api';
import { createInitialAtom } from '../conversation/atom';
import type { InlineStreamState } from '../hooks/useConversationInlineStream';
import { copyToClipboard } from '../utils/clipboard';
import { SubAgentTaskDetails } from './SubAgentTaskDetails';

vi.mock('../utils/clipboard', () => ({ copyToClipboard: vi.fn().mockResolvedValue(true) }));

const child: Conversation = {
  id: 'child', slug: 'child-slug', model: 'unknown/custom-child-model', effort: 'high',
  cwd: '/repo', created_at: '', updated_at: '', message_count: 0,
  browser_session_active: false, terminal_uses_tmux: false, work_scope_key: 'child-scope',
};

function ready(conversation: Conversation | null = child): InlineStreamState {
  return { type: 'ready', atom: { ...createInitialAtom(), conversation }, error: null };
}

const task = `  Keep leading spaces\n\n<script>alert("task")</script>\n<img src=x onerror=alert(1)>\n\t${'long-unbroken-task'.repeat(180)}\nTrailing spaces  `;

describe('SubAgentTaskDetails', () => {
  beforeEach(() => vi.clearAllMocks());

  it('discloses and copies the exact long multiline task as plaintext, not HTML or Markdown', async () => {
    const { container } = render(<SubAgentTaskDetails task={task} inline={ready()} />);
    const details = container.querySelector('details')!;
    expect(details).not.toHaveAttribute('open');
    expect(details.querySelector('summary')).toHaveTextContent('Delegated task');
    fireEvent.click(screen.getByText('Delegated task'));
    expect(details).toHaveAttribute('open');
    expect(container.querySelector('.subagent-delegated-task-text')?.textContent).toBe(task);
    expect(container.querySelector('script, img')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Copy delegated task' }));
    await waitFor(() => expect(copyToClipboard).toHaveBeenCalledExactlyOnceWith(task));
    expect(details).toHaveAttribute('open');
  });

  it('shows the actual child model verbatim even if it is not a known catalog ID', () => {
    render(<SubAgentTaskDetails task="task" inline={ready()} />);
    expect(screen.getByText('Model: unknown/custom-child-model')).toBeInTheDocument();
    expect(screen.getByText('Effort: high')).toBeInTheDocument();
    expect(screen.queryByText(/inherit/i)).not.toBeInTheDocument();
  });

  it.each<{ effort: Conversation['effort']; expected: string }>([
    { effort: null, expected: 'model default' },
    { effort: undefined, expected: 'not reported' },
    { effort: 'low', expected: 'low' },
  ])('renders child effort $effort as $expected without inference', ({ effort, expected }) => {
    const conversation = { ...child };
    if (effort === undefined) delete conversation.effort;
    else conversation.effort = effort;
    render(<SubAgentTaskDetails task="task" inline={ready(conversation)} />);
    expect(screen.getByText(`Effort: ${expected}`)).toBeInTheDocument();
  });

  it.each(['idle', 'connecting', 'ready', 'error'] as const)('retains the parent task with explicit missing child metadata in %s state', (type) => {
    const atom = createInitialAtom();
    const inline: InlineStreamState = type === 'error'
      ? { type, atom, error: 'Child unavailable' }
      : { type, atom, error: null };
    const { container } = render(<SubAgentTaskDetails task={task} inline={inline} />);
    expect(container.querySelector('.subagent-delegated-task-text')?.textContent).toBe(task);
    expect(screen.getByText(`Model: ${type === 'idle' || type === 'connecting' ? 'Loading…' : 'Unavailable'}`)).toBeInTheDocument();
    expect(screen.getByText('Effort: not reported')).toBeInTheDocument();
    expect(screen.queryByText(/inherit/i)).not.toBeInTheDocument();
  });

  it('keeps known child metadata when the stream later errors', () => {
    render(<SubAgentTaskDetails task="task" inline={{ ...ready(), type: 'error', error: 'Disconnected' }} />);
    expect(screen.getByText('Model: unknown/custom-child-model')).toBeInTheDocument();
    expect(screen.getByText('Effort: high')).toBeInTheDocument();
  });
});
