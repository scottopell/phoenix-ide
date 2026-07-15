import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { api, ConflictError } from '../api';
import type { ProjectInstructionRefreshStatus } from '../generated/ProjectInstructionRefreshStatus';
import { ProjectInstructionsRefresh } from './ProjectInstructionsRefresh';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: {
      ...actual.api,
      getProjectInstructionRefreshStatus: vi.fn(),
      confirmProjectInstructionRefresh: vi.fn(),
    },
  };
});

const changedStatus: ProjectInstructionRefreshStatus = {
  active_bundle_id: 'active-1',
  queued_bundle_id: null,
  candidate_bundle_id: 'candidate-2',
  changed_manifest: {
    guidance: [
      { relative_path: 'AGENTS.md', status: 'changed' },
      { relative_path: 'docs/AGENTS.md', status: 'added' },
      { relative_path: 'old/AGENT.md', status: 'removed' },
    ],
    skills: [{ name: 'rust-dev', status: 'changed' }],
    unchanged_guidance_count: 2,
    unchanged_skill_count: 5,
  },
  estimated_rewarm_tokens: 12_400,
  rewarm_tokens_are_estimate: true,
  rewarm_estimate_notice: 'Provider tokenization may differ.',
  is_queued: false,
};

const queuedStatus: ProjectInstructionRefreshStatus = {
  ...changedStatus,
  queued_bundle_id: 'candidate-2',
  candidate_bundle_id: null,
  is_queued: true,
};

const getStatus = vi.mocked(api.getProjectInstructionRefreshStatus);
const confirmRefresh = vi.mocked(api.confirmProjectInstructionRefresh);

async function renderChanged(state: 'idle' | 'awaiting_llm' = 'idle') {
  getStatus.mockResolvedValue(changedStatus);
  render(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: state }} />);
  await screen.findByText('↻ changed');
}

async function openDialog() {
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  return screen.findByRole('dialog');
}

describe('ProjectInstructionsRefresh', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('lists the content-free changed manifest, unchanged summary, and provider estimate', async () => {
    await renderChanged();
    const dialog = within(await openDialog());

    expect(dialog.getByText('AGENTS.md')).toBeInTheDocument();
    expect(dialog.getByText('docs/AGENTS.md')).toBeInTheDocument();
    expect(dialog.getByText('old/AGENT.md')).toBeInTheDocument();
    expect(dialog.getByText('rust-dev')).toBeInTheDocument();
    expect(dialog.getByText('Unchanged: 2 guidance, 5 skills')).toBeInTheDocument();
    expect(dialog.getByText('May rewarm ~12K input tokens once.')).toBeInTheDocument();
    expect(dialog.getByText('Provider tokenization may differ.')).toBeInTheDocument();
    expect(dialog.queryByText(/instruction contents/i)).not.toBeInTheDocument();
    expect(dialog.queryByText(/secret guidance/i)).not.toBeInTheDocument();
  });

  it('queues the exact preview candidate and shows the queued label', async () => {
    confirmRefresh.mockResolvedValue({ status: queuedStatus });
    await renderChanged('awaiting_llm');
    const dialog = within(await openDialog());
    expect(dialog.getByText('The conversation is working. Activation waits for the next user turn.')).toBeInTheDocument();

    fireEvent.click(dialog.getByRole('button', { name: 'Confirm refresh' }));

    await waitFor(() => expect(confirmRefresh).toHaveBeenCalledWith('conv-1', 'candidate-2'));
    expect(await screen.findByText('queued for next user turn')).toBeInTheDocument();
  });

  it('refetches and clearly reports a stale candidate conflict', async () => {
    const updated = {
      ...changedStatus,
      candidate_bundle_id: 'candidate-3',
      changed_manifest: {
        ...changedStatus.changed_manifest,
        guidance: [{ relative_path: 'new/AGENTS.md', status: 'changed' as const }],
      },
    };
    getStatus.mockResolvedValueOnce(changedStatus).mockResolvedValueOnce(changedStatus).mockResolvedValueOnce(updated);
    confirmRefresh.mockRejectedValue(new ConflictError({
      error: 'stale',
      error_type: 'stale_project_instruction_candidate',
    }));
    render(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'idle' }} />);
    await screen.findByText('↻ changed');
    const dialog = within(await openDialog());
    fireEvent.click(dialog.getByRole('button', { name: 'Confirm refresh' }));

    expect(await dialog.findByRole('alert')).toHaveTextContent('changed while this preview was open');
    expect(dialog.getByText('new/AGENTS.md')).toBeInTheDocument();
    expect(confirmRefresh).toHaveBeenCalledWith('conv-1', 'candidate-2');
  });

  it('renders queued status directly', async () => {
    getStatus.mockResolvedValue(queuedStatus);
    render(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'idle' }} />);
    expect(await screen.findByText('queued for next user turn')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Refresh' })).not.toBeInTheDocument();
  });
});
