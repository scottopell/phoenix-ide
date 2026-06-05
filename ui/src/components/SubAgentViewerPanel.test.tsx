// The docked sub-agent viewer derives live status from the sub-agent's OWN
// stream, not from the parent spawn card (Codex review on PR #219). The card
// lives in a virtualized list and unmounts when scrolled off-screen, so any
// card-sourced status would go stale; the always-mounted panel owning the
// stream keeps "live" correct regardless.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { ConversationState } from '../api';

vi.mock('../hooks/useConversationInlineStream', () => ({
  useConversationInlineStream: vi.fn(),
}));

import { useConversationInlineStream } from '../hooks/useConversationInlineStream';
import { createInitialAtom } from '../conversation/atom';
import { SubAgentViewerPanel } from './SubAgentViewerPanel';

const mockStream = vi.mocked(useConversationInlineStream);

function readyWithPhase(phase: ConversationState) {
  return { type: 'ready' as const, atom: { ...createInitialAtom(), phase }, error: null };
}

function renderPanel() {
  return render(
    <MemoryRouter>
      <SubAgentViewerPanel opened={{ agentId: 'agent-x', task: 'do x' }} onClose={() => {}} />
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

  it('streams live unconditionally so a just-spawned (momentarily Idle) sub-agent is followed', () => {
    mockStream.mockReturnValue(readyWithPhase({ type: 'idle' }));
    renderPanel();
    expect(mockStream).toHaveBeenCalledWith('agent-x', true, true);
  });
});
