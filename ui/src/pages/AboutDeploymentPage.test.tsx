import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import type { AboutResourcesSnapshot } from '../generated/AboutResourcesSnapshot';
import type { DeploymentInfo } from '../generated/DeploymentInfo';
import type { DeploymentDiskInfo } from '../generated/DeploymentDiskInfo';
import type { ReleaseUpdateSnapshot } from '../generated/ReleaseUpdateSnapshot';
import type { SqliteWorkloadReportResponse } from '../generated/SqliteWorkloadReportResponse';
import type { SqliteReportCategory } from '../generated/SqliteReportCategory';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    deploymentInfo: vi.fn(),
    deploymentDiskInfo: vi.fn(),
    deploymentResources: vi.fn(),
    deploymentSqliteWorkload: vi.fn(),
    cleanupManagedWorktree: vi.fn(),
    revealPath: vi.fn(),
    releaseUpdateSnapshot: vi.fn(),
    releaseUpdateTransaction: vi.fn(),
    approveReleaseUpdate: vi.fn(),
  },
}));

vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return {
    ...actual,
    api: apiMock,
  };
});

import { AboutDeploymentPage, appendResourceHistory, computeResourceRollups } from './AboutDeploymentPage';

function deployment(overrides: Partial<DeploymentInfo> = {}): DeploymentInfo {
  return {
    build: {
      version: '0.1.0',
      git_sha: 'abc123',
      started_at: '2026-06-01T00:00:00Z',
      uptime_seconds: 10,
    },
    network: {
      bind_address: '127.0.0.1:8031',
      socket_activated: false,
      tls: {
        enabled: false,
        mode: null,
        cert_path: null,
        key_path: null,
        ca_cert_path: null,
        hosts: [],
      },
    },
    log: { stdout: true, file: null, fatal_file: null },
    instance_id: null,
    installation_ownership: { kind: 'development' },
    local_access: false,
    sampled_at: '2026-06-01T00:00:01Z',
    ...overrides,
  };
}

function deploymentDisk(overrides: Partial<DeploymentDiskInfo> = {}): DeploymentDiskInfo {
  return {
    disk: [],
    managed_worktrees: [],
    sampled_at: '2026-06-01T00:00:02Z',
    ...overrides,
  };
}

function resourcesSnapshot(overrides: Partial<AboutResourcesSnapshot> = {}): AboutResourcesSnapshot {
  return {
    sampled_at: '2026-06-01T00:00:03Z',
    host: {
      logical_cpu_count: 8,
      cpu_busy_percent: 25,
      cpu_system_percent: 7,
      cpu_idle_percent: 75,
      total_memory_bytes: 16 * 1024,
      available_memory_bytes: 6 * 1024,
      used_memory_bytes: 10 * 1024,
      load_average_one: 0.8,
      load_average_five: 0.9,
      load_average_fifteen: 1.2,
    },
    managed_total: {
      cpu_percent: 24,
      memory_bytes: 3 * 1024,
      process_count: 3,
      deduplicated_pid_count: 2,
    },
    categories: [
      {
        kind: 'api',
        label: 'API',
        attribution: 'available',
        reason: null,
        totals: {
          cpu_percent: 12,
          memory_bytes: 1024,
          process_count: 1,
          deduplicated_pid_count: 1,
        },
        processes: [
          {
            name: 'phoenix-api',
            category: 'api',
            pid: 123,
            cpu_percent: 12,
            memory_bytes: 1024,
            thread_count: 8,
            cpu_time_seconds: 90,
          },
        ],
      },
      {
        kind: 'browser',
        label: 'Browser',
        attribution: 'available',
        reason: null,
        totals: {
          cpu_percent: 12,
          memory_bytes: 2 * 1024,
          process_count: 2,
          deduplicated_pid_count: 1,
        },
        processes: [
          {
            name: 'chromium',
            category: 'browser',
            pid: 456,
            cpu_percent: 12,
            memory_bytes: 2 * 1024,
            thread_count: 14,
            cpu_time_seconds: 120,
          },
        ],
      },
      {
        kind: 'mcp',
        label: 'MCP',
        attribution: 'unavailable',
        reason: 'MCP disabled on this deployment',
        totals: {
          cpu_percent: null,
          memory_bytes: null,
          process_count: 0,
          deduplicated_pid_count: 0,
        },
        processes: [],
      },
    ],
    ...overrides,
  };
}

function sqliteBaselineCategory(category: SqliteReportCategory, label: string) {
  return {
    category,
    label,
    baseline_statement_count: 0,
    native_statement_latency: { sample_count: 0, total_ms: 0, avg_ms: null, p50_upper_bound_ms: null, p95_upper_bound_ms: null, p99_upper_bound_ms: null },
  };
}

function sqliteCategory(category: SqliteReportCategory, label: string) {
  return {
    category,
    label,
    typed_operation_count: 0,
    typed_latency: { sample_count: 0, total_ms: 0, avg_ms: null, p50_upper_bound_ms: null, p95_upper_bound_ms: null, p99_upper_bound_ms: null },
    writer_occupancy_percent: 0,
    peak_concurrency: 0,
    pool_wait: { sample_count: 0, total_ms: 0, avg_ms: null, p50_upper_bound_ms: null, p95_upper_bound_ms: null, p99_upper_bound_ms: null },
    admission_wait: { sample_count: 0, total_ms: 0, avg_ms: null, p50_upper_bound_ms: null, p95_upper_bound_ms: null, p99_upper_bound_ms: null },
    retries: null,
    failures: { busy: 0, locked: 0, pool_timeout: 0, other_timeout: 0, other_failure: 0, abandoned: 0 },
  };
}

function sqliteReadCategory(category: SqliteReportCategory, label: string) {
  return {
    category,
    label,
    typed_operation_count: 0,
    typed_latency: { sample_count: 0, total_ms: 0, avg_ms: null, p50_upper_bound_ms: null, p95_upper_bound_ms: null, p99_upper_bound_ms: null },
    total_profiled_read_execution_ms: 0,
    profiled_statement_latency: { sample_count: 0, total_ms: 0, avg_ms: null, p50_upper_bound_ms: null, p95_upper_bound_ms: null, p99_upper_bound_ms: null },
    peak_concurrency: 0,
    pool_wait: { sample_count: 0, total_ms: 0, avg_ms: null, p50_upper_bound_ms: null, p95_upper_bound_ms: null, p99_upper_bound_ms: null },
    retries: null,
    failures: { busy: 0, locked: 0, pool_timeout: 0, other_timeout: 0, other_failure: 0, abandoned: 0 },
  };
}

function sqliteReadFamily() {
  return {
    family: 'active_list' as const,
    label: 'Active conversation list',
    attempt_count: 0,
    success_count: 0,
    failure_count: 0,
    abandoned_count: 0,
    logical_elapsed: { sample_count: 0, total_ms: 0, avg_ms: null, p50_upper_bound_ms: null, p95_upper_bound_ms: null, p99_upper_bound_ms: null },
  };
}

function sqliteReport(overrides: Partial<SqliteWorkloadReportResponse> = {}): SqliteWorkloadReportResponse {
  return {
    sampled_at: '2026-06-01T00:00:05Z',
    window: 'one_hour',
    bucket_count: 2,
    restart_truncated: true,
    process_started_at: '2026-06-01T00:00:00Z',
    process_uptime_seconds: 125,
    covered_uptime_seconds: 125,
    covered_uptime_micros: 125_000_000,
    coverage: { bucket_count: 2, fully_covered: false, label: '2m 5s covered across 2 buckets; requested 60m' },
    classification: { typed_outcome_count: 0,
      typed_other_outcome_count: 0,
      typed_other_outcome_share_percent: null,
      baseline_statement_count: 0,
      baseline_other_statement_count: 0,
      baseline_other_statement_share_percent: null,
      abandoned_count: 0,
      classification_gap_count: 0,
      writer_occupancy_gap_count: 0 },
    baseline_categories: [sqliteBaselineCategory('message_persistence', 'Message persistence')],
    writer_categories: [sqliteCategory('message_persistence', 'Message persistence')],
    reads: [sqliteReadCategory('message_persistence', 'Message persistence')],
    read_families: [sqliteReadFamily()],
    ...overrides,
  };
}

function LocationProbe() {
  return <output aria-label="Current route">{useLocation().pathname}</output>;
}

function deferredReleaseSnapshot() {
  let resolve!: (snapshot: ReleaseUpdateSnapshot) => void;
  const promise = new Promise<ReleaseUpdateSnapshot>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function renderPage(
  info: DeploymentInfo,
  disk: DeploymentDiskInfo = deploymentDisk(),
  sqlite: SqliteWorkloadReportResponse = sqliteReport(),
) {
  apiMock.deploymentInfo.mockResolvedValue(info);
  apiMock.deploymentDiskInfo.mockResolvedValue(disk);
  apiMock.deploymentResources.mockResolvedValue(resourcesSnapshot());
  apiMock.deploymentSqliteWorkload.mockResolvedValue(sqlite);
  return render(
    <MemoryRouter>
      <AboutDeploymentPage />
    </MemoryRouter>,
  );
}

describe('AboutDeploymentPage resource helpers', () => {
  it('bounds history to the last five minutes and deduplicates by sample time', () => {
    const older = resourcesSnapshot({ sampled_at: '2026-06-01T00:00:00Z', managed_total: { cpu_percent: 10, memory_bytes: 1000, process_count: 1, deduplicated_pid_count: 1 } });
    const newer = resourcesSnapshot({ sampled_at: '2026-06-01T00:04:30Z', managed_total: { cpu_percent: 20, memory_bytes: 2000, process_count: 1, deduplicated_pid_count: 1 } });
    const latest = resourcesSnapshot({ sampled_at: '2026-06-01T00:05:01Z', managed_total: { cpu_percent: 30, memory_bytes: 3000, process_count: 1, deduplicated_pid_count: 1 } });

    const one = appendResourceHistory([], older);
    const two = appendResourceHistory(one, newer);
    const three = appendResourceHistory(two, latest);
    const deduped = appendResourceHistory(three, latest);

    expect(deduped).toHaveLength(2);
    expect(deduped.map((entry) => entry.sampledAt)).toEqual(['2026-06-01T00:04:30Z', '2026-06-01T00:05:01Z']);
  });

  it('bases retention on server sample time rather than the browser clock', () => {
    const first = resourcesSnapshot({ sampled_at: '2026-06-01T00:00:00Z' });
    const latest = resourcesSnapshot({ sampled_at: '2026-06-01T00:05:01Z' });

    const history = appendResourceHistory(appendResourceHistory([], first), latest);

    expect(history.map((entry) => entry.sampledAt)).toEqual(['2026-06-01T00:05:01Z']);
  });

  it('computes current, average, and peak rollups from history', () => {
    const rollups = computeResourceRollups([
      { sampledAt: '2026-06-01T00:00:00Z', timeLabel: '00:00:00', cpuPercent: 10, memoryBytes: 100 },
      { sampledAt: '2026-06-01T00:00:01Z', timeLabel: '00:00:01', cpuPercent: 20, memoryBytes: 200 },
      { sampledAt: '2026-06-01T00:00:02Z', timeLabel: '00:00:02', cpuPercent: 30, memoryBytes: 400 },
    ]);

    expect(rollups.currentCpuPercent).toBe(30);
    expect(rollups.averageCpuPercent).toBe(20);
    expect(rollups.peakCpuPercent).toBe(30);
    expect(rollups.currentMemoryBytes).toBe(400);
    expect(rollups.averageMemoryBytes).toBeCloseTo(700 / 3);
    expect(rollups.peakMemoryBytes).toBe(400);
  });
});

describe('AboutDeploymentPage disk usage health', () => {
  beforeEach(() => {
    apiMock.deploymentInfo.mockReset();
    apiMock.deploymentDiskInfo.mockReset();
    apiMock.deploymentResources.mockReset();
    apiMock.deploymentSqliteWorkload.mockReset().mockResolvedValue(sqliteReport());
    apiMock.cleanupManagedWorktree.mockReset();
    apiMock.revealPath.mockReset();
    apiMock.releaseUpdateTransaction.mockReset().mockResolvedValue({ kind: 'none' });
    apiMock.releaseUpdateSnapshot.mockReset().mockResolvedValue({
      installation_ownership: { kind: 'development' },
      current_version: '0.1.0',
      current_git_sha: 'abc123',
      preview: { kind: 'unavailable', reason: 'not checked' },
      transaction: { kind: 'none' },
      authority: { kind: 'not_production' },
      sampled_at: '2026-06-01T00:00:04Z',
    });
    apiMock.approveReleaseUpdate.mockReset();
    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'visible' });
  });

  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it('loads SQLite workload and switches fixed report windows', async () => {
    apiMock.deploymentSqliteWorkload
      .mockResolvedValueOnce(sqliteReport({
        read_families: [{
          ...sqliteReadFamily(),
          attempt_count: 3,
          success_count: 2,
          failure_count: 1,
          logical_elapsed: { sample_count: 3, total_ms: 42, avg_ms: 14, p50_upper_bound_ms: 19, p95_upper_bound_ms: 49, p99_upper_bound_ms: 49 },
        }],
      }))
      .mockResolvedValueOnce(sqliteReport({ window: 'six_hours' }));

    renderPage(deployment());

    expect(await screen.findByRole('heading', { name: 'SQLite workload' })).toBeInTheDocument();
    expect(screen.getByText('Restart truncated')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Logical reads by source-defined family' })).toBeInTheDocument();
    expect(screen.getByText('success 2 · fail 1 · abandoned 0')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '6h' }));
    expect(apiMock.deploymentSqliteWorkload).toHaveBeenLastCalledWith('six_hours');
    await screen.findAllByText(/No (native baseline|instrumented contention|native read load)/);
  });

  it('clears the prior window report while a newly selected window loads', async () => {
    const sixHours = deferred<SqliteWorkloadReportResponse>();
    apiMock.deploymentSqliteWorkload
      .mockResolvedValueOnce(sqliteReport({ coverage: { bucket_count: 2, fully_covered: false, label: 'one-hour-only label' } }))
      .mockImplementationOnce(() => sixHours.promise);
    renderPage(deployment());
    expect(await screen.findByText('one-hour-only label')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '6h' }));
    expect(screen.queryByText('one-hour-only label')).not.toBeInTheDocument();
    expect(screen.getByText('Loading SQLite report…')).toBeInTheDocument();
    await act(async () => {
      sixHours.resolve(sqliteReport({ window: 'six_hours', coverage: { bucket_count: 6, fully_covered: false, label: 'six-hour label' } }));
    });
    expect(await screen.findByText('six-hour label')).toBeInTheDocument();
  });

  it('renders zero coverage as warm-up unavailable instead of normal zero rows', async () => {
    renderPage(deployment(), deploymentDisk(), sqliteReport({
      covered_uptime_micros: 0,
      coverage: { bucket_count: 0, fully_covered: false, label: '0s covered across 0 buckets; requested 60m' },
    }));
    expect(await screen.findByText('SQLite workload coverage is warming up; no covered interval is available yet.')).toBeInTheDocument();
    expect(screen.queryByText('Native statement baseline by category')).not.toBeInTheDocument();
  });

  it('keeps prior SQLite values visible and labels a failed refresh stale', async () => {
    apiMock.deploymentSqliteWorkload
      .mockResolvedValueOnce(sqliteReport())
      .mockRejectedValueOnce(new Error('collector unavailable'));

    renderPage(deployment());
    await screen.findByText('Restart truncated');
    fireEvent.click(screen.getByRole('button', { name: 'Refresh SQLite report' }));

    expect(await screen.findByText('SQLite report stale — collector unavailable')).toBeInTheDocument();
    expect(screen.getByText('Restart truncated')).toBeInTheDocument();
  });

  it('ignores an older SQLite response that resolves after a newer window selection', async () => {
    let resolveOneHour: ((value: SqliteWorkloadReportResponse) => void) | undefined;
    let resolveSixHours: ((value: SqliteWorkloadReportResponse) => void) | undefined;
    renderPage(deployment());
    apiMock.deploymentSqliteWorkload.mockReset().mockImplementation((window: string) => {
      if (window === 'one_hour') {
        return new Promise<SqliteWorkloadReportResponse>((resolve) => { resolveOneHour = resolve; });
      }
      if (window === 'six_hours') {
        return new Promise<SqliteWorkloadReportResponse>((resolve) => { resolveSixHours = resolve; });
      }
      return Promise.resolve(sqliteReport({ window: window as SqliteWorkloadReportResponse['window'] }));
    });
    expect(await screen.findByRole('heading', { name: 'SQLite workload' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Refresh SQLite report' }));
    fireEvent.click(screen.getByRole('button', { name: '6h' }));
    await waitFor(() => {
      expect(apiMock.deploymentSqliteWorkload).toHaveBeenLastCalledWith('six_hours');
    });

    const newer = resolveSixHours;
    const older = resolveOneHour;
    if (!newer || !older) throw new Error('expected both SQLite requests to be pending');

    await act(async () => {
      newer(sqliteReport({ window: 'six_hours', coverage: { bucket_count: 12, fully_covered: true, label: '6h covered across 12 buckets; requested 360m' } }));
      await Promise.resolve();
    });
    expect(screen.getByText('6h covered across 12 buckets; requested 360m')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '6h' })).toHaveClass('about-sqlite-window-button--active');

    await act(async () => {
      older(sqliteReport({ window: 'one_hour', coverage: { bucket_count: 2, fully_covered: false, label: '1h stale response should be ignored' } }));
      await Promise.resolve();
    });

    expect(screen.queryByText('1h stale response should be ignored')).not.toBeInTheDocument();
    expect(screen.getByText('6h covered across 12 buckets; requested 360m')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '6h' })).toHaveClass('about-sqlite-window-button--active');
  });

  it('shows a no-samples state when the API returns category rows with zero counts', async () => {
    apiMock.deploymentSqliteWorkload.mockReset().mockResolvedValue(sqliteReport());

    renderPage(deployment());

    expect(await screen.findAllByText(/No (native baseline|instrumented contention|native read load)/)).toHaveLength(3);
    expect(screen.getAllByText('Message persistence')).toHaveLength(3);
  });

  it('treats baseline-only SQLite reports as sampled', async () => {
    renderPage(deployment(), deploymentDisk(), sqliteReport({
      classification: { ...sqliteReport().classification, baseline_statement_count: 1 },
    }));
    await screen.findByRole('heading', { name: 'SQLite workload' });
    expect(screen.queryAllByText('No SQLite samples captured for this window yet.')).toHaveLength(0);
  });

  it('treats occupancy-only SQLite reports as sampled', async () => {
    const report = sqliteReport();
    renderPage(deployment(), deploymentDisk(), sqliteReport({
      writer_categories: report.writer_categories.map((row, index) => index === 0
        ? { ...row, writer_occupancy_percent: 0.5, peak_concurrency: 1 }
        : row),
    }));
    await screen.findByRole('heading', { name: 'SQLite workload' });
    expect(screen.queryAllByText('No SQLite samples captured for this window yet.')).toHaveLength(0);
  });

  it('keeps mixed native, typed, and occupancy authorities separate', async () => {
    const report = sqliteReport();
    renderPage(deployment(), deploymentDisk(), sqliteReport({
      classification: {
        ...report.classification,
        typed_outcome_count: 1,
        baseline_statement_count: 1,
      },
      baseline_categories: report.baseline_categories.map((row, index) => index === 0
        ? { ...row, baseline_statement_count: 1, native_statement_latency: { sample_count: 1, total_ms: 12, avg_ms: 12, p50_upper_bound_ms: 20, p95_upper_bound_ms: 20, p99_upper_bound_ms: 20 } }
        : row),
      writer_categories: report.writer_categories.map((row, index) => index === 0
        ? { ...row, typed_operation_count: 1, typed_latency: { sample_count: 1, total_ms: 45, avg_ms: 45, p50_upper_bound_ms: 50, p95_upper_bound_ms: 50, p99_upper_bound_ms: 50 }, writer_occupancy_percent: 2, peak_concurrency: 1 }
        : row),
    }));
    expect(await screen.findByText(/avg 12 ms · p95≤ 20 ms/)).toBeInTheDocument();
    expect(screen.getByText('1 native statements')).toBeInTheDocument();
    expect(screen.getByText(/avg 45 ms · n=1 typed writes/)).toBeInTheDocument();
    expect(screen.queryByText(/Average connection time/)).not.toBeInTheDocument();
  });

  it('labels alignment-shortened coverage without claiming the full window', async () => {
    renderPage(deployment(), deploymentDisk(), sqliteReport({
      restart_truncated: false,
      coverage: { bucket_count: 59, fully_covered: false, label: '59m covered across 59 buckets; requested 60m' },
    }));
    expect(await screen.findByText('Alignment-shortened coverage')).toBeInTheDocument();
    expect(screen.queryByText('Full requested uptime available')).not.toBeInTheDocument();
  });

  it('presents running identity, ownership, and remote access as separate primary facts', async () => {
    renderPage(deployment({
      installation_ownership: {
        kind: 'ambiguous',
        reason: 'supervisor status probe timed out',
      },
      local_access: false,
    }));

    const summary = await screen.findByRole('region', { name: 'Version 0.1.0' });
    expect(within(summary).getByLabelText('Running git commit abc123')).toHaveAttribute('title', 'abc123');
    expect(within(summary).getByText('Runtime manager is ambiguous')).toBeInTheDocument();
    expect(within(summary).getByText('supervisor status probe timed out')).toBeInTheDocument();
    expect(within(summary).getByText('Viewing remotely')).toBeInTheDocument();
    expect(within(summary).getByText(/host-local actions are unavailable/i)).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Build' })).not.toBeInTheDocument();
  });

  it('shows the bounded fatal diagnostic path as a log sink', async () => {
    renderPage(deployment({
      log: {
        stdout: false,
        file: '/srv/phoenix/prod.log',
        fatal_file: '/srv/phoenix/prod-fatal.log',
      },
    }));

    expect(await screen.findByText('/srv/phoenix/prod-fatal.log')).toBeInTheDocument();
    expect(screen.getByText('/srv/phoenix/prod.log')).toBeInTheDocument();
    expect(screen.queryByText('No log output configured.')).not.toBeInTheDocument();
  });

  it('refreshes the single deployment summary when updates report a different running identity', async () => {
    const initial = deployment();
    const restarted = deployment({
      build: { ...initial.build, version: '1.1.0', git_sha: 'def456' },
    });
    const update = deferredReleaseSnapshot();
    apiMock.deploymentInfo
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(restarted);
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk());
    apiMock.deploymentResources.mockResolvedValue(resourcesSnapshot());
    apiMock.releaseUpdateSnapshot.mockReturnValueOnce(update.promise);
    const updateSnapshot: ReleaseUpdateSnapshot = {
      installation_ownership: { kind: 'development' },
      current_version: '1.1.0',
      current_git_sha: 'def456',
      preview: { kind: 'unavailable', reason: 'not checked' },
      transaction: { kind: 'none' },
      authority: { kind: 'not_production' },
      sampled_at: '2026-06-01T00:00:04Z',
    };
    render(
      <MemoryRouter>
        <AboutDeploymentPage />
      </MemoryRouter>,
    );

    expect(await screen.findByText('Version 0.1.0')).toBeInTheDocument();
    await act(async () => { update.resolve(updateSnapshot); });
    expect(await screen.findByText('Version 1.1.0')).toBeInTheDocument();
    expect(screen.getByLabelText('Running git commit def456')).toBeInTheDocument();
    expect(apiMock.deploymentInfo).toHaveBeenCalledTimes(2);
  });

  it('refreshes the summary when ownership changes without a build change', async () => {
    const initial = deployment({
      installation_ownership: { kind: 'ambiguous', reason: 'supervisor busy' },
    });
    const managed = deployment({
      installation_ownership: { kind: 'systemd_managed' },
    });
    const update = deferredReleaseSnapshot();
    apiMock.deploymentInfo
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(managed);
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk());
    apiMock.deploymentResources.mockResolvedValue(resourcesSnapshot());
    apiMock.releaseUpdateSnapshot.mockReturnValueOnce(update.promise);
    const updateSnapshot: ReleaseUpdateSnapshot = {
      installation_ownership: { kind: 'systemd_managed' },
      current_version: initial.build.version,
      current_git_sha: initial.build.git_sha,
      preview: { kind: 'unavailable', reason: 'not checked' },
      transaction: { kind: 'none' },
      authority: { kind: 'allowed' },
      sampled_at: '2026-06-01T00:00:04Z',
    };
    render(
      <MemoryRouter>
        <AboutDeploymentPage />
      </MemoryRouter>,
    );

    expect(await screen.findByText('Runtime manager is ambiguous')).toBeInTheDocument();
    await act(async () => { update.resolve(updateSnapshot); });
    expect(await screen.findByText('Managed by systemd')).toBeInTheDocument();
    expect(apiMock.deploymentInfo).toHaveBeenCalledTimes(2);
  });

  it('uses non-alarming language for local development instances', async () => {
    renderPage(deployment({ local_access: true }));

    const summary = await screen.findByRole('region', { name: 'Version 0.1.0' });
    expect(within(summary).getByText('Development instance')).toBeInTheDocument();
    expect(within(summary).getByText(/production service management does not apply/i)).toBeInTheDocument();
    expect(within(summary).getByText('Viewing locally')).toBeInTheDocument();
  });

  it.each([
    [{ kind: 'launchd_managed' } as const, 'Managed by launchd'],
    [{ kind: 'systemd_managed' } as const, 'Managed by systemd'],
    [{ kind: 'bare_supervisor_managed', supervisor_pid: 42 } as const, 'Managed by Phoenix supervisor'],
    [{ kind: 'unmanaged', reason: 'started manually' } as const, 'Running without a proven manager'],
    [{ kind: 'unsupported', platform: 'windows' } as const, 'Runtime manager unsupported'],
  ])('renders truthful ownership copy for %s', async (installation_ownership, label) => {
    renderPage(deployment({ installation_ownership }));

    const summary = await screen.findByRole('region', { name: 'Version 0.1.0' });
    expect(within(summary).getByText(label)).toBeInTheDocument();
  });

  it('explains missing reveal actions at the disk table for remote viewers', async () => {
    renderPage(deployment({ local_access: false }));

    expect(await screen.findByText(/reveal actions require viewing this page on the Phoenix host/i)).toBeInTheDocument();
    expect(screen.getByText(/cleanup availability is shown separately per worktree/i)).toBeInTheDocument();
    expect(screen.queryByText(/remote browser remains read-only/i)).not.toBeInTheDocument();
  });

  it('summarizes measured, not-measured, and absent disk rows without summing overlaps', async () => {
    renderPage(deployment(), deploymentDisk({
      disk: [
        { category: 'database', label: 'Database', path: '/tmp/phoenix.db', size: { kind: 'measured', bytes: 1024 } },
        { category: 'managed_worktrees', label: 'Phoenix-managed worktrees', path: '/repo/.phoenix/worktrees/*', size: { kind: 'measured', bytes: 2048 } },
        { category: 'browser_profiles', label: 'Browser profiles', path: '/tmp/phoenix-browser-*', size: { kind: 'not_measured' } },
        { category: 'tls', label: 'TLS directory', path: '/tmp/tls', size: { kind: 'absent' } },
        { category: 'attachments', label: 'Attachments', path: '/tmp/phoenix.db', size: { kind: 'inline_db' } },
      ],
    }));

    const summary = await screen.findByLabelText('Disk usage health');
    expect(within(summary).getByText('Largest measured')).toBeInTheDocument();
    expect(within(summary).getByText('2.0 KiB')).toBeInTheDocument();
    expect(within(summary).queryByText('3.0 KiB')).not.toBeInTheDocument();
    expect(within(summary).getByText('Measured rows').nextElementSibling).toHaveTextContent('2');
    expect(within(summary).getByText('Not measured').nextElementSibling).toHaveTextContent('1');
    expect(within(summary).getByText('Absent').nextElementSibling).toHaveTextContent('1');
    expect(screen.getByText('1 disk row is path-only; measured rows may also overlap, so this section highlights categories rather than summing them.')).toBeInTheDocument();
  });

  it('renders the live resource monitor with unavailable category reasons and process rows', async () => {
    renderPage(deployment());

    expect(await screen.findByText('Host mostly idle')).toBeInTheDocument();
    expect(screen.getByText('Phoenix managed')).toBeInTheDocument();
    expect(screen.getByText('MCP disabled on this deployment')).toBeInTheDocument();
    expect(screen.getByText('phoenix-api')).toBeInTheDocument();
    expect(screen.getByText('chromium')).toBeInTheDocument();
    expect(screen.getByText('Managed CPU over time')).toBeInTheDocument();
    expect(screen.getByText('Managed memory over time')).toBeInTheDocument();
  });

  it('navigates to the deterministic conversations destination', async () => {
    apiMock.deploymentInfo.mockResolvedValue(deployment());
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk());
    apiMock.deploymentResources.mockResolvedValue(resourcesSnapshot());
    render(
      <MemoryRouter initialEntries={['/about']}>
        <AboutDeploymentPage />
        <LocationProbe />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'Conversations' }));
    expect(screen.getByLabelText('Current route')).toHaveTextContent('/');
  });

  it('renders independent diagnostics when deployment facts are unavailable', async () => {
    apiMock.deploymentInfo.mockRejectedValue(new Error('facts offline'));
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk({
      disk: [{ category: 'database', label: 'Database', path: '/tmp/phoenix.db', size: { kind: 'measured', bytes: 128 } }],
    }));
    apiMock.deploymentResources.mockResolvedValue(resourcesSnapshot());
    render(
      <MemoryRouter>
        <AboutDeploymentPage />
      </MemoryRouter>,
    );

    expect(await screen.findByText(/deployment facts unavailable — facts offline/i)).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Phoenix release updates' })).toBeInTheDocument();
    expect(await screen.findByText('Managed CPU over time')).toBeInTheDocument();
    expect(await screen.findByText('Database')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Network & TLS' })).not.toBeInTheDocument();
  });

  it('retains last-good deployment and disk snapshots when scoped refreshes fail', async () => {
    renderPage(deployment(), deploymentDisk({
      disk: [{
        category: 'database', label: 'Database', path: '/tmp/phoenix.db',
        size: { kind: 'measured', bytes: 128 },
      }],
    }));
    await screen.findByText('Version 0.1.0');
    apiMock.deploymentInfo.mockRejectedValueOnce(new Error('facts offline'));
    apiMock.deploymentDiskInfo.mockRejectedValueOnce(new Error('disk offline'));

    fireEvent.click(screen.getByRole('button', { name: 'Refresh deployment facts' }));
    fireEvent.click(screen.getByRole('button', { name: 'Refresh disk' }));

    expect(await screen.findByText(/deployment facts are stale — facts offline/i)).toBeInTheDocument();
    expect(await screen.findByText(/disk inventory is stale — disk offline/i)).toBeInTheDocument();
    expect(screen.getByText('Version 0.1.0')).toBeInTheDocument();
    expect(screen.getByText('Database')).toBeInTheDocument();
  });

  it('deployment refresh does not refresh disk or supersede resource sampling', async () => {
    apiMock.deploymentResources.mockImplementation(() => new Promise(() => {}));
    renderPage(deployment());
    await screen.findByText('Version 0.1.0');
    vi.clearAllMocks();
    apiMock.deploymentInfo.mockResolvedValue(deployment({ sampled_at: '2026-06-01T00:01:00Z' }));

    fireEvent.click(screen.getByRole('button', { name: 'Refresh deployment facts' }));
    await act(async () => {});

    expect(apiMock.deploymentInfo).toHaveBeenCalledTimes(1);
    expect(apiMock.deploymentDiskInfo).not.toHaveBeenCalled();
    expect(apiMock.deploymentResources).not.toHaveBeenCalled();
  });

  it('resource refresh supersedes an in-flight resource request while visible', async () => {
    const signals: AbortSignal[] = [];
    apiMock.deploymentInfo.mockResolvedValue(deployment());
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk());
    apiMock.deploymentResources
      .mockImplementationOnce(({ signal }: { signal: AbortSignal }) => {
        signals.push(signal);
        return new Promise<AboutResourcesSnapshot>(() => {});
      })
      .mockImplementationOnce(({ signal }: { signal: AbortSignal }) => {
        signals.push(signal);
        return Promise.resolve(resourcesSnapshot({ sampled_at: '2026-06-01T00:00:10Z' }));
      });

    render(
      <MemoryRouter>
        <AboutDeploymentPage />
      </MemoryRouter>,
    );

    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(1);
    await screen.findByRole('button', { name: 'Refresh resources now' });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Refresh resources now' }));
      await Promise.resolve();
    });

    expect(signals[0]?.aborted).toBe(true);
    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(2);
    expect(await screen.findByText('Host mostly idle')).toBeInTheDocument();
    expect(screen.getByText(/Resource sample captured/)).not.toHaveTextContent('stale');
  });

  it('deployment refresh respects hidden resource polling suspension', async () => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'hidden' });
    renderPage(deployment());

    await screen.findByRole('button', { name: 'Refresh deployment facts' });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Refresh deployment facts' }));
      await Promise.resolve();
    });

    expect(apiMock.deploymentInfo).toHaveBeenCalledTimes(2);
    expect(apiMock.deploymentDiskInfo).toHaveBeenCalledTimes(1);
    expect(apiMock.deploymentResources).not.toHaveBeenCalled();
  });

  it('polls only while visible, avoids overlap, and keeps the last good sample stale on error', async () => {
    vi.useFakeTimers();
    let resolveFirst: ((value: AboutResourcesSnapshot) => void) | undefined;
    apiMock.deploymentInfo.mockResolvedValue(deployment());
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk());
    apiMock.deploymentResources
      .mockImplementationOnce(() => new Promise<AboutResourcesSnapshot>((resolve) => { resolveFirst = resolve; }))
      .mockRejectedValueOnce(new Error('backend unavailable'));

    render(
      <MemoryRouter>
        <AboutDeploymentPage />
      </MemoryRouter>,
    );

    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(1);

    const firstResolve = resolveFirst;
    if (!firstResolve) throw new Error('expected first resource request to be pending');
    await act(async () => {
      firstResolve(resourcesSnapshot());
      await Promise.resolve();
    });
    expect(screen.getByText('Host mostly idle')).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
      await Promise.resolve();
    });
    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(2);
    expect(screen.getByText(/Live data stale — backend unavailable/)).toBeInTheDocument();
    expect(screen.getByText(/Resource sample captured/)).toHaveTextContent('stale');

    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'hidden' });
    document.dispatchEvent(new Event('visibilitychange'));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3_000);
    });
    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(2);

    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'visible' });
    apiMock.deploymentResources.mockResolvedValueOnce(resourcesSnapshot({ sampled_at: '2026-06-01T00:00:10Z' }));
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
      await Promise.resolve();
    });
    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(3);
  });

  it('skips the initial resource fetch while hidden and fetches immediately when visible', async () => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'hidden' });
    apiMock.deploymentInfo.mockResolvedValue(deployment());
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk());
    apiMock.deploymentResources.mockResolvedValue(resourcesSnapshot());

    render(
      <MemoryRouter>
        <AboutDeploymentPage />
      </MemoryRouter>,
    );

    expect(apiMock.deploymentResources).not.toHaveBeenCalled();
    await screen.findByText('No resource sample available yet.');

    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'visible' });
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
      await Promise.resolve();
    });

    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(1);
    expect(await screen.findByText('Host mostly idle')).toBeInTheDocument();
  });

  it('ignores an in-flight resource completion after unmount', async () => {
    let resolveFirst: ((value: AboutResourcesSnapshot) => void) | undefined;
    apiMock.deploymentInfo.mockResolvedValue(deployment());
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk());
    apiMock.deploymentResources.mockImplementationOnce(() => new Promise<AboutResourcesSnapshot>((resolve) => { resolveFirst = resolve; }));

    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    const view = render(
      <MemoryRouter>
        <AboutDeploymentPage />
      </MemoryRouter>,
    );

    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(1);
    view.unmount();

    const firstResolve = resolveFirst;
    if (!firstResolve) throw new Error('expected first resource request to be pending');
    await act(async () => {
      firstResolve(resourcesSnapshot());
      await Promise.resolve();
    });

    expect(consoleError).not.toHaveBeenCalled();
    consoleError.mockRestore();
  });

  it.each(['resolve', 'reject'] as const)('ignores deferred SQLite %s after unmount', async (outcome) => {
    let resolveSqlite!: (value: SqliteWorkloadReportResponse) => void;
    let rejectSqlite!: (reason: Error) => void;
    apiMock.deploymentSqliteWorkload.mockImplementationOnce(() => new Promise((resolve, reject) => {
      resolveSqlite = resolve;
      rejectSqlite = reject;
    }));
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    const view = renderPage(deployment());
    expect(apiMock.deploymentSqliteWorkload).toHaveBeenCalledTimes(1);
    view.unmount();
    await act(async () => {
      if (outcome === 'resolve') resolveSqlite(sqliteReport());
      else rejectSqlite(new Error('late sqlite failure'));
      await Promise.resolve();
    });
    expect(consoleError).not.toHaveBeenCalled();
    consoleError.mockRestore();
  });

  it('keeps the first pending request across a skipped poll, then aborts it on hidden and unmount without state updates', async () => {
    vi.useFakeTimers();
    const abortedSignals: AbortSignal[] = [];
    let resolveFirst: ((value: AboutResourcesSnapshot) => void) | undefined;
    apiMock.deploymentInfo.mockResolvedValue(deployment());
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk());
    apiMock.deploymentResources.mockImplementationOnce(({ signal }: { signal: AbortSignal }) => {
      abortedSignals.push(signal);
      return new Promise<AboutResourcesSnapshot>((resolve, reject) => {
        resolveFirst = resolve;
        signal.addEventListener('abort', () => {
          reject(new DOMException('Aborted', 'AbortError'));
        }, { once: true });
      });
    });

    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    const view = render(
      <MemoryRouter>
        <AboutDeploymentPage />
      </MemoryRouter>,
    );

    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(1);
    expect(abortedSignals).toHaveLength(1);
    expect(abortedSignals[0]?.aborted).toBe(false);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(1);
    expect(abortedSignals[0]?.aborted).toBe(false);

    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'hidden' });
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
      await Promise.resolve();
    });
    expect(abortedSignals[0]?.aborted).toBe(true);

    const firstResolve = resolveFirst;
    if (!firstResolve) throw new Error('expected first resource request to be pending');
    await act(async () => {
      firstResolve(resourcesSnapshot());
      await Promise.resolve();
    });
    expect(consoleError).not.toHaveBeenCalled();
    expect(screen.queryByText('Host mostly idle')).not.toBeInTheDocument();

    view.unmount();
    expect(abortedSignals[0]?.aborted).toBe(true);
    consoleError.mockRestore();
  });
  it('starts a new request on visible immediately after hide even when the old promise ignores abort', async () => {
    let resolveFirst: ((value: AboutResourcesSnapshot) => void) | undefined;
    let resolveSecond: ((value: AboutResourcesSnapshot) => void) | undefined;
    const abortedSignals: AbortSignal[] = [];
    apiMock.deploymentInfo.mockResolvedValue(deployment());
    apiMock.deploymentDiskInfo.mockResolvedValue(deploymentDisk());
    apiMock.deploymentResources
      .mockImplementationOnce(({ signal }: { signal: AbortSignal }) => {
        abortedSignals.push(signal);
        return new Promise<AboutResourcesSnapshot>((resolve) => {
          resolveFirst = resolve;
        });
      })
      .mockImplementationOnce(({ signal }: { signal: AbortSignal }) => {
        abortedSignals.push(signal);
        return new Promise<AboutResourcesSnapshot>((resolve) => {
          resolveSecond = resolve;
        });
      });

    render(
      <MemoryRouter>
        <AboutDeploymentPage />
      </MemoryRouter>,
    );

    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(1);
    expect(abortedSignals[0]?.aborted).toBe(false);

    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'hidden' });
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
      await Promise.resolve();
    });
    expect(abortedSignals[0]?.aborted).toBe(true);

    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'visible' });
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
      await Promise.resolve();
    });
    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(2);
    expect(abortedSignals[1]?.aborted).toBe(false);

    const second = resolveSecond;
    if (!second) throw new Error('expected second resource request to be pending');
    await act(async () => {
      second(resourcesSnapshot({ sampled_at: '2026-06-01T00:00:10Z', managed_total: { cpu_percent: 40, memory_bytes: 4 * 1024, process_count: 3, deduplicated_pid_count: 2 } }));
      await Promise.resolve();
    });
    expect(screen.getByText(/Resource sample captured/)).not.toHaveTextContent('stale');
    expect(screen.getAllByText('40.0%').length).toBeGreaterThan(0);

    const first = resolveFirst;
    if (!first) throw new Error('expected first resource request to be pending');
    await act(async () => {
      first(resourcesSnapshot({ sampled_at: '2026-06-01T00:00:20Z', managed_total: { cpu_percent: 5, memory_bytes: 512, process_count: 1, deduplicated_pid_count: 1 } }));
      await Promise.resolve();
    });

    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(2);
    expect(screen.getAllByText('40.0%').length).toBeGreaterThan(0);
    expect(screen.queryByText('5.0%')).not.toBeInTheDocument();
    expect(screen.getByText(/Resource sample captured/)).not.toHaveTextContent('stale');
  });

  it('highlights managed worktrees when they are the largest measured category', async () => {
    renderPage(deployment(), deploymentDisk({
      disk: [
        { category: 'database', label: 'Database', path: '/tmp/phoenix.db', size: { kind: 'measured', bytes: 1024 } },
        { category: 'managed_worktrees', label: 'Phoenix-managed worktrees', path: '/repo/.phoenix/worktrees/*', size: { kind: 'measured', bytes: 4096 } },
      ],
    }));

    expect(await screen.findByText('Phoenix-managed worktrees are the largest measured disk category.')).toBeInTheDocument();
    expect(screen.getByText('Phoenix-managed worktrees').closest('tr')).toHaveClass('deploy-table__row--largest');
  });

  it('renders managed worktree drilldown actions from typed disposition', async () => {
    renderPage(deployment(), deploymentDisk({
      disk: [
        { category: 'managed_worktrees', label: 'Phoenix-managed worktrees', path: '/repo/.phoenix/worktrees/*', size: { kind: 'measured', bytes: 3000 } },
      ],
      managed_worktrees: [
        {
          path: '/repo/.phoenix/worktrees/live',
          size: { kind: 'measured', bytes: 2000 },
          repository: '/repo',
          branch_name: 'task-live',
          disposition: { kind: 'live', conversation_id: 'live-conv', slug: 'live-task', title: 'Live task', state: 'Idle', archived: false },
        },
        {
          path: '/repo/.phoenix/worktrees/leftover',
          size: { kind: 'measured', bytes: 1000 },
          repository: '/repo',
          branch_name: 'task-leftover',
          disposition: { kind: 'leftover', source_conversation_id: 'old-conv', source_state: 'Terminal', archived: true, cleanup_allowed: true },
        },
      ],
    }));

    fireEvent.click(await screen.findByText('Show worktrees'));

    expect(screen.getByText(/Live: Live task/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Open conversation' })).toBeInTheDocument();
    expect(screen.getByText(/Leftover: old-conv/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Clean up leftover' })).toBeInTheDocument();
  });
});
