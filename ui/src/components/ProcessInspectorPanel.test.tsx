// Output accumulation + termination behaviour for the process inspector
// (specs/process-inspector/, REQ-PINSP-006 / REQ-PINSP-008). The inspector
// seeds with a no-`since` tail, then polls with `since = end_offset` and
// APPENDS the returned lines; a poll reporting `truncated_before` surfaces a
// gap marker; a terminal snapshot stops the poll and renders the final state.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import type { BashHandleInspection } from '../api';
import { api } from '../api';

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
