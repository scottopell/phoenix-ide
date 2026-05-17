/**
 * ExploreOnboardingBanner
 *
 * One-time inline explainer shown above the input on a user's first Explore
 * conversation. Explains that Explore is read-only and how to reach Work mode.
 * Dismisses on click or after the first message is sent, and never reappears
 * (dismissal persisted in localStorage). Task 08628.
 */
import { useEffect, useState } from 'react';

const DISMISSED_KEY = 'phoenix:explore-onboarding-dismissed';

function alreadyDismissed(): boolean {
  try {
    return localStorage.getItem(DISMISSED_KEY) === '1';
  } catch {
    // Private mode / storage disabled — treat as dismissed so we never nag.
    return true;
  }
}

function persistDismissed(): void {
  try {
    localStorage.setItem(DISMISSED_KEY, '1');
  } catch {
    // Ignore — banner is best-effort onboarding, not load-bearing.
  }
}

interface Props {
  /** Server-computed mode label ("Explore" | "Direct" | "Work" | "Branch"). */
  convModeLabel?: string;
  /** Message count for this conversation; >0 means the first message was sent. */
  messageCount: number;
}

export function ExploreOnboardingBanner({ convModeLabel, messageCount }: Props) {
  const [dismissed, setDismissed] = useState(alreadyDismissed);

  // First message sent — retire the banner permanently.
  useEffect(() => {
    if (messageCount > 0 && !dismissed) {
      persistDismissed();
      setDismissed(true);
    }
  }, [messageCount, dismissed]);

  if (dismissed || convModeLabel !== 'Explore' || messageCount > 0) {
    return null;
  }

  const handleDismiss = () => {
    persistDismissed();
    setDismissed(true);
  };

  return (
    <div className="explore-onboarding-banner" role="note">
      <span className="explore-onboarding-text">
        This is an <strong>Explore</strong> conversation &mdash; the agent can
        read and analyze the codebase but won&rsquo;t make changes. When
        you&rsquo;re ready to modify code, describe what you want and the agent
        will propose a plan for your review.
      </span>
      <button
        className="explore-onboarding-dismiss"
        onClick={handleDismiss}
        title="Dismiss"
        aria-label="Dismiss onboarding tip"
      >
        &times;
      </button>
    </div>
  );
}
