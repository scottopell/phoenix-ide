import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { ErrorBanner } from './ErrorBanner';
import { getErrorPresentation } from '../errorPresentation';

vi.mock('../codexQuota', () => ({
  useCodexQuota: () => null,
}));

describe('ErrorBanner', () => {
  it('shows retry/continue affordance for auth errors', () => {
    const onRetry = vi.fn();
    render(
      <ErrorBanner
        message="Authentication failed"
        error={getErrorPresentation('auth')}
        onRetry={onRetry}
        onDismiss={vi.fn()}
      />,
    );

    const retry = screen.getByRole('button', { name: /retry.*continue/i });
    fireEvent.click(retry);
    expect(onRetry).toHaveBeenCalledOnce();
    expect(screen.queryByText(/start a new conversation/i)).not.toBeInTheDocument();
  });

  it('shows retry/continue affordance for usage limits (window resets on a clock boundary)', () => {
    const onRetry = vi.fn();
    render(
      <ErrorBanner
        message="You've hit your usage limit. Try again later."
        error={getErrorPresentation('usage_limit_reached')}
        onRetry={onRetry}
        onDismiss={vi.fn()}
      />,
    );

    const retry = screen.getByRole('button', { name: /retry.*continue/i });
    fireEvent.click(retry);
    expect(onRetry).toHaveBeenCalledOnce();
    expect(screen.queryByText(/start a new conversation/i)).not.toBeInTheDocument();
  });

  it('offers neither Retry nor Dismiss for a non-resumable error', () => {
    render(
      <ErrorBanner
        message="Bad request"
        error={getErrorPresentation('invalid_request')}
        onRetry={vi.fn()}
        onDismiss={vi.fn()}
      />,
    );

    expect(screen.queryByRole('button', { name: /retry.*continue/i })).not.toBeInTheDocument();
    // Dismiss -> Idle would reopen the resume path the policy denies, so it is
    // not offered for non-resumable errors.
    expect(screen.queryByRole('button', { name: /dismiss/i })).not.toBeInTheDocument();
    expect(screen.getByText(/start a new conversation/i)).toBeInTheDocument();
  });

  it('still shows retry/continue affordance for transient errors', () => {
    render(
      <ErrorBanner
        message="Server error"
        error={getErrorPresentation('server_error')}
        onRetry={vi.fn()}
        onDismiss={vi.fn()}
      />,
    );

    expect(screen.getByRole('button', { name: /retry.*continue/i })).toBeInTheDocument();
  });
});
