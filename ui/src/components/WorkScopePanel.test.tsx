// The `work_scope_update` SSE push is edge-triggered on bash state
// transitions, so `output_bytes` (which grows continuously as a process
// emits output) stays frozen between transitions. `WorkScopeSection` closes
// that gap by polling the inventory endpoint while any bash handle is running,
// merged into a single last-arrival-wins displayed snapshot alongside the
// initial fetch and the SSE-fed `liveInventory` prop.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import type { WorkScopeInventory, BashHandleInventory } from '../api';
import { api } from '../api';
import { hasRunningBash, hasLiveResource } from './workScopeHelpers';

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
    output_bytes: 0,
    ...over,
  };
}

/** A clean-exit tombstone: exit 0, no signal. */
function tombSuccess(over: Partial<BashHandleInventory> = {}): BashHandleInventory {
  return bash({ state: 'tombstoned', duration_ms: 10, exit_code: 0, ...over });
}

function inv(
  bashes: BashHandleInventory[],
  over: Partial<Pick<WorkScopeInventory, 'tmux' | 'browser'>> = {},
): WorkScopeInventory {
  return { scope_key: 'ws-1', bash: bashes, tmux: null, browser: null, ...over };
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

describe('hasLiveResource', () => {
  it('is false for null / empty / all-terminal resources', () => {
    expect(hasLiveResource(null)).toBe(false);
    expect(hasLiveResource(inv([]))).toBe(false);
    expect(hasLiveResource(inv([bash({ state: 'tombstoned' })]))).toBe(false);
    expect(hasLiveResource(inv([], { tmux: { status: 'gone' } }))).toBe(false);
    expect(hasLiveResource(inv([], { browser: { state: 'torn_down', idle_ms: 0 } }))).toBe(false);
  });

  it('is true for a running bash handle (bash-only)', () => {
    expect(hasLiveResource(inv([bash({ state: 'running' })]))).toBe(true);
    expect(hasLiveResource(inv([bash({ state: 'kill_pending_kernel' })]))).toBe(true);
  });

  it('is true for a tmux entry that exists — live or not_probed (tmux-only)', () => {
    expect(hasLiveResource(inv([], { tmux: { status: 'live' } }))).toBe(true);
    expect(hasLiveResource(inv([], { tmux: { status: 'not_probed' } }))).toBe(true);
  });

  it('is true for a live browser session (browser-only)', () => {
    expect(hasLiveResource(inv([], { browser: { state: 'live', idle_ms: 120_000 } }))).toBe(true);
  });
});

describe('bash glyph: liveness vs outcome', () => {
  // The glyph separates LIVENESS (a running handle is "alive", a live dot)
  // from OUTCOME (a terminal handle is ✓/✗ by exit status). A running handle
  // must NOT read as a success check.
  function glyphEl() {
    const label = screen.getByText('sleep 100');
    const row = label.closest('.ws-row');
    if (!row) throw new Error('bash row not found');
    const g = row.querySelector('.ws-glyph');
    if (!g) throw new Error('glyph not found');
    return g;
  }

  it('running → green live dot (●, ws-glyph--live), not a check', async () => {
    getInv.mockResolvedValue(inv([bash({ state: 'running' })]));
    await renderExpanded();
    const g = glyphEl();
    expect(g.textContent).toBe('●');
    expect(g.classList.contains('ws-glyph--live')).toBe(true);
    expect(g.classList.contains('ws-glyph--ok')).toBe(false);
  });

  it('tombstoned exit 0 → green check (✓, ws-glyph--ok)', async () => {
    getInv.mockResolvedValue(inv([tombSuccess()]));
    await renderExpanded();
    const g = glyphEl();
    expect(g.textContent).toBe('✓');
    expect(g.classList.contains('ws-glyph--ok')).toBe(true);
  });

  it('tombstoned non-zero exit → red ✗ (ws-glyph--err) with the code in the title', async () => {
    getInv.mockResolvedValue(inv([tombSuccess({ exit_code: 3 })]));
    await renderExpanded();
    const g = glyphEl();
    expect(g.textContent).toBe('✗');
    expect(g.classList.contains('ws-glyph--err')).toBe(true);
    expect(g.getAttribute('title')).toBe('exited 3');
  });

  it('tombstoned killed by signal → red ✗ (ws-glyph--err) with the signal in the title', async () => {
    // Killed-by-signal tombstone: a signal, no exit code.
    getInv.mockResolvedValue(
      inv([bash({ state: 'tombstoned', duration_ms: 10, signal_number: 9 })]),
    );
    await renderExpanded();
    const g = glyphEl();
    expect(g.textContent).toBe('✗');
    expect(g.classList.contains('ws-glyph--err')).toBe(true);
    expect(g.getAttribute('title')).toBe('killed (signal 9)');
  });
});

describe('WorkScopeSection running-handle poll', () => {
  it('polls every 2s while a handle runs, advancing the displayed byte count', async () => {
    getInv
      .mockResolvedValueOnce(inv([bash({ output_bytes: 0 })]))
      .mockResolvedValueOnce(inv([bash({ output_bytes: 4096 })]));

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

  it('polls with a tmux-only inventory (no running bash), so a terminal-only scope refreshes', async () => {
    // The terminal panel opens a tmux server but spawns no bash handle. The
    // old `hasRunningBash` gate left this scope un-polled; `hasLiveResource`
    // keeps it polling so a status change (not_probed → live) is picked up.
    getInv.mockResolvedValue(inv([], { tmux: { status: 'not_probed' } }));

    await renderExpanded();

    await act(async () => {
      vi.advanceTimersByTime(2000);
      await Promise.resolve();
    });
    // Initial fetch + one poll, despite there being no bash handle at all.
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
    const sse = inv([bash({ output_bytes: 0 })]);
    getInv.mockResolvedValue(inv([bash({ output_bytes: 8192 })]));

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
