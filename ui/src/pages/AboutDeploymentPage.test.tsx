import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import type { AboutResourcesSnapshot } from '../generated/AboutResourcesSnapshot';
import type { DeploymentInfo } from '../generated/DeploymentInfo';
import type { DeploymentDiskInfo } from '../generated/DeploymentDiskInfo';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    deploymentInfo: vi.fn(),
    deploymentDiskInfo: vi.fn(),
    deploymentResources: vi.fn(),
    cleanupManagedWorktree: vi.fn(),
    revealPath: vi.fn(),
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
    log: { stdout: true, file: null },
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

function renderPage(info: DeploymentInfo, disk: DeploymentDiskInfo = deploymentDisk()) {
  apiMock.deploymentInfo.mockResolvedValue(info);
  apiMock.deploymentDiskInfo.mockResolvedValue(disk);
  apiMock.deploymentResources.mockResolvedValue(resourcesSnapshot());
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
    apiMock.cleanupManagedWorktree.mockReset();
    apiMock.revealPath.mockReset();
    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'visible' });
  });

  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    vi.clearAllMocks();
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

  it('page refresh supersedes an in-flight resource request while visible', async () => {
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
    await screen.findByRole('button', { name: 'Refresh' });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
      await Promise.resolve();
    });

    expect(signals[0]?.aborted).toBe(true);
    expect(apiMock.deploymentResources).toHaveBeenCalledTimes(2);
    expect(await screen.findByText('Host mostly idle')).toBeInTheDocument();
    expect(screen.getByText(/Resource sample captured/)).not.toHaveTextContent('stale');
  });

  it('page refresh respects hidden resource polling suspension', async () => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'hidden' });
    renderPage(deployment());

    await screen.findByRole('button', { name: 'Refresh' });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
      await Promise.resolve();
    });

    expect(apiMock.deploymentInfo).toHaveBeenCalledTimes(2);
    expect(apiMock.deploymentDiskInfo).toHaveBeenCalledTimes(2);
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
