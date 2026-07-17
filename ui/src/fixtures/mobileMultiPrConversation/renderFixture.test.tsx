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
    expect(screen.getByRole('button', { name: /#417 open/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#423 open 2 new feedback/ })).toHaveAttribute('aria-expanded', 'false');
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

  it('expands the active PR into hero and secondary action rows', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('chooser-open');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.getByTestId('mobile-primary-address-feedback')).toHaveTextContent('Address feedback · 2 new');
    expect(screen.getByRole('button', { name: 'PR #423 diff' })).toHaveTextContent('PR diff');
    expect(screen.getByRole('button', { name: 'Workspace diff' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Clean up' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Abandon\./ })).toHaveClass('mobile-pr-action--danger');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
