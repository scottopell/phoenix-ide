import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { StateBar } from './StateBar';
import { api, type AssociatedPrStatusEnvelope, type Conversation, type ConversationState, type ModelInfo, type PrStatusResponse } from '../api';

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
  setMobileViewport(false);
  (api.getPrStatus as ReturnType<typeof vi.fn>).mockResolvedValue(mockPrStatus({ found: false }));
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
    model: 'claude-sonnet-5',
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
    work_scope_key: 'worktree:/repo/.phoenix/worktrees/conv-1',
    ...overrides,
  };
}

function renderStateBar({
  conversation = makeConversation(),
  convState = { type: 'idle' } as const,
  contextWindowUsed = 0,
  modelContextWindow = 200_000,
  continuation,
  prStatus,
  connectionState = 'connected',
  connectionAttempt = 0,
  phaseStateUpdatedAt,
  lastSseEventAt,
  firstByteRequestId,
  turnRetryContext,
  onOpenFiles,
  availableModels,
  onUpgradeModel,
}: {
  conversation?: Conversation;
  convState?: ComponentProps<typeof StateBar>['convState'];
  contextWindowUsed?: number;
  modelContextWindow?: number;
  continuation?: ComponentProps<typeof StateBar>['continuation'];
  prStatus?: PrStatusResponse;
  connectionState?: ComponentProps<typeof StateBar>['connectionState'];
  connectionAttempt?: number;
  phaseStateUpdatedAt?: number | null;
  lastSseEventAt?: number;
  firstByteRequestId?: string | null;
  turnRetryContext?: ComponentProps<typeof StateBar>['turnRetryContext'];
  onOpenFiles?: ComponentProps<typeof StateBar>['onOpenFiles'];
  availableModels?: ComponentProps<typeof StateBar>['availableModels'];
  onUpgradeModel?: ComponentProps<typeof StateBar>['onUpgradeModel'];
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
  if (onOpenFiles !== undefined) {
    props.onOpenFiles = onOpenFiles;
  }
  if (availableModels !== undefined) {
    props.availableModels = availableModels;
  }
  if (onUpgradeModel !== undefined) {
    props.onUpgradeModel = onUpgradeModel;
  }
  if (continuation) {
    props.continuation = continuation;
  }
  if (prStatus) {
    props.prStatusHandle = makePrStatusHandle(prStatus, prStatus.selection ?? null);
  }
  if (phaseStateUpdatedAt !== undefined) {
    props.phaseStateUpdatedAt = phaseStateUpdatedAt;
  }
  if (lastSseEventAt !== undefined) {
    props.lastSseEventAtRef = { current: lastSseEventAt };
  }
  if (firstByteRequestId !== undefined) {
    props.firstByteRequestId = firstByteRequestId;
  }
  if (turnRetryContext !== undefined) {
    props.turnRetryContext = turnRetryContext;
  }
  return render(
    <MemoryRouter>
      <StateBar {...props} />
    </MemoryRouter>,
  );
}

function setMobileViewport(matches = true) {
  const listeners = new Set<(event: MediaQueryListEvent) => void>();
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: query === '(max-width: 768px)' ? matches : false,
      media: query,
      onchange: null,
      addEventListener: vi.fn((_event: string, cb: (event: MediaQueryListEvent) => void) => listeners.add(cb)),
      removeEventListener: vi.fn((_event: string, cb: (event: MediaQueryListEvent) => void) => listeners.delete(cb)),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
}

const pickerModels: ModelInfo[] = [
  { id: 'claude-sonnet-5', provider: 'anthropic', description: '', context_window: 1_000_000, recommended: true },
  { id: 'claude-sonnet-4-6', provider: 'anthropic', description: '', context_window: 1_000_000, recommended: false },
  { id: 'claude-opus-4-7', provider: 'anthropic', description: '', context_window: 1_000_000, recommended: true },
];

function makeSelection(overrides: Partial<AssociatedPrStatusEnvelope> = {}): AssociatedPrStatusEnvelope {
  return {
    associated_prs: [
      {
        repo_owner: 'o',
        repo_name: 'r',
        pr_number: 12,
        title: 'Fix CI',
        url: 'https://github.com/scottopell/phoenix-ide/pull/12',
        state: 'OPEN',
        draft: false,
        display_state: 'open',
        base: 'main',
        head: 'task-123-pr-status',
        feedback_status: 'open',
      },
    ],
    active_pr: {
      pr: { repo_owner: 'o', repo_name: 'r', pr_number: 12 },
      provenance: 'inferred',
    },
    ...overrides,
  };
}

function makePrStatusHandle(prStatus: PrStatusResponse, selection: ReturnType<typeof makeSelection> | null = makeSelection()) {
  return {
    state: { status: 'ready' as const, prStatus: selection ? { ...prStatus, selection } : prStatus },
    refresh: vi.fn().mockResolvedValue(undefined),
    activeSelection: selection,
    activePrSummary: selection?.active_pr
      ? selection.associated_prs.find((pr) => pr.repo_owner === selection.active_pr?.pr.repo_owner
        && pr.repo_name === selection.active_pr?.pr.repo_name
        && pr.pr_number === selection.active_pr?.pr.pr_number) ?? null
      : null,
    ambiguous: !!selection && !selection.active_pr && selection.associated_prs.filter((pr) => pr.display_state === 'open' || pr.display_state === 'draft').length > 1,
    pinActivePr: vi.fn().mockResolvedValue(undefined),
    resumeInference: vi.fn().mockResolvedValue(undefined),
  };
}

function mockPrStatus(status: Partial<PrStatusResponse>): PrStatusResponse {
  return {
    found: false,
    refresh: {
      state: 'not_found',
      last_attempted_at: '2026-01-01T00:00:00Z',
      last_refreshed_at: '2026-01-01T00:00:00Z',
      stale: false,
    },
    work_change: { kind: 'clean' },
    ...status,
  };
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
    renderStateBar({ prStatus: mockPrStatus({
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
    }) });

    const badge = await screen.findByRole('link', { name: label });
    expect(badge).toHaveClass('pr-badge', className);
    expect(badge).toHaveAttribute('href', 'https://github.com/scottopell/phoenix-ide/pull/12');
    expect(badge).toHaveAttribute('target', '_blank');
    expect(badge).toHaveAttribute('rel', 'noreferrer');
    expect(badge.getAttribute('title')).toContain('Add PR tracking');
  });

  it('renders no badge when gh finds no PR', async () => {
    renderStateBar({ prStatus: mockPrStatus({ found: false }) });

    expect(screen.queryByText(/^#\d+/)).not.toBeInTheDocument();
  });

  it('renders a compact gh authentication hint when status is unavailable', async () => {
    renderStateBar({ prStatus: mockPrStatus({ found: false, unavailable_reason: 'not_authenticated' }) });

    expect(screen.getByText('gh auth')).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: /^#\d+/ })).not.toBeInTheDocument();
  });

  it('does not fetch PR status itself for conversations without a branch', async () => {
    renderStateBar({ conversation: makeConversation({ branch_name: null, base_branch: null }) });

    await waitFor(() => expect(screen.getByText('track-pr-status')).toBeInTheDocument());
    expect(api.getPrStatus).not.toHaveBeenCalled();
  });

  it('renders PR status as a direct PR link instead of an inline popover trigger', async () => {
    renderStateBar({ prStatus: mockPrStatus({
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
      feedback_summary: { total: 2, unresolved: 2, feedback_status: 'open', items: [], coverage: [] },
    }) });

    const badge = await screen.findByRole('link', { name: /#12 checks ✗/i });
    expect(badge).toHaveClass('pr-badge', 'pr-badge--failing');
    expect(badge).toHaveAttribute('href', 'https://github.com/scottopell/phoenix-ide/pull/12');
    expect(badge).toHaveAttribute('target', '_blank');
    expect(badge).toHaveAttribute('rel', 'noreferrer');
    expect(badge.getAttribute('title')).toContain('Fix CI');
    expect(screen.queryByRole('dialog', { name: /PR CI monitoring/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Auto-fix CI & address comments/i })).not.toBeInTheDocument();
    expect(api.createPrAutoFixContext).not.toHaveBeenCalled();
  });
  it('shows pinned selector choice and can resume automatic inference', async () => {
    const selection = makeSelection({ active_pr: { pr: { repo_owner: 'o', repo_name: 'r', pr_number: 12 }, provenance: 'pinned' } });
    const handle = makePrStatusHandle(mockPrStatus({ found: true, number: 12, url: 'https://github.com/scottopell/phoenix-ide/pull/12', display_state: 'open' }), selection);
    render(
      <MemoryRouter>
        <StateBar
          conversation={makeConversation()}
          convState={{ type: 'idle' }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          prStatusHandle={handle}
        />
      </MemoryRouter>,
    );

    expect(screen.getByTestId('active-pr-pinned-indicator')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('active-pr-selector-trigger'));
    fireEvent.click(screen.getByTestId('active-pr-resume-inference'));
    await waitFor(() => expect(handle.resumeInference).toHaveBeenCalled());
  });

  it('marks only the fully matching repo/number row as active', async () => {
    const selection = makeSelection({
      associated_prs: [
        { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Main repo PR', url: 'https://github.com/scottopell/phoenix-ide/pull/12', state: 'OPEN', draft: false, display_state: 'open', base: 'main', head: 'a', feedback_status: 'open' },
        { repo_owner: 'fork', repo_name: 'r', pr_number: 12, title: 'Fork PR', url: 'https://github.com/fork/r/pull/12', state: 'OPEN', draft: false, display_state: 'open', base: 'main', head: 'b', feedback_status: 'open' },
      ],
      active_pr: { pr: { repo_owner: 'o', repo_name: 'r', pr_number: 12 }, provenance: 'pinned' },
    });
    const handle = makePrStatusHandle(mockPrStatus({ found: true, number: 12, url: 'https://github.com/scottopell/phoenix-ide/pull/12', display_state: 'open' }), selection);
    render(
      <MemoryRouter>
        <StateBar
          conversation={makeConversation()}
          convState={{ type: 'idle' }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          prStatusHandle={handle}
        />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByTestId('active-pr-selector-trigger'));
    expect(screen.getByText('Main repo PR').closest('[role="menuitemradio"]')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByText('Fork PR').closest('[role="menuitemradio"]')).toHaveAttribute('aria-checked', 'false');
    expect(screen.getAllByText('Active')).toHaveLength(1);
  });

  it('shows ambiguity explicitly and does not render an unrelated PR badge', async () => {
    const selection = makeSelection({
      associated_prs: [
        { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://github.com/scottopell/phoenix-ide/pull/12', state: 'OPEN', draft: false, display_state: 'open', base: 'main', head: 'a', feedback_status: 'open' },
        { repo_owner: 'o', repo_name: 'r', pr_number: 34, title: 'Stacked follow-up', url: 'https://github.com/scottopell/phoenix-ide/pull/34', state: 'OPEN', draft: false, display_state: 'open', base: 'develop', head: 'b', feedback_status: 'open' },
      ],
    });
    delete selection.active_pr;
    const handle = makePrStatusHandle(mockPrStatus({ found: false }), selection);
    render(
      <MemoryRouter>
        <StateBar
          conversation={makeConversation()}
          convState={{ type: 'idle' }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          prStatusHandle={handle}
        />
      </MemoryRouter>,
    );

    expect(screen.queryByRole('link', { name: /^#12/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId('active-pr-selector-trigger'));
    expect(screen.getByTestId('active-pr-ambiguity-label')).toBeInTheDocument();
    expect(screen.getByText('b → develop · o/r')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('active-pr-choice-34'));
    await waitFor(() => expect(handle.pinActivePr).toHaveBeenCalledWith({ repo_owner: 'o', repo_name: 'r', pr_number: 34 }));
  });

  it('supports keyboard navigation, selection, escape, and focus restoration', async () => {
    const selection = makeSelection({
      associated_prs: [
        { repo_owner: 'o', repo_name: 'r', pr_number: 12, title: 'Fix CI', url: 'https://github.com/scottopell/phoenix-ide/pull/12', state: 'OPEN', draft: false, display_state: 'open', base: 'main', head: 'task-123-pr-status', feedback_status: 'open' },
        { repo_owner: 'o', repo_name: 'r', pr_number: 34, title: 'Stacked follow-up', url: 'https://github.com/scottopell/phoenix-ide/pull/34', state: 'OPEN', draft: false, display_state: 'open', base: 'task-123-pr-status', head: 'task-123-follow-up', feedback_status: 'open' },
      ],
      latest_observed_branch: { branch_name: 'task-123-follow-up', repository_identity: 'o/r' },
    });
    delete selection.active_pr;
    const handle = makePrStatusHandle(mockPrStatus({ found: false }), selection);
    render(
      <MemoryRouter>
        <button type="button">Before</button>
        <StateBar
          conversation={makeConversation()}
          convState={{ type: 'idle' }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          prStatusHandle={handle}
        />
      </MemoryRouter>,
    );

    const trigger = screen.getByTestId('active-pr-selector-trigger');
    fireEvent.click(trigger);
    expect(screen.getByTestId('active-pr-choice-12')).toHaveFocus();
    fireEvent.keyDown(screen.getByTestId('active-pr-choice-12'), { key: 'ArrowDown' });
    expect(screen.getByTestId('active-pr-choice-34')).toHaveFocus();
    fireEvent.keyDown(screen.getByTestId('active-pr-choice-34'), { key: 'Home' });
    expect(screen.getByTestId('active-pr-choice-12')).toHaveFocus();
    fireEvent.keyDown(screen.getByTestId('active-pr-choice-12'), { key: 'End' });
    expect(screen.getByTestId('active-pr-choice-34')).toHaveFocus();
    fireEvent.keyDown(screen.getByTestId('active-pr-choice-34'), { key: 'Escape' });
    await waitFor(() => expect(trigger).toHaveFocus());

    fireEvent.keyDown(trigger, { key: 'Enter' });
    expect(screen.getByTestId('active-pr-choice-12')).toHaveFocus();
    expect(screen.getByTestId('active-pr-ambiguity-label')).toBeInTheDocument();
    fireEvent.keyDown(screen.getByTestId('active-pr-choice-12'), { key: 'ArrowDown' });
    fireEvent.keyDown(screen.getByTestId('active-pr-choice-34'), { key: ' ' });
    await waitFor(() => expect(handle.pinActivePr).toHaveBeenCalledWith({ repo_owner: 'o', repo_name: 'r', pr_number: 34 }));
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it('shows pending and visible error state for selector mutations and mobile-safe auto summary', async () => {
    const selection = makeSelection({
      latest_observed_branch: { branch_name: 'task-123-pr-status', repository_identity: 'o/r' },
      active_pr: { pr: { repo_owner: 'o', repo_name: 'r', pr_number: 12 }, provenance: 'pinned' },
    });
    const pinActivePr = vi.fn().mockRejectedValue(new Error('Pin failed'));
    const resumeInference = vi.fn().mockImplementation(() => new Promise<void>(() => {}));
    const handle = { ...makePrStatusHandle(mockPrStatus({ found: true, number: 12, url: 'https://github.com/scottopell/phoenix-ide/pull/12', display_state: 'open' }), selection), pinActivePr, resumeInference };
    render(
      <MemoryRouter>
        <StateBar
          conversation={makeConversation()}
          convState={{ type: 'idle' }}
          connectionState="connected"
          connectionAttempt={0}
          nextRetryIn={null}
          contextWindowUsed={0}
          modelContextWindow={200_000}
          prStatusHandle={handle}
        />
      </MemoryRouter>,
    );

    fireEvent.click(screen.getByTestId('active-pr-selector-trigger'));
    expect(screen.getByText('Auto follows the latest observed branch: task-123-pr-status · o/r.')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('active-pr-choice-12'));
    expect(await screen.findByRole('alert')).toHaveTextContent('Pin failed');
    fireEvent.click(screen.getByTestId('active-pr-resume-inference'));
    expect(screen.getByRole('status')).toHaveTextContent('Saving active PR…');
  });
});

describe('StateBar model picker enablement (task 02713)', () => {
  const models: ModelInfo[] = [
    { id: 'claude-sonnet-5', provider: 'anthropic', description: '', context_window: 1_000_000, recommended: true },
    { id: 'claude-sonnet-4-6', provider: 'anthropic', description: '', context_window: 1_000_000, recommended: false },
    { id: 'claude-opus-4-7', provider: 'anthropic', description: '', context_window: 1_000_000, recommended: true },
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
    const { container } = renderWithState({ type: 'error', message: 'overloaded', error_kind: 'server_overloaded' });
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

    const action = await screen.findByRole('button', { name: /summarize & continue/i });
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

    expect(screen.queryByRole('button', { name: /summarize & continue/i })).not.toBeInTheDocument();
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
    expect(screen.getByText(/awaiting LLM response.*7s/i)).toBeInTheDocument();
    const dot = document.querySelector('.dot');
    expect(dot?.className).toMatch(/working/);
  });

  // Retry-suffix composition (REQ-WPV-003 / REQ-LRV-001). The
  // `turnRetryContext` is populated by `sse_llm_attempt` and survives
  // intra-turn phase transitions, so the suffix appears on every
  // working-phase rendering until agent_done / error clears it.
  it('appends "(retry K/N after <reason>)" during llm_requesting pre-first-byte', () => {
    renderStateBar({
      convState: { type: 'llm_requesting', attempt: 2 },
      phaseStateUpdatedAt: T_NOW - 5_000,
      lastSseEventAt: T_NOW - 1_000,
      turnRetryContext: { attempt: 2, maxAttempts: 3, reasonText: 'rate limit' },
    });
    expect(
      screen.getByText(/awaiting LLM response.*5s.*\(retry 2\/3 after rate limit\)/i)
    ).toBeInTheDocument();
  });

  it('appends the retry suffix even when first byte has arrived ("streaming (retry…)")', () => {
    renderStateBar({
      convState: { type: 'llm_requesting', attempt: 3 },
      phaseStateUpdatedAt: T_NOW - 2_000,
      lastSseEventAt: T_NOW - 200,
      firstByteRequestId: 'req-xyz',
      turnRetryContext: { attempt: 3, maxAttempts: 3, reasonText: 'server error' },
    });
    expect(screen.getByText(/^streaming \(retry 3\/3 after server error\)$/i)).toBeInTheDocument();
  });

  it('appends the retry suffix on tool_executing too (carries across intra-turn transitions)', () => {
    renderStateBar({
      convState: {
        type: 'tool_executing',
        current_tool: { id: 'bash-1', name: 'bash', input: {} },
        remaining_tools: [],
      },
      phaseStateUpdatedAt: T_NOW - 12_000,
      lastSseEventAt: T_NOW - 1_000,
      turnRetryContext: { attempt: 2, maxAttempts: 3, reasonText: 'network error' },
    });
    expect(
      screen.getByText(/\b12s.*\(retry 2\/3 after network error\)/i)
    ).toBeInTheDocument();
  });

  it('omits the retry suffix when turnRetryContext is null', () => {
    renderStateBar({
      convState: { type: 'llm_requesting', attempt: 1 },
      phaseStateUpdatedAt: T_NOW - 4_000,
      lastSseEventAt: T_NOW - 1_000,
      turnRetryContext: null,
    });
    expect(screen.queryByText(/\(retry/i)).not.toBeInTheDocument();
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
          lastSseEventAtRef={{ current: T_NOW - 1_000 }}
        />
      </MemoryRouter>,
    );
    // First render — connected, working: shows "awaiting LLM response ... 12s".
    expect(screen.getByText(/awaiting LLM response.*12s/i)).toBeInTheDocument();
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
          lastSseEventAtRef={{ current: T_NOW - 1_000 }}
        />
      </MemoryRouter>,
    );
    // Now we should see "reconnecting (2) — last: awaiting LLM response ... 12s".
    expect(screen.getByText(/reconnecting \(2\).*last.*awaiting LLM response.*12s/i)).toBeInTheDocument();
    const dot = document.querySelector('.dot');
    expect(dot?.className).toMatch(/reconnecting/);
  });

  // Disambiguation: llm_requesting and awaiting_user_response both
  // previously rendered the word "awaiting response", which conflated
  // "waiting on the LLM" with "waiting on the human user". The fix
  // qualifies the LLM case with "LLM response" and rewrites the user
  // case as a direct second-person address.
  it('renders "awaiting LLM response Ns" for llm_requesting (pre-first-byte)', () => {
    renderStateBar({
      convState: { type: 'llm_requesting', attempt: 1 },
      phaseStateUpdatedAt: T_NOW - 4_000,
      lastSseEventAt: T_NOW - 1_000,
    });
    expect(screen.getByText(/awaiting LLM response.*4s/i)).toBeInTheDocument();
    // Negative-case the ambiguous prose to make sure the old label
    // can't sneak back in.
    expect(screen.queryByText(/^awaiting response\b/i)).not.toBeInTheDocument();
  });

  it('renders "awaiting your reply" for awaiting_user_response', () => {
    renderStateBar({
      convState: {
        type: 'awaiting_user_response',
        questions: [
          {
            question: 'Which option?',
            header: 'Pick one',
            options: [{ label: 'A' }, { label: 'B' }],
            multiSelect: false,
          },
        ],
      },
      // awaiting_user_response is not a working phase — no counter,
      // and the prose must not mention "LLM response".
    });
    expect(screen.getByText(/awaiting your reply/i)).toBeInTheDocument();
    expect(screen.queryByText(/awaiting LLM response/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/^awaiting response\b/i)).not.toBeInTheDocument();
    const dot = document.querySelector('.dot');
    expect(dot?.className).toMatch(/approval/);
  });

  it('switches to "streaming" (no counter) once first byte arrives (REQ-WPV-007)', () => {
    renderStateBar({
      convState: { type: 'llm_requesting', attempt: 1 },
      phaseStateUpdatedAt: T_NOW - 4_000,
      lastSseEventAt: T_NOW - 1_000,
      firstByteRequestId: 'req-abc',
    });
    expect(screen.getByText(/^streaming$/i)).toBeInTheDocument();
    // The pre-first-byte "awaiting LLM response Ns" form must NOT be present once
    // the first byte has arrived.
    expect(screen.queryByText(/awaiting LLM response.*4s/i)).not.toBeInTheDocument();
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
          lastSseEventAtRef={{ current: T_NOW - 1_000 }}
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
          lastSseEventAtRef={{ current: T_NOW - 1_000 }}
        />
      </MemoryRouter>,
    );
    expect(screen.getByText(/^offline.*last.*8s/i)).toBeInTheDocument();
    const dot = document.querySelector('.dot');
    expect(dot?.className).toMatch(/offline/);
  });
});

describe('StateBar mobile layout', () => {
  beforeEach(() => {
    setMobileViewport(true);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
    });
  });

  it('keeps collapsed mobile sparse and exposes explore details when expanded', () => {
    renderStateBar({
      conversation: makeConversation({
        slug: 'explore-long-project',
        conv_mode_label: 'Explore',
        cwd: '/Users/scott/projects/phoenix-ide',
        branch_name: null,
        base_branch: 'main',
        task_title: null,
        project_name: 'Phoenix IDE',
      }),
    });

    expect(screen.getByText('explore-long-project')).toBeInTheDocument();
    expect(screen.getByText('ready')).toBeInTheDocument();
    expect(screen.queryByText(/read-only/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /copy full working directory/i })).not.toBeInTheDocument();

    fireEvent.click(screen.getAllByRole('button', { name: /expand status bar/i })[0]!);

    expect(screen.getByTitle(/Explore mode/i)).toHaveTextContent('Explore');
    expect(screen.getByText(/Read-only git project/i)).toBeInTheDocument();
    expect(screen.getByText('claude-sonnet-5')).toBeInTheDocument();
    expect(screen.getByText('…/projects/phoenix-ide')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /copy full working directory .*phoenix-ide/i })).toBeInTheDocument();
    expect(screen.queryByText('Phoenix IDE')).not.toBeInTheDocument();
    expect(screen.queryByText('main')).not.toBeInTheDocument();
  });

  it('renders work task, branch, PR, context, cwd copy, model, and file action without base branch', () => {
    const onOpenFiles = vi.fn();
    renderStateBar({
      onOpenFiles,
      contextWindowUsed: 170_000,
      modelContextWindow: 200_000,
      prStatus: mockPrStatus({
        found: true,
        number: 12,
        title: 'Mobile StateBar',
        url: 'https://github.com/scottopell/phoenix-ide/pull/12',
        state: 'OPEN',
        draft: false,
        base: 'main',
        head: 'task-56004-redesign-mobile-state-bar',
        display_state: 'open',
        check_state: 'passing',
      }),
    });

    fireEvent.click(screen.getByRole('button', { name: /browse project files/i }));
    expect(onOpenFiles).toHaveBeenCalledTimes(1);
    expect(screen.getAllByRole('button', { name: /expand status bar/i }).at(-1)!).toHaveAttribute('aria-expanded', 'false');

    fireEvent.click(screen.getAllByRole('button', { name: /expand status bar/i }).at(-1)!);

    expect(screen.getByText('Work')).toBeInTheDocument();
    expect(screen.getByText(/Task branch/i)).toBeInTheDocument();
    expect(screen.getByText('Track PR status')).toBeInTheDocument();
    expect(screen.getByText('task-123-pr-status')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /#12 checks ✓/i })).toBeInTheDocument();
    expect(screen.getByText('170k')).toBeInTheDocument();
    expect(screen.getByText('claude-sonnet-5')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /copy full working directory/i })).toBeInTheDocument();
    expect(screen.queryByText('main')).not.toBeInTheDocument();
  });

  it('renders branch mode branch identity without task title or base branch', () => {
    renderStateBar({
      conversation: makeConversation({
        conv_mode_label: 'Branch',
        task_title: null,
        branch_name: 'feature/existing-branch',
        base_branch: 'develop',
        cwd: '/repo/product',
      }),
    });

    fireEvent.click(screen.getAllByRole('button', { name: /expand status bar/i }).at(-1)!);

    expect(screen.getByTitle(/Branch mode/i)).toHaveTextContent('Branch');
    expect(screen.getByText(/Existing branch/i)).toBeInTheDocument();
    expect(screen.getByText('feature/existing-branch')).toBeInTheDocument();
    expect(screen.queryByText('Track PR status')).not.toBeInTheDocument();
    expect(screen.queryByText('develop')).not.toBeInTheDocument();
  });

  it('renders direct fallback mode and cwd without separate project name', () => {
    renderStateBar({
      conversation: makeConversation({
        conv_mode_label: 'Direct',
        cwd: '/Users/scott/projects/direct-project',
        branch_name: null,
        base_branch: null,
        task_title: null,
        project_name: 'Direct Project',
      }),
    });

    fireEvent.click(screen.getAllByRole('button', { name: /expand status bar/i }).at(-1)!);

    expect(screen.getByText('Direct')).toBeInTheDocument();
    expect(screen.getByText(/Full access/i)).toBeInTheDocument();
    expect(screen.getByText('…/projects/direct-project')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /copy full working directory .*direct-project/i })).toBeInTheDocument();
    expect(screen.queryByText('Direct Project')).not.toBeInTheDocument();
  });

  it('keeps model picker enablement and file button keyboard behavior on mobile', () => {
    const onUpgradeModel = vi.fn();
    const onOpenFiles = vi.fn();
    renderStateBar({
      availableModels: pickerModels,
      onUpgradeModel,
      onOpenFiles,
    });

    fireEvent.keyDown(screen.getByRole('button', { name: /browse project files/i }), { key: 'Enter' });
    fireEvent.click(screen.getByRole('button', { name: /browse project files/i }));
    expect(onOpenFiles).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(screen.getAllByRole('button', { name: /expand status bar/i })[0]!, { key: 'Enter' });
    expect(screen.getByTitle(/Model: claude-sonnet-5/i)).toBeInTheDocument();

    fireEvent.click(screen.getByTitle(/Model: claude-sonnet-5/i));
    expect(screen.getByRole('listbox', { name: /select model/i })).toBeInTheDocument();
  });
});
