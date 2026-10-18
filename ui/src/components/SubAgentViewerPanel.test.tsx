// The docked sub-agent viewer derives live status from the sub-agent's OWN
// stream, not from the parent spawn card (Codex review on PR #219). The card
// lives in a virtualized list and unmounts when scrolled off-screen, so any
// card-sourced status would go stale; the always-mounted panel owning the
// stream keeps "live" correct regardless.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { Conversation, ConversationState } from '../api';
import { copyToClipboard } from '../utils/clipboard';

vi.mock('../hooks/useConversationInlineStream', () => ({
  useConversationInlineStream: vi.fn(),
}));
vi.mock('../utils/clipboard', () => ({ copyToClipboard: vi.fn().mockResolvedValue(true) }));

import { useConversationInlineStream } from '../hooks/useConversationInlineStream';
import { createInitialAtom } from '../conversation/atom';
import { SubAgentViewerPanel } from './SubAgentViewerPanel';

const mockStream = vi.mocked(useConversationInlineStream);

const child: Conversation = {
  id: 'agent-x', slug: 'child-slug', model: 'custom-child-model', effort: null,
  cwd: '/repo', created_at: '', updated_at: '', message_count: 0,
  browser_session_active: false, terminal_uses_tmux: false, work_scope_key: 'child-scope',
};

function readyWithPhase(phase: ConversationState, conversation: Conversation | null = null) {
  return { type: 'ready' as const, atom: { ...createInitialAtom(), phase, conversation }, error: null };
}

function renderPanel(task = 'do x') {
  return render(
    <MemoryRouter>
      <SubAgentViewerPanel opened={{ agentId: 'agent-x', task }} onClose={() => {}} />
    </MemoryRouter>,
  );
}

describe('SubAgentViewerPanel live status', () => {
  beforeEach(() => mockStream.mockReset());

  it('shows "live" while the sub-agent is still working', () => {
    mockStream.mockReturnValue(readyWithPhase({ type: 'awaiting_llm' }));
    const { container } = renderPanel();
    expect(container.querySelector('.subagent-viewer-subtitle')?.textContent).toContain('live');
  });

  it('drops "live" once the sub-agent reaches a terminal state', () => {
    mockStream.mockReturnValue(readyWithPhase({ type: 'idle' }));
    const { container } = renderPanel();
    const subtitle = container.querySelector('.subagent-viewer-subtitle')?.textContent ?? '';
    expect(subtitle).toContain('read-only');
    expect(subtitle).not.toContain('live');
  });

  it('renders one task block with live child metadata and updates it independently of the parent card', () => {
    mockStream.mockReturnValue(readyWithPhase({ type: 'awaiting_llm' }, child));
    const { container, rerender } = renderPanel();
    expect(container.querySelectorAll('.subagent-task-details')).toHaveLength(1);
    expect(screen.getByText('Model: custom-child-model')).toBeInTheDocument();
    expect(screen.getByText('Effort: model default')).toBeInTheDocument();
    mockStream.mockReturnValue(readyWithPhase({ type: 'idle' }, { ...child, model: 'changed-child-model', effort: 'low' }));
    rerender(<MemoryRouter><SubAgentViewerPanel opened={{ agentId: 'agent-x', task: 'do x' }} onClose={() => {}} /></MemoryRouter>);
    expect(screen.getByText('Model: changed-child-model')).toBeInTheDocument();
    expect(screen.getByText('Effort: low')).toBeInTheDocument();
    expect(container.querySelector('.subagent-viewer-subtitle')).not.toHaveTextContent('live');
  });

  it.each(['connecting', 'error'] as const)('allows exact parent task disclosure and copy while the child is %s', async (type) => {
    const task = '  Full parent delegation\n\n<img src=x onerror=alert(1)>\n' + 'long task '.repeat(200);
    const atom = createInitialAtom();
    mockStream.mockReturnValue(type === 'error'
      ? { type, atom, error: 'Cannot load child' }
      : { type, atom, error: null });
    vi.mocked(copyToClipboard).mockClear();
    const { container } = renderPanel(task);
    expect(container.querySelectorAll('.subagent-task-details')).toHaveLength(1);
    expect(screen.getByText(`Model: ${type === 'connecting' ? 'Loading…' : 'Unavailable'}`)).toBeInTheDocument();
    expect(screen.getByText('Effort: not reported')).toBeInTheDocument();
    const details = container.querySelector('details')!;
    expect(details).not.toHaveAttribute('open');
    fireEvent.click(screen.getByText('Delegated task'));
    expect(details).toHaveAttribute('open');
    expect(container.querySelector('.subagent-delegated-task-text')?.textContent).toBe(task);
    expect(container.querySelector('img')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Copy delegated task' }));
    await waitFor(() => expect(copyToClipboard).toHaveBeenCalledExactlyOnceWith(task));
    expect(container.querySelector('.subagent-viewer-subtitle')).toHaveTextContent('read-only');
  });

  it('streams live unconditionally so a just-spawned (momentarily Idle) sub-agent is followed', () => {
    mockStream.mockReturnValue(readyWithPhase({ type: 'idle' }));
    renderPanel();
    expect(mockStream).toHaveBeenCalledWith('agent-x', true, true);
  });
});
