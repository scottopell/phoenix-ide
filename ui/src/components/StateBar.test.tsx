import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { StateBar } from './StateBar';
import { api, type Conversation, type ConversationState, type ModelInfo, type PrStatusResponse } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      getPrStatus: vi.fn(),
      createPrAutoFixContext: vi.fn(),
      getConversationUsage: vi.fn(),
    },
  };
});

beforeAll(() => {
  if (!window.matchMedia) {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    });
  }
});

beforeEach(() => {
  vi.clearAllMocks();
  (api.getPrStatus as ReturnType<typeof vi.fn>).mockResolvedValue({ found: false });
  (api.createPrAutoFixContext as ReturnType<typeof vi.fn>).mockResolvedValue({
    artifact_path: '.phoenix/pr-context/pr-12.json',
    pr_number: 12,
    message: 'Address `.phoenix/pr-context/pr-12.json`',
  });
  (api.getConversationUsage as ReturnType<typeof vi.fn>).mockResolvedValue({
    own: { input_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0, output_tokens: 0, turns: 0 },
    total: { input_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0, output_tokens: 0, turns: 0 },
  });
});

function makeConversation(overrides: Partial<Conversation> = {}): Conversation {
  return {
    id: 'conv-1',
    slug: 'track-pr-status',
    model: 'claude-sonnet-4-6',
    cwd: '/repo/.phoenix/worktrees/conv-1',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    message_count: 3,
    state: { type: 'idle' },
    branch_name: 'task-123-pr-status',
    base_branch: 'main',
    worktree_path: '/repo/.phoenix/worktrees/conv-1',
    task_title: 'Track PR status',
    conv_mode_label: 'Work',
    browser_session_active: false,
    terminal_uses_tmux: false,
    ...overrides,
  };
}

function renderStateBar({
  conversation = makeConversation(),
  convState = { type: 'idle' } as const,
  contextWindowUsed = 0,
  modelContextWindow = 200_000,
  continuation,
  onSendMessage,
  connectionState = 'connected',
  connectionAttempt = 0,
  phaseStateUpdatedAt,
  lastSseEventAt,
  firstByteRequestId,
}: {
  conversation?: Conversation;
  convState?: ComponentProps<typeof StateBar>['convState'];
  contextWindowUsed?: number;
  modelContextWindow?: number;
  continuation?: ComponentProps<typeof StateBar>['continuation'];
  onSendMessage?: ComponentProps<typeof StateBar>['onSendMessage'];
  connectionState?: ComponentProps<typeof StateBar>['connectionState'];
  connectionAttempt?: number;
  phaseStateUpdatedAt?: number | null;
  lastSseEventAt?: number;
  firstByteRequestId?: string | null;
} = {}) {
  const props: ComponentProps<typeof StateBar> = {
    conversation,
    convState,
    connectionState,
    connectionAttempt,
    nextRetryIn: null,
    contextWindowUsed,
    modelContextWindow,
  };
  if (continuation) {
    props.continuation = continuation;
  }
  if (onSendMessage) {
    props.onSendMessage = onSendMessage;
  }
  if (phaseStateUpdatedAt !== undefined) {
    props.phaseStateUpdatedAt = phaseStateUpdatedAt;
  }
  if (lastSseEventAt !== undefined) {
    props.lastSseEventAt = lastSseEventAt;
  }
  if (firstByteRequestId !== undefined) {
    props.firstByteRequestId = firstByteRequestId;
  }
  return render(
    <MemoryRouter>
      <StateBar {...props} />
    </MemoryRouter>,
  );
}

function mockPrStatus(status: PrStatusResponse) {
  (api.getPrStatus as ReturnType<typeof vi.fn>).mockResolvedValue(status);
}

describe('StateBar PR badge', () => {
  it.each([
    [{ display_state: 'merged', check_state: 'passing' }, /#12 merged/i, 'pr-badge--merged'],
    [{ display_state: 'open', check_state: 'passing' }, /#12 checks ✓/i, 'pr-badge--passing'],
    [{ display_state: 'open', check_state: 'pending' }, /#12 checks \.\.\./i, 'pr-badge--pending'],
    [{ display_state: 'draft', check_state: 'pending' }, /#12 draft/i, 'pr-badge--pending'],
    [{ display_state: 'open', check_state: 'failing' }, /#12 checks ✗/i, 'pr-badge--failing'],
    [{ display_state: 'closed', check_state: 'unknown' }, /#12 closed/i, 'pr-badge--failing'],
    [{ display_state: 'open', check_state: 'unknown' }, /^#12$/i, 'pr-badge--unknown'],
  ] as const)('renders %s as %s', async (state, label, className) => {
    mockPrStatus({
      found: true,
      number: 12,
      title: 'Add PR tracking',
      url: 'https://github.com/scottopell/phoenix-ide/pull/12',
      state: state.display_state.toUpperCase(),
      draft: state.display_state === 'draft',
      base: 'main',
      head: 'task-123-pr-status',
      display_state: state.display_state,
      check_state: state.check_state,
    } as PrStatusResponse);

    renderStateBar();

    const badge = await screen.findByRole('button', { name: label });
    expect(badge).toHaveClass('pr-badge', className);
    expect(badge.getAttribute('title')).toContain('Add PR tracking');
  });

  it('renders no badge when gh finds no PR', async () => {
    mockPrStatus({ found: false });

    renderStateBar();

    await waitFor(() => expect(api.getPrStatus).toHaveBeenCalledWith('conv-1'));
    expect(screen.queryByText(/^#\d+/)).not.toBeInTheDocument();
  });

  it('renders a compact gh authentication hint when status is unavailable', async () => {
    mockPrStatus({ found: false, unavailable_reason: 'not_authenticated' });

    renderStateBar();

    expect(await screen.findByText('gh auth')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^#\d+/ })).not.toBeInTheDocument();
  });

  it('does not fetch PR status for conversations without a branch', async () => {
    renderStateBar({ conversation: makeConversation({ branch_name: null, base_branch: null }) });

    await waitFor(() => expect(screen.getByText('track-pr-status')).toBeInTheDocument());
    expect(api.getPrStatus).not.toHaveBeenCalled();
  });

  it('opens CI popover and sends auto-fix message from captured context', async () => {
    const onSendMessage = vi.fn();
    mockPrStatus({
      found: true,
      number: 12,
      title: 'Fix CI',
      url: 'https://github.com/scottopell/phoenix-ide/pull/12',
      state: 'OPEN',
      draft: false,
      base: 'main',
      head: 'task-123-pr-status',
      display_state: 'open',
      check_state: 'failing',
      check_summary: {
        passing: 1,
        pending: 0,
        failing: 1,
        skipped: 0,
        unknown: 0,
        failing_names: ['test'],
        pending_names: [],
      },
      feedback_summary: { total: 2, unresolved: 2, items: [], coverage: [] },
    });

    renderStateBar({ onSendMessage });
    fireEvent.click(await screen.findByRole('button', { name: /#12 checks ✗/i }));
    expect(await screen.findByRole('dialog', { name: /PR CI monitoring/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Auto-fix CI & address comments/i }));

    await waitFor(() => expect(api.createPrAutoFixContext).toHaveBeenCalledWith('conv-1'));
    await waitFor(() => expect(onSendMessage).toHaveBeenCalledWith('Address `.phoenix/pr-context/pr-12.json`'));
  });
});

describe('StateBar model picker enablement (task 02713)', () => {
  const models: ModelInfo[] = [
    { id: 'claude-sonnet-4-6', provider: 'anthropic', description: '', context_window: 200_000, recommended: true },
    { id: 'claude-opus-4-7', provider: 'anthropic', description: '', context_window: 200_000, recommended: true },
  ];

  function renderWithState(convState: ConversationState) {
    return render(
      <MemoryRouter>
        <StateBar
          conversation={makeConversation()}
          convState={convState}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          availableModels={models}
          onUpgradeModel={vi.fn()}
        />
      </MemoryRouter>,
    );
  }

  it('renders the model picker as an interactive button in error state', () => {
    const { container } = renderWithState({ type: 'error', message: 'overloaded' });
    expect(container.querySelector('button.conv-model--button')).not.toBeNull();
  });

  it('renders the model picker as an interactive button when idle', () => {
    const { container } = renderWithState({ type: 'idle' });
    expect(container.querySelector('button.conv-model--button')).not.toBeNull();
  });

  it('disables the model picker (read-only span) while an LLM request is in flight', () => {
    const { container } = renderWithState({ type: 'llm_requesting', attempt: 1 });
    expect(container.querySelector('button.conv-model--button')).toBeNull();
    expect(container.querySelector('span.conv-model')).not.toBeNull();
  });

  it('disables the model picker while a tool is executing', () => {
    const { container } = renderWithState({
      type: 'tool_executing',
      current_tool: { id: 't', name: 'bash', input: {} },
      remaining_tools: [],
    });
    expect(container.querySelector('button.conv-model--button')).toBeNull();
  });
});

describe('StateBar manual continuation action', () => {
  it('offers manual continuation below the warning threshold while idle', async () => {
    const onTriggerContinuation = vi.fn();
    renderStateBar({
      contextWindowUsed: 100_000,
      modelContextWindow: 1_000_000,
      continuation: { phase: 'idle', onTrigger: onTriggerContinuation },
    });

    fireEvent.click(screen.getByText('100k'));

    const action = await screen.findByRole('button', { name: /end & summarize now/i });
    expect(screen.getByText(/continue in a new conversation/i)).toBeInTheDocument();

    fireEvent.click(action);

    expect(onTriggerContinuation).toHaveBeenCalledTimes(1);
  });

  it('does not offer manual continuation while the conversation is busy', () => {
    renderStateBar({
      convState: { type: 'awaiting_llm' },
      contextWindowUsed: 100_000,
      modelContextWindow: 1_000_000,
      continuation: { phase: 'unavailable' },
    });

    fireEvent.click(screen.getByText('100k'));

    expect(screen.queryByRole('button', { name: /end & summarize now/i })).not.toBeInTheDocument();
  });
});

// Working-phase indicators (REQ-WPV-001 / 003 / 004 / 005). The reducer
// math driving these is unit-tested above; these tests pin the StateBar's
// composition function — which combinations of (connectionState, convState,
// phaseStateUpdatedAt, lastSseEventAt) produce which text + dot class.
describe('StateBar working-phase indicators', () => {
  const T_NOW = 1_700_000_000_000; // any stable wall-clock anchor

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T_NOW);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('renders the live elapsed counter for llm_requesting (REQ-WPV-001/003)', () => {
    // 7 seconds into the llm_requesting phase.
    renderStateBar({
      convState: { type: 'llm_requesting', attempt: 1 },
      phaseStateUpdatedAt: T_NOW - 7_000,
      lastSseEventAt: T_NOW - 1_000,
    });
    expect(screen.getByText(/thinking.*7s/i)).toBeInTheDocument();
    const dot = document.querySelector('.dot');
    expect(dot?.className).toMatch(/working/);
  });

  it('renders the live counter for non-llm working phases too (REQ-WPV-001)', () => {
    // tool_executing is the same generalized path now.
    renderStateBar({
      convState: {
        type: 'tool_executing',
        current_tool: { id: 'bash-1', name: 'bash', input: {} },
        remaining_tools: [],
      },
      phaseStateUpdatedAt: T_NOW - 12_000,
      lastSseEventAt: T_NOW - 1_000,
    });
    // The state-description helper produces "running bash" or similar;
    // we just assert the elapsed suffix is present.
    expect(screen.getByText(/\b12s\b/)).toBeInTheDocument();
  });

  it('overrides working text with "no signal from server" when watchdog stale (REQ-WPV-004)', () => {
    // 40s since the last observed SSE event > 35s threshold.
    renderStateBar({
      convState: { type: 'llm_requesting', attempt: 1 },
      phaseStateUpdatedAt: T_NOW - 50_000,
      lastSseEventAt: T_NOW - 40_000,
    });
    expect(screen.getByText(/no signal from server for 40s/i)).toBeInTheDocument();
    const dot = document.querySelector('.dot');
    expect(dot?.className).toMatch(/degraded/);
  });

  it('does NOT trip the watchdog when not in a working phase', () => {
    // Even after a long silence, idle conversations are not "stuck."
    renderStateBar({
      convState: { type: 'idle' },
      phaseStateUpdatedAt: T_NOW - 60_000,
      lastSseEventAt: T_NOW - 60_000,
    });
    expect(screen.queryByText(/no signal from server/i)).not.toBeInTheDocument();
    expect(screen.getByText(/ready/i)).toBeInTheDocument();
  });

  it('does NOT trip the watchdog when the connection has already degraded', () => {
    // Reconnecting / offline carry their own messaging; the watchdog
    // should disarm to avoid duplicate "no signal" + "reconnecting" text.
    renderStateBar({
      convState: { type: 'llm_requesting', attempt: 1 },
      connectionState: 'reconnecting',
      connectionAttempt: 2,
      phaseStateUpdatedAt: T_NOW - 10_000,
      lastSseEventAt: T_NOW - 60_000, // > 35s but connection isn't healthy
    });
    expect(screen.queryByText(/no signal from server/i)).not.toBeInTheDocument();
  });

  it('shows BOTH connection + last-known activity during reconnecting (REQ-WPV-005)', () => {
    // The capture happens on the connected→reconnecting transition;
    // a single render starting in reconnecting won't have the snapshot,
    // so this test re-renders the component to simulate the edge.
    const { rerender } = render(
      <MemoryRouter>
        <StateBar
          conversation={makeConversation()}
          convState={{ type: 'llm_requesting', attempt: 1 }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          phaseStateUpdatedAt={T_NOW - 12_000}
          lastSseEventAt={T_NOW - 1_000}
        />
      </MemoryRouter>,
    );
    // First render — connected, working: shows "thinking ... 12s".
    expect(screen.getByText(/thinking.*12s/i)).toBeInTheDocument();
    // Connection drops mid-working — capture snapshot, freeze elapsed.
    rerender(
      <MemoryRouter>
        <StateBar
          conversation={makeConversation()}
          convState={{ type: 'llm_requesting', attempt: 1 }}
          connectionState="reconnecting"
          connectionAttempt={2}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          phaseStateUpdatedAt={T_NOW - 12_000}
          lastSseEventAt={T_NOW - 1_000}
        />
      </MemoryRouter>,
    );
    // Now we should see "reconnecting (2) — last: thinking ... 12s".
    expect(screen.getByText(/reconnecting \(2\).*last.*thinking.*12s/i)).toBeInTheDocument();
    const dot = document.querySelector('.dot');
    expect(dot?.className).toMatch(/reconnecting/);
  });

  it('switches to "streaming" (no counter) once first byte arrives (REQ-WPV-007)', () => {
    renderStateBar({
      convState: { type: 'llm_requesting', attempt: 1 },
      phaseStateUpdatedAt: T_NOW - 4_000,
      lastSseEventAt: T_NOW - 1_000,
      firstByteRequestId: 'req-abc',
    });
    expect(screen.getByText(/^streaming$/i)).toBeInTheDocument();
    // The pre-first-byte "thinking Ns" form must NOT be present once
    // the first byte has arrived.
    expect(screen.queryByText(/thinking.*4s/i)).not.toBeInTheDocument();
    const dot = document.querySelector('.dot');
    expect(dot?.className).toMatch(/working/);
  });

  it('keeps the elapsed counter for non-llm working phases even with firstByteRequestId set', () => {
    // First byte applies only to llm_requesting-family states; a
    // tool_executing phase keeps its elapsed counter even if a
    // first-byte signal from a prior LLM request is still on the atom.
    renderStateBar({
      convState: { type: 'tool_executing', current_tool: { id: 'bash-1', name: 'bash', input: {} }, remaining_tools: [] },
      phaseStateUpdatedAt: T_NOW - 9_000,
      lastSseEventAt: T_NOW - 1_000,
      firstByteRequestId: 'req-prior',
    });
    expect(screen.queryByText(/^streaming$/i)).not.toBeInTheDocument();
    expect(screen.getByText(/\b9s\b/)).toBeInTheDocument();
  });

  it('shows BOTH offline chip + last-known activity during offline (REQ-WPV-005)', () => {
    const { rerender } = render(
      <MemoryRouter>
        <StateBar
          conversation={makeConversation()}
          convState={{ type: 'tool_executing', current_tool: { id: 'bash-1', name: 'bash', input: {} }, remaining_tools: [] }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          phaseStateUpdatedAt={T_NOW - 8_000}
          lastSseEventAt={T_NOW - 1_000}
        />
      </MemoryRouter>,
    );
    rerender(
      <MemoryRouter>
        <StateBar
          conversation={makeConversation()}
          convState={{ type: 'tool_executing', current_tool: { id: 'bash-1', name: 'bash', input: {} }, remaining_tools: [] }}
          connectionState="offline"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          phaseStateUpdatedAt={T_NOW - 8_000}
          lastSseEventAt={T_NOW - 1_000}
        />
      </MemoryRouter>,
    );
    expect(screen.getByText(/^offline.*last.*8s/i)).toBeInTheDocument();
    const dot = document.querySelector('.dot');
    expect(dot?.className).toMatch(/offline/);
  });
});
