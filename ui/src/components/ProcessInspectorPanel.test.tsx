// Output accumulation + termination behaviour for the process inspector
// (specs/process-inspector/, REQ-PINSP-006 / REQ-PINSP-008). The inspector
// seeds with a no-`since` tail, then polls with `since = end_offset` and
// APPENDS the returned lines; a poll reporting `truncated_before` surfaces a
// gap marker; a terminal snapshot stops the poll and renders the final state.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import type { BashHandleInspection } from '../api';
import { api, NotFoundError } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: { ...actual.api, getBashHandleInspection: vi.fn() },
  };
});

import { ProcessInspectorPanel } from './ProcessInspectorPanel';

const getInsp = vi.mocked(api.getBashHandleInspection);

function snap(over: Partial<BashHandleInspection> = {}): BashHandleInspection {
  return {
    handle_id: 'b-1',
    cmd: 'tail -f log',
    state: 'running',
    started_at: new Date().toISOString(),
    output: { start_offset: 0, end_offset: 0, truncated_before: false, lines: [] },
    ...over,
  };
}

function lines(...specs: [number, string][]) {
  return specs.map(([offset, bytes]) => ({ offset, bytes }));
}

async function renderPanel() {
  let utils!: ReturnType<typeof render>;
  await act(async () => {
    utils = render(<ProcessInspectorPanel scopeKey="ws-1" handleId="b-1" />);
    await Promise.resolve();
  });
  return utils;
}

beforeEach(() => {
  vi.useFakeTimers();
  getInsp.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('ProcessInspectorPanel — output accumulation', () => {
  it('seeds with no `since`, then polls with `since = end_offset`, appending lines', async () => {
    getInsp
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 0, end_offset: 2, truncated_before: false, lines: lines([0, 'first'], [1, 'second']) } }),
      )
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 2, end_offset: 3, truncated_before: false, lines: lines([2, 'third']) } }),
      );

    await renderPanel();
    // Seed call carries no `since`.
    expect(getInsp).toHaveBeenLastCalledWith('ws-1', 'b-1');
    expect(screen.getByText('first')).toBeTruthy();
    expect(screen.getByText('second')).toBeTruthy();

    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });

    // Poll call advances `since` to the prior `end_offset` (2).
    expect(getInsp).toHaveBeenLastCalledWith('ws-1', 'b-1', 2);
    // Prior lines retained, new line appended.
    expect(screen.getByText('first')).toBeTruthy();
    expect(screen.getByText('third')).toBeTruthy();
  });

  it('renders the live partial as a trailing in-progress line, replaced (not appended) each poll', async () => {
    getInsp
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'done']), partial: 'in-progr' } }),
      )
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 1, end_offset: 1, truncated_before: false, lines: [], partial: 'in-progress now' } }),
      );

    await renderPanel();
    expect(screen.getByText('done')).toBeTruthy();
    const partialEl = screen.getByText('in-progr');
    expect(partialEl.className).toContain('pinsp-output-line--partial');

    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });

    // Partial is transient: the prior value is gone, replaced by the latest.
    expect(screen.queryByText('in-progr')).toBeNull();
    expect(screen.getByText('in-progress now')).toBeTruthy();
    // The completed line stays (offset-keyed entries are append-only).
    expect(screen.getByText('done')).toBeTruthy();
  });

  it('surfaces truncated_before on the SEED response (earlier output already evicted), but not on a fresh non-truncated seed', async () => {
    // A seed whose no-`since` window already reports earlier output gone (the
    // ring evicted it, or a tombstone retains only a final tail). The marker is
    // a real signal here and must be shown.
    getInsp.mockResolvedValueOnce(
      snap({ output: { start_offset: 10, end_offset: 11, truncated_before: true, lines: lines([10, 'tail']) } }),
    );

    const { unmount } = await renderPanel();
    expect(screen.getByText(/output truncated/)).toBeTruthy();
    expect(screen.getByText('tail')).toBeTruthy();
    unmount();

    // A fresh handle with no eviction reports truncated_before=false → no marker.
    getInsp.mockReset();
    getInsp.mockResolvedValueOnce(
      snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'fresh']) } }),
    );
    await renderPanel();
    expect(screen.getByText('fresh')).toBeTruthy();
    expect(screen.queryByText(/output truncated/)).toBeNull();
  });

  it('renders a truncation marker when a poll reports truncated_before', async () => {
    getInsp
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'a']) } }),
      )
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 5, end_offset: 6, truncated_before: true, lines: lines([5, 'b']) } }),
      );

    await renderPanel();
    expect(screen.queryByText(/output truncated/)).toBeNull();

    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });

    expect(screen.getByText(/output truncated/)).toBeTruthy();
    expect(screen.getByText('a')).toBeTruthy();
    expect(screen.getByText('b')).toBeTruthy();
  });

  it('stops polling once the handle is terminal and shows the exit cause', async () => {
    getInsp.mockResolvedValueOnce(
      snap({
        state: 'tombstoned',
        exit_code: 0,
        duration_ms: 1500,
        output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'done']) },
      }),
    );

    await renderPanel();
    expect(screen.getByText('exit 0')).toBeTruthy();

    await act(async () => {
      vi.advanceTimersByTime(5000);
      await Promise.resolve();
    });

    // Only the seed fetch — no polling on a terminal handle.
    expect(getInsp).toHaveBeenCalledTimes(1);
  });

  it('caps accumulated entries at the scrollback bound, dropping the oldest while keeping the newest', async () => {
    const CAP = 5000;
    // Seed with one line, then drive enough polls that the running total of
    // appended lines exceeds the UI cap.
    getInsp.mockResolvedValueOnce(
      snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'line-0']) } }),
    );

    await renderPanel();
    expect(screen.getByText('line-0')).toBeTruthy();

    // Each poll appends a batch of lines. Total appended (seed + polls) climbs
    // well past CAP so the front must be trimmed.
    const BATCH = 1000;
    const POLLS = 7; // 1 (seed) + 7*1000 = 7001 lines total observed
    let offset = 1;
    for (let p = 0; p < POLLS; p++) {
      const batch: [number, string][] = [];
      for (let i = 0; i < BATCH; i++) {
        batch.push([offset, `line-${offset}`]);
        offset++;
      }
      getInsp.mockResolvedValueOnce(
        snap({ output: { start_offset: offset - BATCH, end_offset: offset, truncated_before: false, lines: lines(...batch) } }),
      );
      await act(async () => {
        vi.advanceTimersByTime(1000);
        await Promise.resolve();
      });
    }

    const total = 1 + POLLS * BATCH; // 7001
    const newest = total - 1; // last offset observed
    // The newest line is retained…
    expect(screen.getByText(`line-${newest}`)).toBeTruthy();
    // …the line exactly at the cap boundary (the oldest survivor) is retained…
    expect(screen.getByText(`line-${total - CAP}`)).toBeTruthy();
    // …and lines older than the cap window are dropped.
    expect(screen.queryByText('line-0')).toBeNull();
    expect(screen.queryByText(`line-${total - CAP - 1}`)).toBeNull();
    // Exactly CAP line rows remain.
    const rendered = document.querySelectorAll('.pinsp-output-line:not(.pinsp-output-line--partial)');
    expect(rendered.length).toBe(CAP);
  });

  it('stays pinned to the bottom on a partial-only update while following (no new full lines)', async () => {
    // A command emitting a growing un-newlined PARTIAL line: `entries` does not
    // change across the poll, only `snapshot.output.partial` grows. A following
    // viewer must stay pinned to the bottom — the autoscroll effect has to re-run
    // on partial growth, not only on completed-line growth.
    getInsp
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'header']), partial: 'progress 10%' } }),
      )
      .mockResolvedValueOnce(
        // Same end_offset / no new full lines → `entries` is unchanged. Only the
        // partial grows.
        snap({ output: { start_offset: 1, end_offset: 1, truncated_before: false, lines: [], partial: 'progress 10% .......... 80%' } }),
      );

    await renderPanel();

    // jsdom does not lay out, so drive scrollHeight/clientHeight by hand. A
    // following viewer (scrollTop tracks scrollHeight) pins to the bottom.
    const output = screen.getByText('header').closest('.pinsp-output') as HTMLDivElement;
    Object.defineProperty(output, 'clientHeight', { configurable: true, value: 100 });
    let height = 200;
    Object.defineProperty(output, 'scrollHeight', { configurable: true, get: () => height });

    // Seed established the partial; grow it on the next poll without new lines.
    expect(screen.getByText('progress 10%')).toBeTruthy();
    height = 600; // the wrapped partial grew the scroll height

    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });

    // No new full lines — only the partial changed.
    expect(screen.getByText('progress 10% .......... 80%')).toBeTruthy();
    // The follow effect re-ran on the partial change and re-pinned to the bottom.
    expect(output.scrollTop).toBe(600);
  });

  it('renders null resource metrics as unavailable, not zero', async () => {
    getInsp.mockResolvedValueOnce(
      // memory/process are null on the wire (capability gap); cpu is present.
      // The readout must show `—` for the gaps, not `0`.
      snap({ resources: { cpu_pct: 12.5, memory_bytes: null, process_count: null } }),
    );

    await renderPanel();
    expect(screen.getByText('12.5%')).toBeTruthy();
    // Two null metrics → two em-dashes.
    expect(screen.getAllByText('—').length).toBe(2);
  });
});

/** A controllable promise so a test can resolve a fetch by hand and exercise
 *  slow / out-of-order resolution. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe('ProcessInspectorPanel — poll serialization', () => {
  it('does not issue a second overlapping poll while one is in flight, and an out-of-order resolution cannot duplicate lines or regress the cursor', async () => {
    // Seed resolves immediately at end_offset 1.
    getInsp.mockResolvedValueOnce(
      snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'seed']) } }),
    );

    // The first poll is slow: we hold its promise open across multiple
    // intervals to simulate the inspect endpoint outlasting the 1s cadence.
    const slowPoll = deferred<BashHandleInspection>();
    getInsp.mockReturnValueOnce(slowPoll.promise);

    await renderPanel();
    expect(screen.getByText('seed')).toBeTruthy();
    // Seed only so far.
    expect(getInsp).toHaveBeenCalledTimes(1);

    // First interval fires the (slow) poll with since = 1.
    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(2);
    expect(getInsp).toHaveBeenLastCalledWith('ws-1', 'b-1', 1);

    // Several more intervals fire while the first poll is still outstanding.
    // The in-flight gate must suppress every one of them — no second fetch.
    await act(async () => {
      vi.advanceTimersByTime(3000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(2);

    // The slow poll finally resolves with lines [1,'a'],[2,'b'] → cursor 3.
    await act(async () => {
      slowPoll.resolve(
        snap({ output: { start_offset: 1, end_offset: 3, truncated_before: false, lines: lines([1, 'a'], [2, 'b']) } }),
      );
      await Promise.resolve();
    });
    expect(screen.getByText('a')).toBeTruthy();
    expect(screen.getByText('b')).toBeTruthy();

    // Next interval issues exactly one fresh poll, and from the ADVANCED
    // cursor (3) — proving the slow poll applied before any successor ran, so
    // the cursor never regressed and lines can't be re-fetched/duplicated.
    getInsp.mockResolvedValueOnce(
      snap({ output: { start_offset: 3, end_offset: 4, truncated_before: false, lines: lines([3, 'c']) } }),
    );
    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(3);
    expect(getInsp).toHaveBeenLastCalledWith('ws-1', 'b-1', 3);

    // No duplicate 'a'/'b' lines (offset-keyed entries appended once).
    expect(screen.getAllByText('a').length).toBe(1);
    expect(screen.getAllByText('b').length).toBe(1);
    expect(screen.getByText('c')).toBeTruthy();
  });

  it('a late poll from a PRIOR target cannot clear the new target\'s in-flight gate (no overlapping fetch on the new target)', async () => {
    // Target A: seed resolves, then a slow poll goes in flight.
    getInsp.mockResolvedValueOnce(
      snap({ handle_id: 'a-1', output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'A-seed']) } }),
    );
    const slowPollA = deferred<BashHandleInspection>();
    getInsp.mockReturnValueOnce(slowPollA.promise);

    let utils!: ReturnType<typeof render>;
    await act(async () => {
      utils = render(<ProcessInspectorPanel scopeKey="ws-1" handleId="a-1" />);
      await Promise.resolve();
    });
    expect(screen.getByText('A-seed')).toBeTruthy();

    // A's poll fires and is left outstanding.
    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(2); // A seed + A slow poll
    expect(getInsp).toHaveBeenLastCalledWith('ws-1', 'a-1', 1);

    // Switch to target B while A's poll is still pending.
    getInsp.mockResolvedValueOnce(
      snap({ handle_id: 'b-2', output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'B-seed']) } }),
    );
    const slowPollB = deferred<BashHandleInspection>();
    getInsp.mockReturnValueOnce(slowPollB.promise);
    await act(async () => {
      utils.rerender(<ProcessInspectorPanel scopeKey="ws-1" handleId="b-2" />);
      await Promise.resolve();
    });
    expect(screen.getByText('B-seed')).toBeTruthy();
    expect(getInsp).toHaveBeenCalledTimes(3); // + B seed

    // B's poll fires and is left outstanding.
    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(4); // + B slow poll
    expect(getInsp).toHaveBeenLastCalledWith('ws-1', 'b-2', 1);

    // Now A's stale poll resolves LATE. Its `.finally` must NOT clear the shared
    // in-flight gate — B still owns it.
    await act(async () => {
      slowPollA.resolve(
        snap({ handle_id: 'a-1', output: { start_offset: 1, end_offset: 2, truncated_before: false, lines: lines([1, 'A-late']) } }),
      );
      await Promise.resolve();
    });

    // The next interval must NOT issue a second B fetch: B's gate is still held.
    await act(async () => {
      vi.advanceTimersByTime(3000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(4);

    // A's late output is for the abandoned target and must not leak into B's view.
    expect(screen.queryByText('A-late')).toBeNull();
    expect(screen.getByText('B-seed')).toBeTruthy();

    // When B's own poll resolves, the gate clears and a fresh B poll can run.
    getInsp.mockResolvedValueOnce(
      snap({ handle_id: 'b-2', output: { start_offset: 1, end_offset: 2, truncated_before: false, lines: lines([1, 'B-next']) } }),
    );
    await act(async () => {
      slowPollB.resolve(
        snap({ handle_id: 'b-2', output: { start_offset: 1, end_offset: 1, truncated_before: false, lines: [] } }),
      );
      await Promise.resolve();
    });
    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(5);
    expect(getInsp).toHaveBeenLastCalledWith('ws-1', 'b-2', 1);
  });
});

describe('ProcessInspectorPanel — poll failure surfacing', () => {
  it('surfaces a stale indicator on a poll failure after a successful seed while keeping the last snapshot', async () => {
    getInsp
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'live output']) } }),
      )
      .mockRejectedValueOnce(new Error('network blip'));

    await renderPanel();
    expect(screen.getByText('live output')).toBeTruthy();
    // No stale banner while healthy.
    expect(screen.queryByText(/stale/)).toBeNull();

    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });

    // The failing poll surfaces a non-destructive stale indicator…
    expect(screen.getByText(/data below may be stale/)).toBeTruthy();
    // …and the last-known output is still on screen.
    expect(screen.getByText('live output')).toBeTruthy();
  });

  it('recovers from stale back to healthy when a later poll succeeds', async () => {
    getInsp
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'live output']) } }),
      )
      .mockRejectedValueOnce(new Error('network blip'))
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 1, end_offset: 2, truncated_before: false, lines: lines([1, 'recovered']) } }),
      );

    await renderPanel();
    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(screen.getByText(/data below may be stale/)).toBeTruthy();

    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(screen.queryByText(/data below may be stale/)).toBeNull();
    expect(screen.getByText('recovered')).toBeTruthy();
  });

  it('treats a 404 as a definitive "handle no longer exists" state and stops polling', async () => {
    getInsp
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'last output']) } }),
      )
      .mockRejectedValueOnce(new NotFoundError('Bash handle no longer exists'));

    await renderPanel();
    expect(getInsp).toHaveBeenCalledTimes(1);

    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });

    // The 404 surfaces a definitive "no longer exists" indicator, keeping the
    // last-known output.
    expect(screen.getByText(/handle no longer exists/)).toBeTruthy();
    expect(screen.getByText('last output')).toBeTruthy();
    expect(getInsp).toHaveBeenCalledTimes(2);

    // Polling has stopped — further intervals issue no new fetches.
    await act(async () => {
      vi.advanceTimersByTime(5000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(2);
  });

  it('renders a terminal "handle gone" state when the FIRST fetch 404s (no prior snapshot) and stops polling', async () => {
    // A persisted inspector URL opened after Phoenix lost its handle table: the
    // very first inspect request 404s, so there is no snapshot to fall back on.
    getInsp.mockRejectedValueOnce(new NotFoundError('Bash handle no longer exists'));

    await renderPanel();
    // The panel still renders, and tells the user the handle is gone.
    expect(screen.queryByTestId('process-inspector-panel')).toBeTruthy();
    expect(screen.getByText(/handle no longer exists/)).toBeTruthy();
    expect(getInsp).toHaveBeenCalledTimes(1);

    // A 404 is terminal: no polling ever starts (no snapshot gate to satisfy,
    // and the handle is marked gone regardless).
    await act(async () => {
      vi.advanceTimersByTime(5000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(1);
  });

  it('surfaces a seed failure as the load-failed empty state (no snapshot)', async () => {
    getInsp.mockRejectedValueOnce(new Error('boom'));

    await renderPanel();
    expect(screen.getByText('inspection failed to load')).toBeTruthy();
    expect(screen.queryByTestId('process-inspector-panel')).toBeTruthy();
  });

  it('retries a transient seed failure on the next interval and recovers to a normal snapshot', async () => {
    // First seed fails transiently (non-404), so no snapshot lands.
    getInsp
      .mockRejectedValueOnce(new Error('seed blip'))
      // The retry on the next interval succeeds.
      .mockResolvedValueOnce(
        snap({ output: { start_offset: 0, end_offset: 1, truncated_before: false, lines: lines([0, 'seeded after retry']) } }),
      );

    await renderPanel();
    // The seed-failed empty state is shown, and only the seed fetch has run.
    expect(screen.getByText('inspection failed to load')).toBeTruthy();
    expect(getInsp).toHaveBeenCalledTimes(1);

    // The recurring effect retries the seed on the interval — with no `since`,
    // since the cursor never advanced past the failed seed.
    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(2);
    // The retry carries no advanced cursor (the failed seed never set one).
    expect(getInsp).toHaveBeenLastCalledWith('ws-1', 'b-1', undefined);

    // Recovered: the empty error state is gone and the snapshot's output renders.
    expect(screen.queryByText('inspection failed to load')).toBeNull();
    expect(screen.getByText('seeded after retry')).toBeTruthy();
  });

  it('does NOT retry when the first seed 404s — it stops on the definitive gone state', async () => {
    getInsp.mockRejectedValueOnce(new NotFoundError('Bash handle no longer exists'));

    await renderPanel();
    expect(screen.getByText(/handle no longer exists/)).toBeTruthy();
    expect(getInsp).toHaveBeenCalledTimes(1);

    // A 404 seed is terminal: no retry fires on subsequent intervals.
    await act(async () => {
      vi.advanceTimersByTime(5000);
      await Promise.resolve();
    });
    expect(getInsp).toHaveBeenCalledTimes(1);
  });
});
