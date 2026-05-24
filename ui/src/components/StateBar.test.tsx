import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
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
}: {
  conversation?: Conversation;
  convState?: ComponentProps<typeof StateBar>['convState'];
  contextWindowUsed?: number;
  modelContextWindow?: number;
  continuation?: ComponentProps<typeof StateBar>['continuation'];
  onSendMessage?: ComponentProps<typeof StateBar>['onSendMessage'];
} = {}) {
  const props: ComponentProps<typeof StateBar> = {
    conversation,
    convState,
    connectionState: 'connected',
    connectionAttempt: 0,
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
      feedback_summary: { total: 2, unresolved: 2, items: [], coverage: [], limitations: [] },
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
