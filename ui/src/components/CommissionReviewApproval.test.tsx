import '../index.css';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { CommissionReviewApproval } from './CommissionReviewApproval';

function renderApproval(overrides: Partial<React.ComponentProps<typeof CommissionReviewApproval>> = {}) {
  const onApprove = vi.fn();
  const onReject = vi.fn();

  render(
    <CommissionReviewApproval
      brief="Review the branch before spending commission tokens."
      focus="Check timer smells in approval handling."
      scope={{
        kind: 'pull_request',
        repo_root: '/repo/phoenix',
        base: 'origin/main',
        head: 'task-19004',
        dirty: false,
        changed_files: 4,
        insertions: 37,
        deletions: 12,
      }}
      onApprove={onApprove}
      onReject={onReject}
      {...overrides}
    />,
  );

  return { onApprove, onReject };
}

describe('CommissionReviewApproval', () => {
  it('renders the dialog with accessible scope details and pending state', () => {
    renderApproval();

    const dialog = screen.getByRole('dialog', { name: 'Commission code review?' });
    expect(dialog).toHaveAttribute('aria-busy', 'false');
    expect(screen.getByText('Capital spend request')).toBeInTheDocument();
    expect(screen.getByText('Awaiting decision')).toBeInTheDocument();
    expect(screen.getByText('Committed-only scope')).toBeInTheDocument();
    expect(screen.getByText('pull request')).toBeInTheDocument();
    expect(screen.getByText('/repo/phoenix')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reject' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Approve review' })).toBeEnabled();
  });

  it('shows busy state and settles after approval', async () => {
    let resolveApprove: (() => void) | undefined;
    const onApprove = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveApprove = resolve;
        }),
    );
    renderApproval({ onApprove });

    fireEvent.click(screen.getByRole('button', { name: 'Approve review' }));

    expect(onApprove).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('dialog')).toHaveAttribute('aria-busy', 'true');
    expect(screen.getByRole('button', { name: 'Approving…' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Reject' })).toBeDisabled();

    resolveApprove?.();

    await waitFor(() => {
      expect(screen.getByRole('status')).toHaveTextContent(
        'ApprovedStarting review… this dialog will close when the conversation state updates.',
      );
    });
    expect(screen.getByRole('status')).toHaveTextContent('Approved');
    expect(screen.getByRole('button', { name: 'Approve review' })).toBeDisabled();
    expect(screen.getByRole('dialog')).toHaveAttribute('aria-busy', 'false');
  });

  it('surfaces action failures and re-enables retry', async () => {
    const onApprove = vi.fn().mockRejectedValue(new Error('Server refused review'));
    renderApproval({ onApprove });

    fireEvent.click(screen.getByRole('button', { name: 'Approve review' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('Server refused review');
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Approve review' })).toBeEnabled();
    });
    expect(screen.getByText('Needs retry')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reject' })).toBeEnabled();
  });

  it('supports rejection and dirty scope messaging', async () => {
    const onReject = vi.fn().mockResolvedValue(undefined);
    renderApproval({
      onReject,
      scope: {
        kind: 'worktree_diff',
        repo_root: '/repo/phoenix',
        base: 'main',
        head: 'feature/dirty',
        dirty: true,
        changed_files: 2,
        insertions: 5,
        deletions: 1,
      },
      focus: null,
    });

    expect(screen.queryByText('Focus')).not.toBeInTheDocument();
    expect(screen.getByText('Dirty tree detected')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Reject' }));

    await waitFor(() => {
      expect(screen.getByRole('status')).toHaveTextContent(
        'RejectedReview request rejected. No review tokens will be spent.',
      );
    });
    expect(onReject).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Approve review' })).toBeDisabled();
  });
});
