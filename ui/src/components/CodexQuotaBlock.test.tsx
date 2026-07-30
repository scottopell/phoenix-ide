import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { QuotaDetails } from '../sseSchemas';
import { CodexQuotaBlock } from './CodexQuotaBlock';

function quota(overrides: Partial<QuotaDetails>): QuotaDetails {
  return {
    plan_type: 'plus',
    resets_at: null,
    limit_id: 'codex',
    limit_name: null,
    primary: null,
    secondary: null,
    additional_limits: [],
    credits: null,
    individual_limit: null,
    promo_message: null,
    rate_limit_reached_type: null,
    ...overrides,
  };
}

describe('CodexQuotaBlock', () => {
  it('labels the current five-hour quota window', () => {
    render(
      <CodexQuotaBlock
        quota={quota({
          primary: {
            used_percent: 3,
            window_minutes: 300,
            resets_at: 1_800_000_000,
          },
        })}
      />,
    );

    expect(screen.getByText('5-hour')).toBeInTheDocument();
    expect(screen.getByText('3%')).toBeInTheDocument();
  });

  it('shows additional named quota families', () => {
    render(
      <CodexQuotaBlock
        quota={quota({
          additional_limits: [{
            limit_name: 'Review',
            primary: { used_percent: 12, window_minutes: 300, resets_at: 1_800_000_000 },
            secondary: null,
          }],
        })}
      />,
    );

    expect(screen.getByText('Review · 5-hour')).toBeInTheDocument();
    expect(screen.getByText('12%')).toBeInTheDocument();
  });

  it('shows an individual spend-control limit without standard windows', () => {
    render(
      <CodexQuotaBlock
        quota={quota({
          individual_limit: {
            limit: '100.00',
            used: '25.00',
            remaining_percent: 75,
            resets_at: 1_800_000_000,
          },
        })}
      />,
    );

    expect(screen.getByText(/Individual limit: 25\.00 \/ 100\.00/)).toBeInTheDocument();
    expect(screen.getByText(/75% remaining/)).toBeInTheDocument();
  });

  it('shows weekly quota without claiming unavailable credits are depleted', () => {
    const { container } = render(
      <CodexQuotaBlock
        quota={quota({
          secondary: {
            used_percent: 4,
            window_minutes: 10_080,
            resets_at: 1_800_000_000,
          },
          credits: { has_credits: false, unlimited: false, balance: null },
        })}
      />,
    );

    expect(screen.getByText('Weekly')).toBeInTheDocument();
    expect(screen.getByText('4%')).toBeInTheDocument();
    expect(screen.queryByText(/credits/i)).not.toBeInTheDocument();
    expect(container.querySelector('.settings-codex-quota__fill')).toHaveStyle({ width: '4%' });
  });

  it('renders nothing when the only data says credit tracking is unavailable', () => {
    const { container } = render(
      <CodexQuotaBlock
        quota={quota({
          credits: { has_credits: false, unlimited: false, balance: null },
        })}
      />,
    );

    expect(container).toBeEmptyDOMElement();
  });

  it('shows available when credits are enabled but the balance is hidden', () => {
    render(
      <CodexQuotaBlock
        quota={quota({
          credits: { has_credits: true, unlimited: false, balance: null },
        })}
      />,
    );

    expect(screen.getByText('Credits: Available')).toBeInTheDocument();
  });

  it('shows a finite credit balance', () => {
    render(
      <CodexQuotaBlock
        quota={quota({
          credits: { has_credits: true, unlimited: false, balance: ' 42.5 ' },
        })}
      />,
    );

    expect(screen.getByText('Credits: 42.5')).toBeInTheDocument();
  });

  it.each([
    ['rate_limit_reached', 'Usage limit reached'],
    ['workspace_owner_usage_limit_reached', 'Workspace usage limit reached'],
    ['workspace_member_usage_limit_reached', 'Member usage limit reached'],
  ] as const)('renders explicit non-credit exhaustion %s', (type, message) => {
    render(<CodexQuotaBlock quota={quota({ rate_limit_reached_type: type })} />);
    expect(screen.getByText(message)).toBeInTheDocument();
  });

  it('shows depletion only when Codex explicitly reports it', () => {
    render(
      <CodexQuotaBlock
        quota={quota({
          credits: { has_credits: false, unlimited: false, balance: null },
          rate_limit_reached_type: 'workspace_member_credits_depleted',
        })}
      />,
    );

    expect(screen.getByText('No credits remaining')).toBeInTheDocument();
  });

  it('shows unlimited independently of the has-credits flag', () => {
    render(
      <CodexQuotaBlock
        quota={quota({
          credits: { has_credits: false, unlimited: true, balance: null },
        })}
      />,
    );

    expect(screen.getByText('Credits: Unlimited')).toBeInTheDocument();
  });
});
