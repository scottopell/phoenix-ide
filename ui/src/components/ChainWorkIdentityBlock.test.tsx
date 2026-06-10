// ChainWorkIdentityBlock tests (REQ-CHN-008).
//
// The block is the work-identity facet of the chain dock. The PR-status hook is
// mocked so these tests stay focused on the block's own rendering: durable work
// identity from props, PR health reused from the per-conversation pipeline, and
// the "no managed work scope" empty state.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import type { ChainWorkIdentity, PrStatusResponse } from '../api';
import type { ConversationPrStatusState } from '../hooks/useConversationPrStatus';
import { ChainWorkIdentityBlock } from './ChainWorkIdentityBlock';

const mockUsePrStatus = vi.fn();
vi.mock('../hooks/useConversationPrStatus', () => ({
  useConversationPrStatus: (args: unknown) => mockUsePrStatus(args),
}));

function setPrState(state: ConversationPrStatusState) {
  mockUsePrStatus.mockReturnValue({
    state,
    manualFallbackEnabled: false,
    enableManualFallback: vi.fn(),
    refresh: vi.fn(),
  });
}

const WORK_IDENTITY: ChainWorkIdentity = {
  work_conv_id: 'm3',
  worktree_path: '/wt/auth',
  branch_name: 'feat-auth',
  base_branch: 'main',
  task_id: '4242',
  task_title: 'Auth refactor',
};

beforeEach(() => {
  mockUsePrStatus.mockReset();
  setPrState({ status: 'disabled', prStatus: null });
});

describe('ChainWorkIdentityBlock', () => {
  it('renders worktree / branch / task for a managed chain', () => {
    render(<ChainWorkIdentityBlock identity={WORK_IDENTITY} />);

    expect(screen.getByTitle('feat-auth → main')).toBeInTheDocument();
    expect(screen.getByText('/wt/auth')).toBeInTheDocument();
    expect(screen.getByText('4242')).toBeInTheDocument();
    expect(screen.getByText(/Auth refactor/)).toBeInTheDocument();
  });

  it('keys PR status off the worktree-owning member as Work mode', () => {
    render(<ChainWorkIdentityBlock identity={WORK_IDENTITY} />);
    expect(mockUsePrStatus).toHaveBeenCalledWith({
      conversationId: 'm3',
      convModeLabel: 'Work',
      branchName: 'feat-auth',
    });
  });

  it('treats a taskless worktree as Branch mode', () => {
    render(
      <ChainWorkIdentityBlock
        identity={{ ...WORK_IDENTITY, task_id: null, task_title: null }}
      />,
    );
    expect(mockUsePrStatus).toHaveBeenCalledWith({
      conversationId: 'm3',
      convModeLabel: 'Branch',
      branchName: 'feat-auth',
    });
    // No Task row when there is no task.
    expect(screen.queryByText('4242')).not.toBeInTheDocument();
  });

  it('renders PR health from the pipeline, including a freshness tag', () => {
    const prStatus: PrStatusResponse = {
      found: true,
      number: 248,
      display_state: 'open',
      check_state: 'passing',
      feedback_freshness: { state: 'new', new_count: 3 },
      refresh: { state: 'fresh', stale: false, last_attempted_at: '2026-04-29T12:00:00Z' },
    };
    setPrState({ status: 'ready', prStatus });

    render(<ChainWorkIdentityBlock identity={WORK_IDENTITY} />);

    expect(screen.getByText('#248 checks ✓')).toBeInTheDocument();
    expect(screen.getByText('3 new')).toBeInTheDocument();
  });

  it('surfaces an unavailable-PR hint rather than claiming "no PR"', () => {
    const prStatus: PrStatusResponse = {
      found: false,
      unavailable_reason: 'gh_missing',
      refresh: {
        state: 'unavailable',
        reason: 'gh_missing',
        stale: false,
        last_attempted_at: '2026-04-29T12:00:00Z',
      },
    };
    setPrState({ status: 'ready', prStatus });

    render(<ChainWorkIdentityBlock identity={WORK_IDENTITY} />);

    expect(screen.getByText('gh missing')).toBeInTheDocument();
    expect(screen.queryByText('no PR')).not.toBeInTheDocument();
  });

  it('shows "No managed work scope" when the chain owns no worktree', () => {
    render(<ChainWorkIdentityBlock identity={null} />);
    expect(screen.getByText('No managed work scope')).toBeInTheDocument();
    // Disabled hook → no PR fetch keyed to a conversation.
    expect(mockUsePrStatus).toHaveBeenCalledWith({
      conversationId: null,
      convModeLabel: undefined,
      branchName: null,
    });
  });
});
