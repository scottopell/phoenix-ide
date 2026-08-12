import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { MobileMultiPrConversationFixture } from './renderFixture';
import {
  getMobileMultiPrConversationScenario,
  mobileMixedBranchPrs,
  mobileMultiPrAssociatedPrs,
  mobileMultiPrSelection,
} from './scenarios';

function setMobileViewport() {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: query === '(max-width: 768px)' || query === '(max-width: 1024px)',
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
    expect(new Set(mobileMultiPrAssociatedPrs.map((pr) => pr.head)).size).toBe(2);
  });

  it('shows PR-specific Work Actions when one of the same two open PRs is active', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('active-pr-actions');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.getByLabelText('Open pull requests')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#417 open feedback in progress \(eyes reaction\)/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#423 open feedback approved \(thumbs-up reaction\) 2 new feedback/ })).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByTestId('mobile-pr-actions')).not.toBeInTheDocument();
  });

  it('shows new comments on an open branch alongside a closed-unmerged sibling branch', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('mixed-branch-history');
    expect(mobileMixedBranchPrs.map((pr) => [pr.head, pr.display_state])).toEqual([
      ['feature/multi-pr-association', 'closed'],
      ['feature/mobile-pr-selector', 'open'],
    ]);
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.getByRole('button', { name: /#423 open 3 new feedback/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /#417/ })).not.toBeInTheDocument();
    expect(screen.queryByTestId('mobile-pr-actions')).not.toBeInTheDocument();
  });

  it('keeps closed branch history out of the open-PR rail', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('mixed-branch-work-sheet');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.queryByRole('button', { name: /#417/ })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#423 open 3 new feedback/ })).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByTestId('mobile-pr-actions')).toHaveTextContent('feature/mobile-pr-selector → main');
  });

  it('opens the StateBar active-PR dialog while in-flight Work Actions are hidden', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('chooser-open');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.getByRole('dialog', { name: /choose active pull request/i })).toBeInTheDocument();
    expect(screen.getByRole('listbox', { name: /active pull request choices/i })).toBeInTheDocument();
    expect(screen.queryByTestId('mobile-pr-actions')).not.toBeInTheDocument();
    expect(screen.getByText(/Locked while the current operation is running/i)).toBeInTheDocument();
  });

  it('opens the idle model-and-effort dialog through its real StateBar trigger', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('model-dialog');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.getByRole('dialog', { name: /model, effort, and speed/i })).toBeInTheDocument();
    expect(screen.getByRole('radiogroup', { name: /select model/i })).toBeInTheDocument();
    expect(screen.getByRole('radiogroup', { name: /select effort/i })).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /Fast Approximately 1.5x speed, increased usage/i })).toHaveAttribute('aria-checked', 'true');
  });
});
