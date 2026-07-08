// The `work_scope_update` SSE push is edge-triggered on bash state
// transitions, so `output_bytes` (which grows continuously as a process
// emits output) stays frozen between transitions. `WorkScopeSection` closes
// that gap by polling the inventory endpoint while any bash handle is running,
// merged into a single last-arrival-wins displayed snapshot alongside the
// initial fetch and the SSE-fed `liveInventory` prop.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { useEffect } from 'react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import type { WorkScopeInventory, BashHandleInventory } from '../api';
import { api } from '../api';
import { ViewerSlotProvider, useViewerSlot } from '../contexts/ViewerSlotContext';
import { hasRunningBash, hasLiveResource } from './workScopeHelpers';

/** Observe the viewer slot so a test can assert that an affordance (e.g. the
 *  browser "open →" button) drove the slot transition. */
function CaptureSlot({ onSlot }: { onSlot: (slot: ReturnType<typeof useViewerSlot>['slot']) => void }) {
  const { slot } = useViewerSlot();
  useEffect(() => { onSlot(slot); }, [slot, onSlot]);
  return null;
}

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: { ...actual.api, getWorkScopeInventory: vi.fn(), stopWorkScopeBrowserSession: vi.fn() },
  };
});

import { WorkScopeSection, WorkScopePanel } from './WorkScopePanel';
import { useSeededLiveCount } from './useWorkScopeSeed';

const getInv = vi.mocked(api.getWorkScopeInventory);
const stopBrowser = vi.mocked(api.stopWorkScopeBrowserSession);

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
  over: Partial<Pick<WorkScopeInventory, 'scope_key' | 'tmux' | 'browser'>> = {},
): WorkScopeInventory {
  return { scope_key: 'ws-1', bash: bashes, tmux: null, browser: null, ...over };
}

/** Render the expanded section, flushing the initial fetch microtask. */
async function renderExpanded(liveInventory?: WorkScopeInventory | null) {
  let utils!: ReturnType<typeof render>;
  await act(async () => {
    utils = render(sectionTree({ scopeKey: 'ws-1', liveInventory: liveInventory ?? null }));
  });
  return utils;
}

/** Open the bash row's detail disclosure so `output` becomes visible. */
function openBashDetail() {
  fireEvent.click(screen.getByTitle('Toggle details'));
}

/** The provider tree the section needs at runtime: a Router (ViewerSlot uses
 *  useLocation) wrapping the viewer-slot context (BashRow's inspect affordance
 *  calls useViewerSlot). Tests render/rerender through this so the section
 *  mounts in the same context the app gives it. */
function sectionTree(props: { scopeKey: string; liveInventory?: WorkScopeInventory | null }) {
  return (
    <MemoryRouter initialEntries={['/c/conv-A']}>
      <Routes>
        <Route
          path="/c/:slug"
          element={
            <ViewerSlotProvider scopeKey="conv-A" browserSessionActive={false}>
              <WorkScopeSection
                scopeKey={props.scopeKey}
                liveInventory={props.liveInventory ?? null}
                expanded={true}
                onToggleExpanded={() => {}}
              />
            </ViewerSlotProvider>
          }
        />
      </Routes>
    </MemoryRouter>
  );
}

beforeEach(() => {
  vi.useFakeTimers();
  getInv.mockReset();
  stopBrowser.mockReset();
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
      await vi.advanceTimersByTimeAsync(2000);
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
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(getInv).toHaveBeenCalledTimes(2);

    // No further polls: the handle is tombstoned, so the interval cleared.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
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
      await vi.advanceTimersByTimeAsync(2000);
    });
    // Initial fetch + one poll, despite there being no bash handle at all.
    expect(getInv).toHaveBeenCalledTimes(2);
  });

  it('does not poll when the only handle is already tombstoned', async () => {
    getInv.mockResolvedValue(inv([bash({ state: 'tombstoned' })]));

    await renderExpanded();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
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
      await vi.advanceTimersByTimeAsync(2000);
    });

    // Poll arrived after SSE → its byte count wins, despite liveInventory being set.
    expect(getInv).toHaveBeenCalled();
    expect(screen.getByText('8.0 KB')).toBeTruthy();
  });
});

describe('WorkScopeSection stale-scope guard', () => {
  it('a fetch for an old scopeKey resolving after the key changed does NOT overwrite the new scope', async () => {
    // Defer the OLD scope's fetch so it resolves last, after scopeKey flips.
    let resolveOld!: (v: WorkScopeInventory) => void;
    const oldPending = new Promise<WorkScopeInventory>((r) => {
      resolveOld = r;
    });
    const oldInv = inv([bash({ cmd: 'OLD-SCOPE-CMD', state: 'tombstoned', duration_ms: 1 })]);
    const newInv = inv([bash({ cmd: 'NEW-SCOPE-CMD', state: 'tombstoned', duration_ms: 1 })]);

    getInv.mockImplementation((key: string) =>
      key === 'ws-old' ? oldPending : Promise.resolve(newInv),
    );

    let utils!: ReturnType<typeof render>;
    await act(async () => {
      utils = render(sectionTree({ scopeKey: 'ws-old', liveInventory: null }));
    });

    // Switch to the new scope; its fetch resolves immediately.
    await act(async () => {
      utils.rerender(sectionTree({ scopeKey: 'ws-new', liveInventory: null }));
      await Promise.resolve();
    });
    expect(screen.getByText('NEW-SCOPE-CMD')).toBeTruthy();

    // The stale OLD fetch resolves LAST — it must be rejected by the guard.
    await act(async () => {
      resolveOld(oldInv);
      await Promise.resolve();
    });

    expect(screen.getByText('NEW-SCOPE-CMD')).toBeTruthy();
    expect(screen.queryByText('OLD-SCOPE-CMD')).toBeNull();
  });
});

describe('WorkScopeSection SSE-generation guard (same-scope time ordering)', () => {
  it('a pull that resolves AFTER an SSE update with a live handle does NOT overwrite the pushed inventory', async () => {
    // Repro: the section opens just before a bash handle spawns. The initial
    // GET sees an empty scope and is deferred; the spawn `work_scope_update`
    // SSE lands first with a live handle. The older-but-later-resolving empty
    // pull must NOT wipe the SSE row, and (because that would also stop the
    // poll) must not strand the inventory stale.
    let resolveInitial!: (v: WorkScopeInventory) => void;
    const initialPending = new Promise<WorkScopeInventory>((r) => {
      resolveInitial = r;
    });
    getInv.mockReturnValue(initialPending);

    // Render with no SSE yet; the initial fetch is in flight (unresolved).
    let utils!: ReturnType<typeof render>;
    await act(async () => {
      utils = render(sectionTree({ scopeKey: 'ws-1', liveInventory: null }));
    });

    // SSE push lands first: a live handle. Bumps the generation.
    const pushed = inv([bash({ cmd: 'SSE-LIVE-CMD', state: 'running' })]);
    await act(async () => {
      utils.rerender(sectionTree({ scopeKey: 'ws-1', liveInventory: pushed }));
      await Promise.resolve();
    });
    expect(screen.getByText('SSE-LIVE-CMD')).toBeTruthy();

    // Now the stale empty initial pull resolves LAST. The generation advanced
    // since it started → its result is dropped, the SSE row survives.
    await act(async () => {
      resolveInitial(inv([]));
      await Promise.resolve();
    });

    expect(screen.getByText('SSE-LIVE-CMD')).toBeTruthy();
    // The badge still reflects the live handle (not wiped to 0).
    expect(screen.getByText('1')).toBeTruthy();

    // And because the live handle survived, the poll keeps running — a stale
    // empty pull did not strand the inventory by stopping the poll.
    getInv.mockResolvedValue(inv([bash({ cmd: 'SSE-LIVE-CMD', state: 'running' })]));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(getInv.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it('normal case: a pull with no intervening SSE still advances the byte count', async () => {
    // No SSE update lands between the pull starting and resolving → the
    // generation is unchanged, so the pull result applies as before.
    getInv
      .mockResolvedValueOnce(inv([bash({ output_bytes: 0 })]))
      .mockResolvedValueOnce(inv([bash({ output_bytes: 4096 })]));

    await renderExpanded();
    openBashDetail();
    expect(screen.getByText('0 B')).toBeTruthy();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });

    expect(getInv).toHaveBeenCalledTimes(2);
    expect(screen.getByText('4.0 KB')).toBeTruthy();
  });
});

describe('WorkScopeSection in-flight gate (slow poll, no overlap or starvation)', () => {
  it('a poll slower than the interval does not stack overlapping fetches, and its result still applies', async () => {
    // Repro: the inventory endpoint is slower than the 2s poll interval. With
    // no in-flight gate, every interval issued a fresh pull that superseded the
    // prior in-flight one, so the slow pull never got to apply — `output_bytes`
    // stopped advancing (and errors were suppressed) until a fetch happened to
    // finish within an interval. The gate ensures at most one poll is
    // outstanding: the slow pull completes and applies.
    let resolveSlow!: (v: WorkScopeInventory) => void;
    const slowPending = new Promise<WorkScopeInventory>((r) => {
      resolveSlow = r;
    });
    const slowResult = inv([bash({ cmd: 'live-cmd', state: 'running', output_bytes: 8192 })]);

    // SSE-seeded live handle so the running poll starts; initial fetch shares it.
    const seed = inv([bash({ cmd: 'live-cmd', state: 'running', output_bytes: 0 })]);
    getInv.mockResolvedValue(seed);

    await renderExpanded(seed);
    openBashDetail();
    expect(screen.getByText('0 B')).toBeTruthy();
    const callsAfterSeed = getInv.mock.calls.length;

    // The next poll is slow — held open across several intervals.
    getInv.mockReturnValueOnce(slowPending);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    // One poll issued; it is now in flight.
    expect(getInv).toHaveBeenCalledTimes(callsAfterSeed + 1);

    // Advance two more intervals while the slow poll is still in flight. The
    // gate must suppress these — no overlapping fetch is issued.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(getInv).toHaveBeenCalledTimes(callsAfterSeed + 1);

    // The slow poll finally resolves — its result is applied (no starvation).
    await act(async () => {
      resolveSlow(slowResult);
      await Promise.resolve();
    });
    expect(screen.getByText('8.0 KB')).toBeTruthy();

    // The gate has cleared, so a subsequent interval can issue the next poll.
    getInv.mockResolvedValueOnce(
      inv([bash({ cmd: 'live-cmd', state: 'running', output_bytes: 16384 })]),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(getInv).toHaveBeenCalledTimes(callsAfterSeed + 2);
    expect(screen.getByText('16.0 KB')).toBeTruthy();
  });
});

describe('WorkScopePanel collapsed standalone dock keeps polling without SSE (REQ-WSUI-009)', () => {
  // The chain dock omits `liveInventory` (no per-conversation SSE channel). If
  // its poll paused while collapsed, a resource live at collapse and then
  // reaped would leave the count badge reading "running" forever until expand.
  // An SSE-less surface therefore keeps polling while collapsed so the badge
  // settles to 0.
  it('polls while collapsed and the badge settles to 0 once nothing is live', async () => {
    getInv
      // Initial fetch: one live handle → badge shows 1.
      .mockResolvedValueOnce(inv([bash({ state: 'running' })]))
      // First poll: the handle has exited → nothing live, badge settles to 0.
      .mockResolvedValueOnce(inv([bash({ state: 'tombstoned' })]));

    await act(async () => {
      render(
        <WorkScopePanel scopeKey="ws-1" liveInventory={null} collapsed={true} onToggle={() => {}} />,
      );
    });
    await act(async () => {
      await Promise.resolve();
    });

    const badge = () => document.querySelector('.ws-count-badge');
    // Initial fetch landed with a live handle.
    expect(badge()?.textContent).toBe('1');
    expect(getInv).toHaveBeenCalledTimes(1);

    // Advance one poll interval: despite being collapsed, the SSE-less dock
    // re-fetches and the badge settles to 0.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(getInv.mock.calls.length).toBeGreaterThan(1);
    expect(badge()?.textContent).toBe('0');

    // The poll self-limits: nothing live → no further fetches.
    const callsAfterSettle = getInv.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(getInv.mock.calls.length).toBe(callsAfterSettle);
  });

  it('an SSE-backed collapsed surface does NOT poll (its push keeps the badge fresh)', async () => {
    // WorkScopeSection is SSE-backed; collapsed (expanded=false) it must stay
    // inert — no poll — relying on the push channel. Guards against the
    // SSE-less fix leaking into the SSE-backed surface.
    getInv.mockResolvedValue(inv([bash({ state: 'running' })]));

    await act(async () => {
      render(
        sectionCollapsedTree({ scopeKey: 'ws-1', liveInventory: inv([bash({ state: 'running' })]) }),
      );
    });
    await act(async () => {
      await Promise.resolve();
    });

    const callsAfterMount = getInv.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    // No additional polls while collapsed + SSE-backed.
    expect(getInv.mock.calls.length).toBe(callsAfterMount);
  });
});

/** WorkScopeSection rendered collapsed (expanded=false) in its provider tree. */
function sectionCollapsedTree(props: { scopeKey: string; liveInventory?: WorkScopeInventory | null }) {
  return (
    <MemoryRouter initialEntries={['/c/conv-A']}>
      <Routes>
        <Route
          path="/c/:slug"
          element={
            <ViewerSlotProvider scopeKey="conv-A" browserSessionActive={false}>
              <WorkScopeSection
                scopeKey={props.scopeKey}
                liveInventory={props.liveInventory ?? null}
                expanded={false}
                onToggleExpanded={() => {}}
              />
            </ViewerSlotProvider>
          }
        />
      </Routes>
    </MemoryRouter>
  );
}

describe('inspect affordance + provider dependency', () => {
  it('a non-inspectable BashRow renders WITHOUT a ViewerSlotProvider (no hook, no throw)', async () => {
    // The standalone chain-page dock renders with inspectable={false} and is NOT
    // wrapped in a ViewerSlotProvider. A non-inspectable row must not call
    // useViewerSlot() (which throws outside a provider), so this renders cleanly.
    getInv.mockResolvedValue(inv([bash({ state: 'running' })]));

    await act(async () => {
      render(
        <WorkScopePanel scopeKey="ws-1" liveInventory={null} collapsed={false} onToggle={() => {}} />,
      );
    });
    await act(async () => {
      await Promise.resolve();
    });

    // The row rendered; opening its detail must not surface an inspect affordance.
    openBashDetail();
    expect(document.querySelector('.ws-row--bash')).toBeTruthy();
    expect(screen.queryByTitle('Open the process inspector for this handle')).toBeNull();
  });

  it('an inspectable BashRow inside a ViewerSlotProvider renders the inspect affordance', async () => {
    getInv.mockResolvedValue(inv([bash({ state: 'running' })]));

    await renderExpanded(); // sectionTree → WorkScopeSection (inspectable) inside the provider
    openBashDetail();
    expect(screen.getByTitle('Open the process inspector for this handle')).toBeTruthy();
    expect(screen.getByText('inspect →')).toBeTruthy();
  });
});

describe('browser open affordance (Phase 3)', () => {
  /** A WorkScopeSection (inspectable rows) over a fixed inventory, with a
   *  CaptureSlot sibling inside the same provider so the test can observe the
   *  slot transition the "open →" button drives. */
  function browserSectionTree(props: {
    inventory: WorkScopeInventory;
    onSlot: (slot: ReturnType<typeof useViewerSlot>['slot']) => void;
  }) {
    return (
      <MemoryRouter initialEntries={['/c/conv-A']}>
        <Routes>
          <Route
            path="/c/:slug"
            element={
              <ViewerSlotProvider scopeKey="conv-A" browserSessionActive={true}>
                <WorkScopeSection
                  scopeKey="ws-1"
                  liveInventory={props.inventory}
                  expanded={true}
                  onToggleExpanded={() => {}}
                />
                <CaptureSlot onSlot={props.onSlot} />
              </ViewerSlotProvider>
            }
          />
        </Routes>
      </MemoryRouter>
    );
  }

  it('a LIVE browser + inspectable rows renders open →; clicking it opens the browser slot', async () => {
    const liveBrowser = inv([], { browser: { state: 'live', idle_ms: 0 } });
    getInv.mockResolvedValue(liveBrowser);

    let slot: ReturnType<typeof useViewerSlot>['slot'] = { kind: 'none' };
    await act(async () => {
      render(browserSectionTree({ inventory: liveBrowser, onSlot: (s) => { slot = s; } }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    const open = screen.getByTestId('browser-open-button');
    expect(open).toBeTruthy();
    expect(screen.getByText('open →')).toBeTruthy();

    expect(slot).toEqual({ kind: 'none' });
    fireEvent.click(open);
    expect(slot).toEqual({ kind: 'browser' });
  });

  it('a LIVE browser row renders stop and calls the work-scope browser-session endpoint', async () => {
    const liveBrowser = inv([], { browser: { state: 'live', idle_ms: 0 } });
    getInv.mockResolvedValue(liveBrowser);
    stopBrowser.mockResolvedValue({ success: true });

    await act(async () => {
      render(browserSectionTree({ inventory: liveBrowser, onSlot: () => {} }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.getByTestId('browser-open-button')).toBeTruthy();
    const stop = screen.getByTestId('browser-stop-button');
    expect(stop).toBeTruthy();
    expect(screen.getByText('stop')).toBeTruthy();

    await act(async () => {
      fireEvent.click(stop);
      await Promise.resolve();
    });

    expect(stopBrowser).toHaveBeenCalledWith('ws-1');
  });

  it('browser stop uses the live inventory scope rather than the requested prop scope', async () => {
    const liveBrowser = inv([], {
      scope_key: 'worktree:/tmp/promoted',
      browser: { state: 'live', idle_ms: 0 },
    });
    getInv.mockResolvedValue(liveBrowser);
    stopBrowser.mockResolvedValue({ success: true });

    await act(async () => {
      render(browserSectionTree({ inventory: liveBrowser, onSlot: () => {} }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    await act(async () => {
      fireEvent.click(screen.getByTestId('browser-stop-button'));
      await Promise.resolve();
    });

    expect(stopBrowser).toHaveBeenCalledWith('worktree:/tmp/promoted');
  });

  it('stop failure is rendered visibly in the work-scope body', async () => {
    const liveBrowser = inv([], { browser: { state: 'live', idle_ms: 0 } });
    getInv.mockResolvedValue(liveBrowser);
    stopBrowser.mockRejectedValue(new Error('nope'));

    await act(async () => {
      render(browserSectionTree({ inventory: liveBrowser, onSlot: () => {} }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    await act(async () => {
      fireEvent.click(screen.getByTestId('browser-stop-button'));
      await Promise.resolve();
    });

    expect(screen.getByRole('alert').textContent).toContain('nope');
  });

  it('a torn_down browser does NOT render open → (even though rows are inspectable)', async () => {
    const deadBrowser = inv([], { browser: { state: 'torn_down', idle_ms: 0 } });
    getInv.mockResolvedValue(deadBrowser);

    await act(async () => {
      render(browserSectionTree({ inventory: deadBrowser, onSlot: () => {} }));
    });
    await act(async () => {
      await Promise.resolve();
    });

    // The browser row is present (torn down), but no open affordance.
    expect(document.querySelector('.ws-row--dead .ws-row-label')?.textContent).toBe('browser');
    expect(screen.queryByTestId('browser-open-button')).toBeNull();
    expect(screen.queryByTestId('browser-stop-button')).toBeNull();
  });

  it('a LIVE browser in a non-inspectable dock renders stop but not open →', async () => {
    // WorkScopePanel's standalone dock has no viewer renderer, so it omits open
    // but still offers scope-keyed session lifecycle control.
    const liveBrowser = inv([], { browser: { state: 'live', idle_ms: 0 } });
    getInv.mockResolvedValue(liveBrowser);

    await act(async () => {
      render(
        <WorkScopePanel
          scopeKey="ws-1"
          liveInventory={liveBrowser}
          collapsed={false}
          onToggle={() => {}}
        />,
      );
    });
    await act(async () => {
      await Promise.resolve();
    });

    const labels = Array.from(document.querySelectorAll('.ws-row-label')).map((n) => n.textContent);
    expect(labels).toContain('browser');
    expect(screen.queryByTestId('browser-open-button')).toBeNull();
    expect(screen.getByTestId('browser-stop-button')).toBeTruthy();
  });
});

describe('useSeededLiveCount (collapsed-badge seed)', () => {
  // A tiny harness that renders the hook's result as text.
  function Harness({
    scopeKey,
    live,
  }: {
    scopeKey: string | null | undefined;
    live: WorkScopeInventory | null | undefined;
  }) {
    const count = useSeededLiveCount(scopeKey, live);
    return <span data-testid="count">{count}</span>;
  }

  it('seeds the count from the inventory endpoint when no SSE value is present', async () => {
    getInv.mockResolvedValue(inv([bash({ state: 'running' })]));

    await act(async () => {
      render(<Harness scopeKey="ws-1" live={null} />);
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(getInv).toHaveBeenCalledWith('ws-1');
    expect(screen.getByTestId('count').textContent).toBe('1');
  });

  it('does not fetch when there is no scopeKey, and reports 0', async () => {
    await act(async () => {
      render(<Harness scopeKey={null} live={null} />);
    });
    expect(getInv).not.toHaveBeenCalled();
    expect(screen.getByTestId('count').textContent).toBe('0');
  });

  it('SSE value is authoritative over the seed once present', async () => {
    // Seed reports two live; SSE reports one live → SSE wins.
    getInv.mockResolvedValue(inv([bash({ state: 'running' }), bash({ state: 'running' })]));
    const sse = inv([bash({ state: 'running' })]);

    await act(async () => {
      render(<Harness scopeKey="ws-1" live={sse} />);
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(screen.getByTestId('count').textContent).toBe('1');
  });
});
