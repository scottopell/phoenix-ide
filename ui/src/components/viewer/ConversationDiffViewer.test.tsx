import { render, screen, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ReviewNotesProvider } from '../../contexts/ReviewNotesContext';
import { ConversationDiffViewer } from './ConversationDiffViewer';
import { api } from '../../api';

vi.mock('../../api', () => ({ api: { getConversationDiff: vi.fn(), getActivePrDiff: vi.fn() } }));

// CodeView renders asynchronously through Shiki; the mock surfaces the parsed
// file name synchronously so the loader's conversation-keyed behavior is
// observable. The diff for `MARKER` touches the file `MARKER.txt`.
vi.mock('@pierre/diffs/react', async () => {
  const { makeCodeViewMock } = await import('./__testutils__/codeViewMock');
  return makeCodeViewMock();
});

function payloadFor(marker: string) {
  return {
    comparator: 'origin/main',
    label: 'Workspace Diff',
    kind: 'workspace',
    commit_log: '',
    committed_diff: `diff --git a/${marker}.txt b/${marker}.txt\n--- a/${marker}.txt\n+++ b/${marker}.txt\n@@ -0,0 +1 @@\n+${marker}`,
    uncommitted_diff: '',
  };
}

function renderViewer(conversationId: string, takeover = false) {
  return render(
    <ReviewNotesProvider>
      <ConversationDiffViewer conversationId={conversationId} onClose={() => undefined} onSendNotes={() => undefined} takeover={takeover} />
    </ReviewNotesProvider>,
  );
}

describe('ConversationDiffViewer — conversation-keyed payload', () => {
  beforeEach(() => vi.clearAllMocks());

  it('does not render a previous conversation’s diff after conversationId changes', async () => {
    // conv-1 resolves immediately; conv-2 is held pending so we can observe the
    // window where the stale conv-1 payload could otherwise show.
    let resolveConv2: ((v: ReturnType<typeof payloadFor>) => void) | null = null;
    (api.getConversationDiff as ReturnType<typeof vi.fn>).mockImplementation((id: string) => {
      if (id === 'conv-1') return Promise.resolve(payloadFor('CONV1'));
      return new Promise((res) => { resolveConv2 = res; });
    });

    const { rerender } = renderViewer('conv-1');
    await waitFor(() => expect(screen.getByText('CONV1.txt')).toBeInTheDocument());

    // Switch conversation; conv-2's fetch is still pending.
    rerender(
      <ReviewNotesProvider>
        <ConversationDiffViewer conversationId="conv-2" onClose={() => undefined} onSendNotes={() => undefined} />
      </ReviewNotesProvider>,
    );

    // conv-1's diff must be gone; loading shown until conv-2 resolves.
    expect(screen.queryByText('CONV1.txt')).not.toBeInTheDocument();
    expect(screen.getByText('Loading diff...')).toBeInTheDocument();

    resolveConv2!(payloadFor('CONV2'));
    await waitFor(() => expect(screen.getByText('CONV2.txt')).toBeInTheDocument());
  });


  it('loads the active PR diff endpoint when requested', async () => {
    (api.getActivePrDiff as ReturnType<typeof vi.fn>).mockResolvedValue({
      ...payloadFor('STACKED'),
      label: 'PR #88 Diff',
      kind: 'active_pr',
      pr_number: 88,
      comparator: 'task/base-pr',
    });

    render(
      <ReviewNotesProvider>
        <ConversationDiffViewer conversationId="conv-pr" target="active_pr" onClose={() => undefined} onSendNotes={() => undefined} />
      </ReviewNotesProvider>,
    );

    await waitFor(() => expect(api.getActivePrDiff).toHaveBeenCalledWith('conv-pr'));
    expect(await screen.findByText('STACKED.txt')).toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'PR #88 Diff' })).toBeInTheDocument();
  });

  it('renders fullscreen presentation as a takeover dialog', async () => {
    (api.getConversationDiff as ReturnType<typeof vi.fn>).mockResolvedValue(payloadFor('TAKEOVER'));

    renderViewer('conv-1', true);

    const dialog = await screen.findByRole('dialog', { name: 'Workspace Diff' });
    expect(dialog).toHaveClass('viewer-shell--takeover');
    expect(dialog).not.toHaveClass('viewer-shell--inline');
  });
});
