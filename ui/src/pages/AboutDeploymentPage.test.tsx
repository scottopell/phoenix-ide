import { fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import type { DeploymentInfo } from '../generated/DeploymentInfo';
import type { DeploymentDiskInfo } from '../generated/DeploymentDiskInfo';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    deploymentInfo: vi.fn(),
    deploymentDiskInfo: vi.fn(),
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

import { AboutDeploymentPage } from './AboutDeploymentPage';

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
    resources: {
      process_memory_bytes: 1024,
      process_cpu_percent: 1.5,
      system_total_memory_bytes: 4096,
      system_available_memory_bytes: 2048,
      logical_cpu_count: 4,
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

function renderPage(info: DeploymentInfo, disk: DeploymentDiskInfo = deploymentDisk()) {
  apiMock.deploymentInfo.mockResolvedValue(info);
  apiMock.deploymentDiskInfo.mockResolvedValue(disk);
  render(
    <MemoryRouter>
      <AboutDeploymentPage />
    </MemoryRouter>,
  );
}

describe('AboutDeploymentPage disk usage health', () => {
  beforeEach(() => {
    apiMock.deploymentInfo.mockReset();
    apiMock.deploymentDiskInfo.mockReset();
    apiMock.cleanupManagedWorktree.mockReset();
    apiMock.revealPath.mockReset();
  });

  afterEach(() => {
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
