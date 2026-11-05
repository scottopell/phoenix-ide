import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DeploymentObservations } from '../generated/DeploymentObservations';

const { observe } = vi.hoisted(() => ({ observe: vi.fn() }));
vi.mock('../api', () => ({ api: { deploymentObservations: observe } }));

import { OperationalObservations } from './OperationalObservations';

function sample(overrides: Partial<DeploymentObservations> = {}): DeploymentObservations {
  return {
    observed_at: '2026-11-05T10:00:01Z',
    low_space_warning_bytes: 1024 ** 3,
    filesystems: [{
      available_bytes: 2 * 1024 ** 3,
      low_space: false,
      sampled_at: '2026-11-05T10:00:00Z',
      locations: [
        { label: 'Database', path: '/server/db', sampled_path: '/server/db' },
        { label: 'Data directory', path: '/server', sampled_path: '/server' },
      ],
    }],
    measurement_errors: [],
    failures: [
      { boundary: 'ordinary_tool_settlement', count: 0, last_observed_at: null },
      { boundary: 'trace_export', count: 0, last_observed_at: null },
    ],
    local_authority_failure_boundary: null,
    sinks: { structured_queue_drops: 3, trace_failure_coverage: 'otlp_export_results', stdout_configured: true, structured_file_configured: true, fatal_diagnostic_configured: false },
    ...overrides,
  };
}

describe('operational observations', () => {
  beforeEach(() => { observe.mockReset().mockResolvedValue(sample()); });

  it('shows shared filesystem space, server locations, sample age, and bounded counters', async () => {
    render(<OperationalObservations />);
    expect(await screen.findByText(/2.00 GiB available/)).toBeInTheDocument();
    expect(screen.getAllByText(/GiB available/)).toHaveLength(1);
    expect(screen.getByText('/server/db')).toBeInTheDocument();
    expect(screen.getByText('/server')).toBeInTheDocument();
    expect(screen.getByText(/Sample age at refresh:/)).toBeInTheDocument();
    expect(screen.getByText(/Required tool settlement: no failure observed/)).toBeInTheDocument();
    expect(screen.getByText(/Counts reset when the process is replaced/)).toBeInTheDocument();
    expect(screen.getByText(/Structured log queue drops: 3/)).toBeInTheDocument();
    expect(screen.queryByText(/Low space observed/)).not.toBeInTheDocument();
    expect(observe).toHaveBeenCalledTimes(1);
  });

  it.each([
    { direction: 'behind', browserNow: '2026-11-01T10:00:00Z' },
    { direction: 'ahead', browserNow: '2026-12-01T10:00:00Z' },
  ])('uses server observation time when the browser clock is $direction', async ({ browserNow }) => {
    const clock = vi.spyOn(Date, 'now').mockReturnValue(Date.parse(browserNow));
    try {
      observe.mockResolvedValue(sample({ observed_at: '2026-11-05T10:02:00Z' }));
      render(<OperationalObservations />);
      expect(await screen.findByText(/Sample age at refresh: 2m old/)).toBeInTheDocument();
    } finally {
      clock.mockRestore();
    }
  });

  it('shows low-space warning, sampled ancestor, and per-location measurement failure', async () => {
    const data = sample();
    data.filesystems[0]!.available_bytes = 0;
    data.filesystems[0]!.low_space = true;
    data.filesystems[0]!.locations[0]!.sampled_path = '/';
    data.measurement_errors = [{ label: 'Structured log', path: '/logs/private', sampled_path: null, error: 'permission_denied' }];
    observe.mockResolvedValue(data);
    render(<OperationalObservations />);
    expect(await screen.findByText(/Low space observed/)).toBeInTheDocument();
    expect(screen.getByText(/sampled existing ancestor/)).toBeInTheDocument();
    expect(screen.getByText(/space unavailable \(permission denied\)/)).toBeInTheDocument();
    expect(screen.getByText(/no write-success guarantee/)).toBeInTheDocument();
  });

  it('retains the last sample as stale after failed manual refresh and recovers on success', async () => {
    observe.mockResolvedValueOnce(sample()).mockRejectedValueOnce(new Error('connection lost')).mockResolvedValueOnce(sample({ filesystems: [] }));
    render(<OperationalObservations />);
    await screen.findByText(/2.00 GiB available/);
    fireEvent.click(screen.getByRole('button', { name: 'Refresh observations' }));
    expect(await screen.findByText(/Observations stale — connection lost/)).toBeInTheDocument();
    expect(screen.getByText(/2.00 GiB available/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Refresh observations' }));
    await screen.findByRole('button', { name: 'Refresh observations' });
    expect(screen.queryByText(/Observations stale/)).not.toBeInTheDocument();
    expect(screen.queryByText(/GiB available/)).not.toBeInTheDocument();
  });

  it('labels a failed initial observation unavailable', async () => {
    observe.mockRejectedValueOnce(new Error('offline'));
    render(<OperationalObservations />);
    expect(await screen.findByText(/Observations unavailable — offline/)).toBeInTheDocument();
    expect(screen.queryByText(/GiB available/)).not.toBeInTheDocument();
    expect(screen.queryByText(/no failure observed/)).not.toBeInTheDocument();
  });

  it('distinguishes unavailable Datadog observation from a zero trace-error count', async () => {
    const data = sample({ local_authority_failure_boundary: 'ordinary_tool_outcome_settlement' });
    data.sinks.trace_failure_coverage = 'datadog_unavailable';
    data.failures[0]!.count = 1;
    data.failures[0]!.last_observed_at = '2026-11-05T09:00:00Z';
    observe.mockResolvedValue(data);
    render(<OperationalObservations />);
    expect(await screen.findByText(/Datadog exporter configured; failure observation unavailable/)).toBeInTheDocument();
    expect(screen.queryByText(/Trace export: no failure observed/)).not.toBeInTheDocument();
    expect(screen.getByText('ordinary_tool_outcome_settlement')).toBeInTheDocument();
    expect(screen.getByText(/Required tool settlement: 1 observed/)).toBeInTheDocument();
  });
});
