import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { StateBar } from './StateBar';
import { api, type Conversation, type PrStatusResponse } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      getPrStatus: vi.fn(),
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
    commits_ahead: 1,
    commits_behind: 0,
    conv_mode_label: 'Work',
    browser_session_active: false,
    ...overrides,
  };
}

function renderStateBar(conversation: Conversation = makeConversation()) {
  return render(
    <MemoryRouter>
      <StateBar
        conversation={conversation}
        convState={{ type: 'idle' }}
        connectionState="connected"
        connectionAttempt={0}
        nextRetryIn={null}
        contextWindowUsed={0}
        modelContextWindow={200_000}
      />
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

    const badge = await screen.findByRole('link', { name: label });
    expect(badge).toHaveClass('pr-badge', className);
    expect(badge).toHaveAttribute('href', 'https://github.com/scottopell/phoenix-ide/pull/12');
    expect(badge).toHaveAttribute('target', '_blank');
    expect(badge).toHaveAttribute('rel', 'noreferrer');
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
    expect(screen.queryByRole('link', { name: /^#\d+/ })).not.toBeInTheDocument();
  });

  it('does not fetch PR status for conversations without a branch', async () => {
    renderStateBar(makeConversation({ branch_name: null, base_branch: null }));

    await waitFor(() => expect(screen.getByText('track-pr-status')).toBeInTheDocument());
    expect(api.getPrStatus).not.toHaveBeenCalled();
  });
});
