import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { api } from '../../api';
import { NewConversationFixture } from './renderFixture';
import { getNewConversationScenario } from './scenarios';

const scenario = getNewConversationScenario('ready-git-project');

afterEach(() => cleanup());

describe('NewConversationFixture', () => {
  it('renders the real ready Git-project page and exposes its capture marker', async () => {
    const { container } = render(<NewConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-new-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(screen.getAllByDisplayValue(scenario.cwd)).toHaveLength(2);
    expect(screen.getAllByDisplayValue(scenario.draft)).toHaveLength(2);
    expect(screen.queryByLabelText('Suggested projects')).not.toBeInTheDocument();
    expect(screen.queryByText('Workflow')).not.toBeInTheDocument();
    expect(screen.queryByText('Chat in a fresh worktree')).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Send' })).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: 'Send' })[0]).toBeEnabled();
  });

  it('restores the document theme, storage, and API methods after unmount', async () => {
    document.documentElement.dataset['theme'] = 'light';
    localStorage.setItem('phoenix-last-cwd', '/previous/project');
    const originalListModels = api.listModels;
    const { container, unmount } = render(<NewConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-new-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });
    expect(document.documentElement.dataset['theme']).toBe('dark');
    expect(api.listModels).not.toBe(originalListModels);

    unmount();

    expect(document.documentElement.dataset['theme']).toBe('light');
    expect(localStorage.getItem('phoenix-last-cwd')).toBe('/previous/project');
    expect(api.listModels).toBe(originalListModels);
  });
});
