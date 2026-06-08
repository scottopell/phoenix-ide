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

  it('renders null resource metrics as unavailable, not zero', async () => {
    getInsp.mockResolvedValueOnce(
      // memory/process are absent (the wire null/skip → capability gap); cpu is
      // present. The readout must show `—` for the gaps, not `0`.
      snap({ resources: { cpu_pct: 12.5 } }),
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

  it('surfaces a seed failure as the load-failed empty state (no snapshot)', async () => {
    getInsp.mockRejectedValueOnce(new Error('boom'));

    await renderPanel();
    expect(screen.getByText('inspection failed to load')).toBeTruthy();
    expect(screen.queryByTestId('process-inspector-panel')).toBeTruthy();
  });
});
