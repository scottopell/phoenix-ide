import { render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import type { DeploymentInfo } from '../generated/DeploymentInfo';

const { apiMock } = vi.hoisted(() => ({
  apiMock: {
    deploymentInfo: vi.fn(),
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
    disk: [],
    log: { stdout: true, file: null },
    local_access: false,
    sampled_at: '2026-06-01T00:00:01Z',
    ...overrides,
  };
}

function renderPage(info: DeploymentInfo) {
  apiMock.deploymentInfo.mockResolvedValue(info);
  render(
    <MemoryRouter>
      <AboutDeploymentPage />
    </MemoryRouter>,
  );
}

describe('AboutDeploymentPage disk usage health', () => {
  beforeEach(() => {
    apiMock.deploymentInfo.mockReset();
    apiMock.revealPath.mockReset();
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it('summarizes measured, not-measured, and absent disk rows without summing overlaps', async () => {
    renderPage(deployment({
      disk: [
        { label: 'Database', path: '/tmp/phoenix.db', size: { kind: 'measured', bytes: 1024 } },
        { label: 'Phoenix-managed worktrees', path: '/repo/.phoenix/worktrees/*', size: { kind: 'measured', bytes: 2048 } },
        { label: 'Browser profiles', path: '/tmp/phoenix-browser-*', size: { kind: 'not_measured' } },
        { label: 'TLS directory', path: '/tmp/tls', size: { kind: 'absent' } },
        { label: 'Attachments', path: '/tmp/phoenix.db', size: { kind: 'inline_db' } },
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
    renderPage(deployment({
      disk: [
        { label: 'Database', path: '/tmp/phoenix.db', size: { kind: 'measured', bytes: 1024 } },
        { label: 'Phoenix-managed worktrees', path: '/repo/.phoenix/worktrees/*', size: { kind: 'measured', bytes: 4096 } },
      ],
    }));

    expect(await screen.findByText('Phoenix-managed worktrees are the largest measured disk category.')).toBeInTheDocument();
    expect(screen.getByText('Phoenix-managed worktrees').closest('tr')).toHaveClass('deploy-table__row--largest');
  });
});
