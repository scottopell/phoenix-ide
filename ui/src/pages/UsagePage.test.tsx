import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { UsagePage } from './UsagePage';
import { api } from '../api';
import type { UsageOverview } from '../generated/UsageOverview';

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: {
      ...actual.api,
      usageOverview: vi.fn(),
      usageConversationDetail: vi.fn(),
      analyticsTrajectoryExport: vi.fn(),
    },
  };
});

const overview: UsageOverview = {
  generated_at: '2025-08-11T00:00:00+00:00',
  windows: {
    today: { input_tokens: 0, output_tokens: 0, cache_write_tokens: 0, cache_read_tokens: 0, total_tokens: 0, turns: 1, cost: { estimated_usd: 0, pricing_known: true, unknown_turns: 0 } },
    week: { input_tokens: 0, output_tokens: 0, cache_write_tokens: 0, cache_read_tokens: 0, total_tokens: 0, turns: 1, cost: { estimated_usd: 0, pricing_known: true, unknown_turns: 0 } },
    month: { input_tokens: 0, output_tokens: 0, cache_write_tokens: 0, cache_read_tokens: 0, total_tokens: 0, turns: 1, cost: { estimated_usd: 0, pricing_known: true, unknown_turns: 0 } },
    all: { input_tokens: 0, output_tokens: 0, cache_write_tokens: 0, cache_read_tokens: 0, total_tokens: 1, turns: 1, cost: { estimated_usd: 0, pricing_known: true, unknown_turns: 0 } },
  },
  daily: [{ day: '2025-08-11', totals: { input_tokens: 0, output_tokens: 0, cache_write_tokens: 0, cache_read_tokens: 0, total_tokens: 0, turns: 1, cost: { estimated_usd: 0, pricing_known: true, unknown_turns: 0 } } }],
  by_model: [],
  by_provider: [],
  by_project: [],
  conversations: [],
  turn_token_histogram: [],
  ttft: {
    window_days: 14,
    sample_count: 3,
    no_token_success_count: 1,
    cancellation_count: 1,
    error_count: 1,
    provider_rows: [
      {
        attempt_scope: 'first_attempt',
        provider: 'Anthropic',
        model: null,
        transport: null,
        sample_count: 2,
        no_token_success_count: 1,
        cancellation_count: 1,
        error_count: 0,
        percentiles: { p50_ms: 800, p75_ms: 1200, p90_ms: 1200, p95_ms: 1200, p99_ms: 1200 },
        thresholds: [
          { threshold_ms: 2000, exceeded_count: 0, exceeded_rate: 0 },
          { threshold_ms: 5000, exceeded_count: 0, exceeded_rate: 0 },
          { threshold_ms: 10000, exceeded_count: 0, exceeded_rate: 0 },
          { threshold_ms: 30000, exceeded_count: 0, exceeded_rate: 0 },
        ],
      },
      {
        attempt_scope: 'retry',
        provider: 'OpenAI',
        model: null,
        transport: null,
        sample_count: 1,
        no_token_success_count: 0,
        cancellation_count: 0,
        error_count: 1,
        percentiles: { p50_ms: 6000, p75_ms: 6000, p90_ms: 6000, p95_ms: 6000, p99_ms: 6000 },
        thresholds: [
          { threshold_ms: 2000, exceeded_count: 1, exceeded_rate: 1 },
          { threshold_ms: 5000, exceeded_count: 1, exceeded_rate: 1 },
          { threshold_ms: 10000, exceeded_count: 0, exceeded_rate: 0 },
          { threshold_ms: 30000, exceeded_count: 0, exceeded_rate: 0 },
        ],
      },
    ],
    grouped_rows: [
      {
        attempt_scope: 'first_attempt',
        provider: 'Anthropic',
        model: 'claude-sonnet-5',
        transport: 'http_sse',
        sample_count: 2,
        no_token_success_count: 1,
        cancellation_count: 1,
        error_count: 0,
        percentiles: { p50_ms: 800, p75_ms: 1200, p90_ms: 1200, p95_ms: 1200, p99_ms: 1200 },
        thresholds: [
          { threshold_ms: 2000, exceeded_count: 0, exceeded_rate: 0 },
          { threshold_ms: 5000, exceeded_count: 0, exceeded_rate: 0 },
          { threshold_ms: 10000, exceeded_count: 0, exceeded_rate: 0 },
          { threshold_ms: 30000, exceeded_count: 0, exceeded_rate: 0 },
        ],
      },
    ],
    daily_trend: [
      { day: '2025-08-10', attempt_scope: 'first_attempt', sample_count: 2, no_token_success_count: 1, cancellation_count: 1, error_count: 0, percentiles: { p50_ms: 800, p75_ms: 1200, p90_ms: 1200, p95_ms: 1200, p99_ms: 1200 } },
      { day: '2025-08-10', attempt_scope: 'retry', sample_count: 1, no_token_success_count: 0, cancellation_count: 0, error_count: 1, percentiles: { p50_ms: 6000, p75_ms: 6000, p90_ms: 6000, p95_ms: 6000, p99_ms: 6000 } },
    ],
  },
};

describe('UsagePage TTFT hero', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders the TTFT hero analytics', async () => {
    vi.mocked(api.usageOverview).mockResolvedValue(overview);

    render(
      <MemoryRouter>
        <UsagePage />
      </MemoryRouter>,
    );

    expect(screen.getByText('Loading…')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText('Time to first token')).toBeInTheDocument());
    expect(screen.getByText(/Provider efficiency over the last 14 days/)).toBeInTheDocument();
    expect(screen.getByText('TTFT samples')).toBeInTheDocument();
    expect(screen.getByText('Cancellations')).toBeInTheDocument();
    expect(screen.getByText('Best-covered first-attempt provider')).toBeInTheDocument();
    expect(screen.getByText('Retry behavior')).toBeInTheDocument();
    expect(screen.getByText('Provider / model / transport comparison')).toBeInTheDocument();
    expect(screen.getAllByText('First attempt').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Retry').length).toBeGreaterThan(0);
    expect(screen.getByText('claude-sonnet-5')).toBeInTheDocument();
  });

  it('renders TTFT observations when token usage is empty', async () => {
    vi.mocked(api.usageOverview).mockResolvedValue({
      ...overview,
      windows: {
        ...overview.windows,
        all: { ...overview.windows.all, total_tokens: 0, turns: 0 },
      },
    });

    render(
      <MemoryRouter>
        <UsagePage />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('Time to first token')).toBeInTheDocument());
    expect(screen.getByText('No token usage recorded yet.')).toBeInTheDocument();
    expect(screen.getByText('TTFT samples')).toBeInTheDocument();
    expect(screen.getByText('No-token success')).toBeInTheDocument();
  });

  it('shows request failures', async () => {
    vi.mocked(api.usageOverview).mockRejectedValue(new Error('boom'));

    render(
      <MemoryRouter>
        <UsagePage />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('boom')).toBeInTheDocument());
  });
});
