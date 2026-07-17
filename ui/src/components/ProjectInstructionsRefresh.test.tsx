import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
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
      previewProjectInstructionRefresh: vi.fn(),
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

const currentStatus: ProjectInstructionRefreshStatus = {
  ...changedStatus,
  candidate_bundle_id: null,
  changed_manifest: {
    guidance: [],
    skills: [],
    unchanged_guidance_count: 3,
    unchanged_skill_count: 6,
  },
};

const queuedStatus: ProjectInstructionRefreshStatus = {
  ...currentStatus,
  queued_bundle_id: 'candidate-2',
  is_queued: true,
};

const queuedWithNewerStatus: ProjectInstructionRefreshStatus = {
  ...changedStatus,
  queued_bundle_id: 'candidate-2',
  candidate_bundle_id: 'candidate-3',
  is_queued: true,
};

const getStatus = vi.mocked(api.getProjectInstructionRefreshStatus);
const getPreview = vi.mocked(api.previewProjectInstructionRefresh);
const confirmRefresh = vi.mocked(api.confirmProjectInstructionRefresh);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((promiseResolve) => { resolve = promiseResolve; });
  return { promise, resolve };
}

async function renderChanged(state: 'idle' | 'awaiting_llm' = 'idle') {
  getStatus.mockResolvedValue(changedStatus);
  getPreview.mockResolvedValue(changedStatus);
  render(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: state }} />);
  await screen.findByText('↻ changed');
}

async function openDialog() {
  fireEvent.click(screen.getByRole('button', { name: 'Review changes' }));
  return screen.findByRole('dialog');
}

describe('ProjectInstructionsRefresh', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('opens only from the explicit preview and lists its content-free manifest', async () => {
    await renderChanged();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

    const dialog = within(await openDialog());

    expect(getPreview).toHaveBeenCalledWith('conv-1', expect.any(AbortSignal));
    expect(dialog.getByText('AGENTS.md')).toBeInTheDocument();
    expect(dialog.getByText('docs/AGENTS.md')).toBeInTheDocument();
    expect(dialog.getByText('old/AGENT.md')).toBeInTheDocument();
    expect(dialog.getByText('rust-dev')).toBeInTheDocument();
    expect(dialog.getByText('Unchanged: 2 guidance, 5 skills')).toBeInTheDocument();
    expect(dialog.getByText('May rewarm ~12K input tokens once.')).toBeInTheDocument();
    expect(dialog.getByText('Provider tokenization may differ.')).toBeInTheDocument();
    expect(dialog.queryByText(/instruction contents/i)).not.toBeInTheDocument();
  });

  it('pins an open dialog to its explicit preview while background status changes', async () => {
    const background = deferred<ProjectInstructionRefreshStatus>();
    getStatus.mockResolvedValueOnce(changedStatus).mockReturnValueOnce(background.promise);
    getPreview.mockResolvedValue(changedStatus);
    const { rerender } = render(
      <ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'idle' }} />,
    );
    await screen.findByText('↻ changed');
    const dialog = within(await openDialog());

    rerender(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'awaiting_llm' }} />);
    background.resolve({
      ...changedStatus,
      candidate_bundle_id: 'candidate-background',
      changed_manifest: {
        ...changedStatus.changed_manifest,
        guidance: [{ relative_path: 'background/AGENTS.md', status: 'added' }],
      },
    });

    await waitFor(() => expect(getStatus).toHaveBeenCalledTimes(2));
    expect(dialog.getByText('AGENTS.md')).toBeInTheDocument();
    expect(dialog.queryByText('background/AGENTS.md')).not.toBeInTheDocument();
    fireEvent.click(dialog.getByRole('button', { name: 'Confirm refresh' }));
    await waitFor(() => expect(confirmRefresh).toHaveBeenCalledWith('conv-1', 'candidate-2'));
  });

  it('keeps the queued badge while exposing newer changes independently', async () => {
    getStatus.mockResolvedValue(queuedWithNewerStatus);
    getPreview.mockResolvedValue(queuedWithNewerStatus);
    render(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'idle' }} />);

    expect(await screen.findByText('queued for next user turn')).toBeInTheDocument();
    expect(screen.getByText('↻ changed')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Review newer changes' })).toBeInTheDocument();
  });

  it('offers a low-noise explicit Check action while current', async () => {
    getStatus.mockResolvedValueOnce(currentStatus).mockResolvedValueOnce(changedStatus);
    render(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'idle' }} />);
    expect(await screen.findByLabelText('Project instructions current')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Check' }));

    expect(await screen.findByText('↻ changed')).toBeInTheDocument();
    expect(getStatus).toHaveBeenCalledTimes(2);
    expect(getPreview).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('ignores out-of-order status responses', async () => {
    const first = deferred<ProjectInstructionRefreshStatus>();
    const second = deferred<ProjectInstructionRefreshStatus>();
    getStatus.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { rerender } = render(
      <ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'idle' }} />,
    );
    rerender(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'awaiting_llm' }} />);

    await act(async () => second.resolve(changedStatus));
    expect(await screen.findByText('↻ changed')).toBeInTheDocument();
    await act(async () => first.resolve(currentStatus));
    await waitFor(() => expect(screen.getByText('↻ changed')).toBeInTheDocument());
    expect(screen.queryByLabelText('Project instructions current')).not.toBeInTheDocument();
  });

  it('resets the preview and ignores previous-conversation responses', async () => {
    const oldPreview = deferred<ProjectInstructionRefreshStatus>();
    getStatus.mockResolvedValue(changedStatus);
    getPreview.mockReturnValue(oldPreview.promise);
    const { rerender } = render(
      <ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'idle' }} />,
    );
    await screen.findByText('↻ changed');
    fireEvent.click(screen.getByRole('button', { name: 'Review changes' }));

    rerender(<ProjectInstructionsRefresh conversationId="conv-2" conversationState={{ type: 'idle' }} />);
    oldPreview.resolve(changedStatus);

    await waitFor(() => expect(getStatus).toHaveBeenCalledWith('conv-2', expect.any(AbortSignal)));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('prevents duplicate preview requests', async () => {
    const preview = deferred<ProjectInstructionRefreshStatus>();
    getStatus.mockResolvedValue(changedStatus);
    getPreview.mockReturnValue(preview.promise);
    render(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'idle' }} />);
    await screen.findByText('↻ changed');
    const button = screen.getByRole('button', { name: 'Review changes' });

    fireEvent.click(button);
    fireEvent.click(button);

    expect(getPreview).toHaveBeenCalledTimes(1);
    await act(async () => preview.resolve(changedStatus));
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
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

  it('refetches an explicit preview on stale conflict and requires review again', async () => {
    const updated = {
      ...changedStatus,
      candidate_bundle_id: 'candidate-3',
      changed_manifest: {
        ...changedStatus.changed_manifest,
        guidance: [{ relative_path: 'new/AGENTS.md', status: 'changed' as const }],
      },
    };
    getStatus.mockResolvedValue(changedStatus);
    getPreview.mockResolvedValueOnce(changedStatus).mockResolvedValueOnce(updated);
    confirmRefresh.mockRejectedValueOnce(new ConflictError({
      error: 'stale',
      error_type: 'stale_project_instruction_candidate',
    })).mockResolvedValueOnce({ status: queuedStatus });
    render(<ProjectInstructionsRefresh conversationId="conv-1" conversationState={{ type: 'idle' }} />);
    await screen.findByText('↻ changed');
    const dialog = within(await openDialog());
    fireEvent.click(dialog.getByRole('button', { name: 'Confirm refresh' }));

    expect(await dialog.findByRole('alert')).toHaveTextContent('preview changed');
    expect(dialog.getByText('new/AGENTS.md')).toBeInTheDocument();
    expect(getPreview).toHaveBeenCalledTimes(2);
    expect(confirmRefresh).toHaveBeenCalledTimes(1);

    fireEvent.click(dialog.getByRole('button', { name: 'Confirm refresh' }));
    await waitFor(() => expect(confirmRefresh).toHaveBeenLastCalledWith('conv-1', 'candidate-3'));
  });

  it('closes on Escape and restores focus to the review trigger', async () => {
    await renderChanged();
    const trigger = screen.getByRole('button', { name: 'Review changes' });
    await openDialog();
    expect(screen.getByRole('button', { name: 'Confirm refresh' })).toHaveFocus();

    fireEvent.keyDown(document, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(trigger).toHaveFocus();
  });
});
