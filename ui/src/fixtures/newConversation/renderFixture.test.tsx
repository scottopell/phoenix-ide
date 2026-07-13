import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
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
    expect(screen.getAllByText('Chat in a fresh worktree')).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: 'Send' })).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: 'Send' })[0]).toBeEnabled();
  });
});
