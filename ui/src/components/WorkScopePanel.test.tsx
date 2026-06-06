// The `work_scope_update` SSE push is edge-triggered on bash state
// transitions, so `ring_bytes_used` (which grows continuously as a process
// emits output) stays frozen between transitions. `WorkScopeSection` closes
// that gap by polling the inventory endpoint while any bash handle is running,
// merged into a single last-arrival-wins displayed snapshot alongside the
// initial fetch and the SSE-fed `liveInventory` prop.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import type { WorkScopeInventory, BashHandleInventory } from '../api';
import { api } from '../api';
import { hasRunningBash } from './workScopeHelpers';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: { ...actual.api, getWorkScopeInventory: vi.fn() },
  };
});

import { WorkScopeSection } from './WorkScopePanel';

const getInv = vi.mocked(api.getWorkScopeInventory);

function bash(over: Partial<BashHandleInventory> = {}): BashHandleInventory {
  return {
    handle_id: 'b-1',
    cmd: 'sleep 100',
    state: 'running',
    started_at: new Date().toISOString(),
    ring_bytes_used: 0,
    ...over,
  };
}

function inv(bashes: BashHandleInventory[]): WorkScopeInventory {
  return { scope_key: 'ws-1', bash: bashes, tmux: null, browser: null };
}

/** Render the expanded section, flushing the initial fetch microtask. */
async function renderExpanded(liveInventory?: WorkScopeInventory | null) {
  let utils!: ReturnType<typeof render>;
  await act(async () => {
    utils = render(
      <WorkScopeSection
        scopeKey="ws-1"
        liveInventory={liveInventory}
        expanded={true}
        onToggleExpanded={() => {}}
      />,
    );
  });
  return utils;
}

/** Open the bash row's detail disclosure so `output` becomes visible. */
function openBashDetail() {
  fireEvent.click(screen.getByTitle('Toggle details'));
}

beforeEach(() => {
  vi.useFakeTimers();
  getInv.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('hasRunningBash', () => {
  it('is false for null / empty / all-tombstoned, true once a handle is live', () => {
    expect(hasRunningBash(null)).toBe(false);
    expect(hasRunningBash(inv([]))).toBe(false);
    expect(hasRunningBash(inv([bash({ state: 'tombstoned' })]))).toBe(false);
    expect(hasRunningBash(inv([bash({ state: 'running' })]))).toBe(true);
    expect(hasRunningBash(inv([bash({ state: 'kill_pending_kernel' })]))).toBe(true);
  });
});

describe('WorkScopeSection running-handle poll', () => {
  it('polls every 2s while a handle runs, advancing the displayed byte count', async () => {
    getInv
      .mockResolvedValueOnce(inv([bash({ ring_bytes_used: 0 })]))
      .mockResolvedValueOnce(inv([bash({ ring_bytes_used: 4096 })]));

    await renderExpanded();
    openBashDetail();
    expect(screen.getByText('0 B')).toBeTruthy();

    // Advance one poll interval and flush the fetch.
    await act(async () => {
      vi.advanceTimersByTime(2000);
      await Promise.resolve();
    });

    expect(getInv).toHaveBeenCalledTimes(2);
    expect(screen.getByText('4.0 KB')).toBeTruthy();
  });

  it('stops polling once no handle is running', async () => {
    getInv
      .mockResolvedValueOnce(inv([bash({ state: 'running' })]))
      // First poll returns a tombstoned handle — nothing running anymore.
      .mockResolvedValueOnce(inv([bash({ state: 'tombstoned' })]));

    await renderExpanded();

    await act(async () => {
      vi.advanceTimersByTime(2000);
      await Promise.resolve();
    });
    expect(getInv).toHaveBeenCalledTimes(2);

    // No further polls: the handle is tombstoned, so the interval cleared.
    await act(async () => {
      vi.advanceTimersByTime(10_000);
      await Promise.resolve();
    });
    expect(getInv).toHaveBeenCalledTimes(2);
  });

  it('does not poll when the only handle is already tombstoned', async () => {
    getInv.mockResolvedValue(inv([bash({ state: 'tombstoned' })]));

    await renderExpanded();

    await act(async () => {
      vi.advanceTimersByTime(10_000);
      await Promise.resolve();
    });
    // Initial fetch only — no poll.
    expect(getInv).toHaveBeenCalledTimes(1);
  });

  it('last-arrival-wins: a poll refreshes bytes even when liveInventory (SSE) is present', async () => {
    // SSE snapshot is fresh on transitions but carries stale bytes between them;
    // the running-handle poll must still advance the displayed byte count.
    const sse = inv([bash({ ring_bytes_used: 0 })]);
    getInv.mockResolvedValue(inv([bash({ ring_bytes_used: 8192 })]));

    await renderExpanded(sse);
    openBashDetail();

    await act(async () => {
      vi.advanceTimersByTime(2000);
      await Promise.resolve();
    });

    // Poll arrived after SSE → its byte count wins, despite liveInventory being set.
    expect(getInv).toHaveBeenCalled();
    expect(screen.getByText('8.0 KB')).toBeTruthy();
  });
});
