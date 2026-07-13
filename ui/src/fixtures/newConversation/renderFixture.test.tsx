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
    const suggestions = screen.getAllByLabelText('Suggested projects');
    expect(suggestions).toHaveLength(2);
    expect(suggestions[0]).toHaveTextContent('phoenix-ide');
    expect(suggestions[0]).toHaveTextContent('design-system');
    expect(suggestions[0]).toHaveTextContent('agent-tools');
    expect(screen.getAllByDisplayValue(scenario.draft)).toHaveLength(2);
    expect(screen.getAllByText('Chat in a fresh worktree')).toHaveLength(2);
    expect(screen.getAllByText('Pick a task file and approve the plan before Work mode.')).toHaveLength(2);
    expect(screen.queryByText('Loading tasks...')).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Send' })).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: 'Send' })[0]).toBeEnabled();
  });

  it('restores the document theme, storage, and API methods after unmount', async () => {
    document.documentElement.dataset['theme'] = 'light';
    localStorage.setItem('phoenix-last-cwd', '/previous/project');
    const originalGetProjects = api.getProjects;
    const { container, unmount } = render(<NewConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-new-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });
    expect(document.documentElement.dataset['theme']).toBe('dark');
    expect(api.getProjects).not.toBe(originalGetProjects);

    unmount();

    expect(document.documentElement.dataset['theme']).toBe('light');
    expect(localStorage.getItem('phoenix-last-cwd')).toBe('/previous/project');
    expect(api.getProjects).toBe(originalGetProjects);
  });
});
