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

  it('renders the production mobile StateBar with the two-PR chooser open', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('chooser-open');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.getByLabelText('Collapse status bar')).toBeInTheDocument();
    expect(screen.getByTestId('active-pr-ambiguity-label')).toHaveTextContent('Multiple actionable PRs');
    expect(screen.getByTestId('active-pr-choice-417')).toHaveTextContent('Add durable multi-PR conversation association');
    expect(screen.getByTestId('active-pr-choice-423')).toHaveTextContent('Follow up with mobile active-PR selection');
    expect(screen.getAllByRole('menuitemradio')).toHaveLength(2);
    expect(screen.queryByText('Active')).not.toBeInTheDocument();
  });
});
