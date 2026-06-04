import { describe, it, expect } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import { MemoryRouter, useNavigate } from 'react-router-dom';
import { SubAgentViewerProvider, useSubAgentViewer } from './SubAgentViewerContext';

function Consumer() {
  const viewer = useSubAgentViewer();
  const navigate = useNavigate();
  return (
    <div>
      <span data-testid="opened">{viewer?.opened?.agentId ?? 'none'}</span>
      <button
        onClick={() =>
          viewer?.open({ agentId: 'agent-1', task: 'do a thing', running: false, resultText: '' })
        }
      >
        open
      </button>
      <button onClick={() => viewer?.close()}>close</button>
      <button onClick={() => navigate('/c/other')}>navigate</button>
    </div>
  );
}

function renderConsumer() {
  return render(
    <MemoryRouter initialEntries={['/c/parent']}>
      <SubAgentViewerProvider>
        <Consumer />
      </SubAgentViewerProvider>
    </MemoryRouter>,
  );
}

describe('SubAgentViewerContext', () => {
  it('opens and closes the viewer', () => {
    renderConsumer();
    expect(screen.getByTestId('opened').textContent).toBe('none');

    act(() => screen.getByText('open').click());
    expect(screen.getByTestId('opened').textContent).toBe('agent-1');

    act(() => screen.getByText('close').click());
    expect(screen.getByTestId('opened').textContent).toBe('none');
  });

  it('closes the viewer when the route changes (the opened sub-agent belongs to the parent you left)', () => {
    renderConsumer();
    act(() => screen.getByText('open').click());
    expect(screen.getByTestId('opened').textContent).toBe('agent-1');

    act(() => screen.getByText('navigate').click());
    expect(screen.getByTestId('opened').textContent).toBe('none');
  });

  it('useSubAgentViewer returns null with no provider so callers can fall back to navigation', () => {
    render(
      <MemoryRouter>
        <Consumer />
      </MemoryRouter>,
    );
    // No provider → hook is null → nothing opens, render is stable.
    expect(screen.getByTestId('opened').textContent).toBe('none');
    act(() => screen.getByText('open').click());
    expect(screen.getByTestId('opened').textContent).toBe('none');
  });
});
