import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { MobileMultiPrConversationFixture } from './renderFixture';
import {
  getMobileMultiPrConversationScenario,
  mobileMultiPrAssociatedPrs,
  mobileMultiPrSelection,
} from './scenarios';

function setMobileViewport() {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: query === '(max-width: 768px)',
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

afterEach(() => cleanup());

describe('MobileMultiPrConversationFixture', () => {
  it('keeps exactly two open PRs ambiguous instead of selecting one', () => {
    expect(mobileMultiPrAssociatedPrs).toHaveLength(2);
    expect(mobileMultiPrAssociatedPrs.every((pr) => pr.display_state === 'open')).toBe(true);
    expect(mobileMultiPrSelection.active_pr).toBeUndefined();
  });

  it('shows PR-specific Work Actions when one of the same two open PRs is active', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('active-pr-actions');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.getByTestId('view-active-pr-diff-button')).toHaveTextContent('PR #423 Diff');
    expect(screen.getByTestId('address-feedback-button')).toHaveTextContent('Address PR #423 feedback');
    expect(screen.getByTestId('open-pr-link')).toHaveTextContent('Open PR #423');
    expect(screen.queryByTestId('active-pr-ambiguity-note')).not.toBeInTheDocument();
  });

  it('renders the production mobile StateBar with the two-PR chooser open', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('chooser-open');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.getByLabelText('Collapse status bar')).toBeInTheDocument();
    expect(screen.getByTestId('view-diff-button')).toHaveTextContent('Workspace Diff');
    expect(screen.getByTestId('active-pr-ambiguity-note')).toHaveTextContent('Multiple actionable PRs');
    expect(screen.queryByTestId('view-active-pr-diff-button')).not.toBeInTheDocument();
    expect(screen.queryByTestId('address-feedback-button')).not.toBeInTheDocument();
    expect(screen.queryByTestId('clean-up-button')).not.toBeInTheDocument();
    expect(screen.getByTestId('active-pr-ambiguity-label')).toHaveTextContent('Multiple actionable PRs');
    expect(screen.getByTestId('active-pr-choice-417')).toHaveTextContent('Add durable multi-PR conversation association');
    expect(screen.getByTestId('active-pr-choice-423')).toHaveTextContent('Follow up with mobile active-PR selection');
    expect(screen.getAllByRole('menuitemradio')).toHaveLength(2);
    expect(screen.queryByText('Active')).not.toBeInTheDocument();
  });
});
