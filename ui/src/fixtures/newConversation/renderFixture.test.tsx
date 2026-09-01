import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api } from '../../api';
import { NewConversationFixture } from './renderFixture';
import { getNewConversationScenario } from './scenarios';

const readyScenario = getNewConversationScenario('ready-git-project');
const recoveryScenario = getNewConversationScenario('recovery-staging');

afterEach(() => cleanup());

describe('NewConversationFixture', () => {
  it('renders the real ready Git-project page and exposes its capture marker', async () => {
    const { container } = render(<NewConversationFixture scenario={readyScenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-new-conversation-fixture-ready="${readyScenario.id}"]`)).not.toBeNull();
    });

    expect(screen.getAllByDisplayValue(readyScenario.cwd)).toHaveLength(2);
    expect(screen.getAllByDisplayValue(readyScenario.draft)).toHaveLength(2);
    expect(screen.queryByLabelText('Suggested projects')).not.toBeInTheDocument();
    expect(screen.queryByText('Workflow')).not.toBeInTheDocument();
    expect(screen.queryByText('Chat in a fresh worktree')).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Send' })).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: 'Send' })[0]).toBeEnabled();
    expect(container.querySelector('[data-new-conversation-recovery-count="0"]')).not.toBeNull();
  });

  it('stages shipped recovery presentation only from shipped types and records fixture-local actions', async () => {
    const { container } = render(<NewConversationFixture scenario={recoveryScenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-new-conversation-fixture-ready="${recoveryScenario.id}"]`)).not.toBeNull();
    });

    expect(screen.getAllByLabelText('Recent product creation attempts')).toHaveLength(2);
    expect(screen.getAllByText('Needs retry')).toHaveLength(2);
    expect(screen.getAllByText('Failed')).toHaveLength(2);
    expect(screen.getAllByText('Finishing')).toHaveLength(2);
    expect(screen.getAllByText('Image-only request')).toHaveLength(2);
    const retryButtons = screen.getAllByRole('button', { name: 'Retry' });
    const retryButton = retryButtons[0];
    expect(retryButton).toBeDefined();
    expect(retryButton!).toBeEnabled();
    fireEvent.click(retryButton!);
    await waitFor(() => {
      expect(screen.getAllByRole('button', { name: 'Retry' })[0]).toBeEnabled();
    });
    expect(screen.getAllByRole('button', { name: 'Delete' })).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: 'Start over' })).toHaveLength(2);
  });

  it('restores the document theme, storage, and API methods after unmount', async () => {
    document.documentElement.dataset['theme'] = 'light';
    localStorage.setItem('phoenix-last-cwd', '/previous/project');
    const originalListModels = api.listModels;
    const originalListRecovery = api.listProductConversationCreations;
    const { container, unmount } = render(<NewConversationFixture scenario={readyScenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-new-conversation-fixture-ready="${readyScenario.id}"]`)).not.toBeNull();
    });
    expect(document.documentElement.dataset['theme']).toBe('dark');
    expect(api.listModels).not.toBe(originalListModels);
    expect(api.listProductConversationCreations).not.toBe(originalListRecovery);

    unmount();

    expect(document.documentElement.dataset['theme']).toBe('light');
    expect(localStorage.getItem('phoenix-last-cwd')).toBe('/previous/project');
    expect(api.listModels).toBe(originalListModels);
    expect(api.listProductConversationCreations).toBe(originalListRecovery);
  });
});
