import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ExploreOnboardingBanner } from './ExploreOnboardingBanner';

const DISMISSED_KEY = 'phoenix:explore-onboarding-dismissed';

describe('ExploreOnboardingBanner', () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => localStorage.clear());

  it('shows on a fresh Explore conversation', () => {
    render(<ExploreOnboardingBanner convModeLabel="Explore" messageCount={0} />);
    expect(screen.getByRole('note')).toBeInTheDocument();
  });

  it('does not show in Direct/Work/Branch conversations', () => {
    for (const label of ['Direct', 'Work', 'Branch']) {
      const { container, unmount } = render(
        <ExploreOnboardingBanner convModeLabel={label} messageCount={0} />,
      );
      expect(container).toBeEmptyDOMElement();
      unmount();
    }
  });

  it('retires after the first message is sent while it was on screen', () => {
    const { container, rerender } = render(
      <ExploreOnboardingBanner convModeLabel="Explore" messageCount={0} />,
    );
    expect(screen.getByRole('note')).toBeInTheDocument();

    // First message sent in this conversation -> banner retires for good.
    rerender(<ExploreOnboardingBanner convModeLabel="Explore" messageCount={1} />);
    expect(container).toBeEmptyDOMElement();
    expect(localStorage.getItem(DISMISSED_KEY)).toBe('1');
  });

  it('does NOT persist dismissal for an existing Explore conv it never showed in', () => {
    // User lands on an old Explore conversation that already has messages
    // (shared link / resumed session): banner was never shown, so the
    // one-time dismissal must be preserved for their first fresh Explore.
    const { container } = render(
      <ExploreOnboardingBanner convModeLabel="Explore" messageCount={5} />,
    );
    expect(container).toBeEmptyDOMElement();
    expect(localStorage.getItem(DISMISSED_KEY)).toBeNull();
  });

  it('does NOT persist dismissal when only non-Explore conversations are seen', () => {
    const { container } = render(
      <ExploreOnboardingBanner convModeLabel="Work" messageCount={3} />,
    );
    expect(container).toBeEmptyDOMElement();
    expect(localStorage.getItem(DISMISSED_KEY)).toBeNull();
  });

  it('dismisses on click and does not reappear', () => {
    const { container, rerender } = render(
      <ExploreOnboardingBanner convModeLabel="Explore" messageCount={0} />,
    );
    fireEvent.click(screen.getByLabelText('Dismiss onboarding tip'));
    expect(container).toBeEmptyDOMElement();
    expect(localStorage.getItem(DISMISSED_KEY)).toBe('1');

    // A brand-new Explore conversation must not show it again.
    rerender(<ExploreOnboardingBanner convModeLabel="Explore" messageCount={0} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('stays hidden when dismissal was already persisted', () => {
    localStorage.setItem(DISMISSED_KEY, '1');
    const { container } = render(
      <ExploreOnboardingBanner convModeLabel="Explore" messageCount={0} />,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
