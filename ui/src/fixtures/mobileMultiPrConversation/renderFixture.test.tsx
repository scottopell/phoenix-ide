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

    expect(screen.getByTestId('mobile-primary-address-feedback')).toHaveTextContent('Address 2 new feedback on PR #423');
    expect(screen.getByRole('button', { name: 'Work details' })).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByText('Workspace diff')).not.toBeInTheDocument();
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

    expect(screen.getByTestId('mobile-primary-address-feedback')).toHaveTextContent('Address 3 new feedback on PR #423');
    expect(screen.getByRole('button', { name: 'Work details' })).toBeInTheDocument();
    expect(screen.queryByText('Associated PRs: 1 open/draft · 1 closed')).not.toBeInTheDocument();
  });

  it('renders closed branch history as structured non-interactive sheet content', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('mixed-branch-work-sheet');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    const closedHistory = screen.getByText('closed').closest('.mobile-work-pr');
    expect(closedHistory).toHaveTextContent('#417 Add durable multi-PR conversation association');
    expect(closedHistory).toHaveTextContent('feature/multi-pr-association → main');
    expect(closedHistory?.tagName).toBe('DIV');
    expect(screen.getByRole('button', { name: /#423 Follow up with mobile active-PR selection/ })).toHaveAttribute('aria-pressed', 'true');
  });

  it('renders the production mobile StateBar with the two-PR chooser open', async () => {
    setMobileViewport();
    const scenario = getMobileMultiPrConversationScenario('chooser-open');
    render(<MobileMultiPrConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['mobileMultiPrConversationFixtureReady']).toBe(scenario.id);
    });

    expect(screen.getByLabelText('Collapse status bar')).toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'Work details' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /#417 Add durable multi-PR conversation association/ })).toHaveTextContent('feature/multi-pr-association → main');
    expect(screen.getByRole('button', { name: /#423 Follow up with mobile active-PR selection/ })).toHaveTextContent('feature/mobile-pr-selector → feature/multi-pr-association');
    expect(screen.getByRole('button', { name: 'Workspace diff' })).toBeInTheDocument();
    expect(screen.queryByTestId('active-pr-selector-trigger')).not.toBeInTheDocument();
  });
});
