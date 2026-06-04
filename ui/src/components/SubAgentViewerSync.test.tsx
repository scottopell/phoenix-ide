// Regression test for the docked sub-agent viewer staying in sync with the
// parent card's live state (Codex review on PR #219).
//
// Opening the panel while a sub-agent is still running snapshots its
// running/result into the viewer context. When the sub-agent later completes,
// the parent SubAgentActivityCard re-renders with the final outcome and must
// push it into the open viewer record — otherwise the panel keeps streaming a
// finished agent (live=true) and never shows the final outcome until reopen.

import { describe, it, expect } from 'vitest';
import { useState } from 'react';
import { render, screen, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { SubAgentViewerProvider, useSubAgentViewer } from '../contexts/SubAgentViewerContext';
import { SubAgentActivityCard } from './MessageComponents';
import type { SubAgentOutcome } from '../api';

function Harness() {
  const viewer = useSubAgentViewer();
  const [outcome, setOutcome] = useState<SubAgentOutcome | null>(null);
  return (
    <>
      <button onClick={() => viewer?.open({ agentId: 'agent-x', task: 'do x', running: true, resultText: '' })}>
        open
      </button>
      <button onClick={() => setOutcome({ type: 'success', result: 'final result' })}>finish</button>
      <span data-testid="running">{String(viewer?.opened?.running ?? 'closed')}</span>
      <span data-testid="result">{viewer?.opened?.resultText ?? 'closed'}</span>
      <SubAgentActivityCard agentId="agent-x" task="do x" outcome={outcome} />
    </>
  );
}

describe('docked sub-agent viewer sync', () => {
  it('updates the open viewer record when the sub-agent completes', () => {
    render(
      <MemoryRouter>
        <SubAgentViewerProvider>
          <Harness />
        </SubAgentViewerProvider>
      </MemoryRouter>,
    );

    // Open the panel while the sub-agent is still running.
    act(() => screen.getByText('open').click());
    expect(screen.getByTestId('running').textContent).toBe('true');
    expect(screen.getByTestId('result').textContent).toBe('');

    // Sub-agent finishes: the card re-renders with the final outcome and the
    // open viewer record follows — no longer running, final outcome present.
    act(() => screen.getByText('finish').click());
    expect(screen.getByTestId('running').textContent).toBe('false');
    expect(screen.getByTestId('result').textContent).toBe('final result');
  });

  it('does not touch the viewer record for a non-open agent', () => {
    function OtherHarness() {
      const viewer = useSubAgentViewer();
      const [outcome, setOutcome] = useState<SubAgentOutcome | null>(null);
      return (
        <>
          <button onClick={() => viewer?.open({ agentId: 'agent-open', task: 't', running: true, resultText: '' })}>
            open
          </button>
          <button onClick={() => setOutcome({ type: 'success', result: 'other result' })}>finish-other</button>
          <span data-testid="agent">{viewer?.opened?.agentId ?? 'closed'}</span>
          <span data-testid="running">{String(viewer?.opened?.running ?? 'closed')}</span>
          {/* A different agent's card completing must not mutate the open record. */}
          <SubAgentActivityCard agentId="agent-other" task="t" outcome={outcome} />
        </>
      );
    }
    render(
      <MemoryRouter>
        <SubAgentViewerProvider>
          <OtherHarness />
        </SubAgentViewerProvider>
      </MemoryRouter>,
    );

    act(() => screen.getByText('open').click());
    act(() => screen.getByText('finish-other').click());
    expect(screen.getByTestId('agent').textContent).toBe('agent-open');
    expect(screen.getByTestId('running').textContent).toBe('true');
  });
});
