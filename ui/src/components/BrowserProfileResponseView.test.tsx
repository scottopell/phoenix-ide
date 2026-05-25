import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { BrowserProfileResponseView } from './BrowserProfileResponseView';

describe('BrowserProfileResponseView', () => {
  describe('run_scenario', () => {
    const sampleData = {
      outcome: 'completed',
      requested_runs: 3,
      warmup: 1,
      methodology_warnings: ['React in production build — react_actual_ms not measured'],
      raw_samples: [
        {
          run_index: 0,
          script_ms: 12.5,
          long_tasks: 1,
          wall_ms: 100.5,
          dom_nodes: 1500,
          gc_ran: true,
          js_heap_used: 2_000_000,
          react_status: 'absent',
          react_commits: null,
          react_actual_ms: null,
        },
        {
          run_index: 1,
          script_ms: 13.1,
          long_tasks: 1,
          wall_ms: 101.0,
          dom_nodes: 1500,
          gc_ran: true,
          js_heap_used: 2_100_000,
          react_status: 'absent',
          react_commits: null,
          react_actual_ms: null,
        },
        {
          run_index: 2,
          script_ms: 11.8,
          long_tasks: 0,
          wall_ms: 99.2,
          dom_nodes: 1500,
          gc_ran: true,
          js_heap_used: 2_050_000,
          react_status: 'absent',
          react_commits: null,
          react_actual_ms: null,
        },
      ],
    };

    it('renders completed outcome, run count, and warning chip', () => {
      render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={sampleData}
          fallbackText=""
          isError={false}
        />,
      );
      expect(screen.getByText('✓ completed')).toBeInTheDocument();
      expect(screen.getByText('3/3 runs')).toBeInTheDocument();
      expect(screen.getByText('+1 warmup discarded')).toBeInTheDocument();
      expect(screen.getByText('1 warning')).toBeInTheDocument();
    });

    it('renders sparkline rows for non-null metrics and hides absent React rows', () => {
      const { container } = render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={sampleData}
          fallbackText=""
          isError={false}
        />,
      );
      // Metric labels for non-null fields surface.
      expect(screen.getByText('script')).toBeInTheDocument();
      expect(screen.getByText('wall')).toBeInTheDocument();
      expect(screen.getByText('JS heap')).toBeInTheDocument();
      // React absent → React rows omitted.
      expect(screen.queryByText('React actual')).toBeNull();
      expect(screen.queryByText('React commits')).toBeNull();
      // One <svg> sparkline per visible metric row.
      expect(container.querySelectorAll('svg.profile-sparkline').length).toBeGreaterThan(0);
    });

    it('reveals the per-run table on click', () => {
      const { container } = render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={sampleData}
          fallbackText=""
          isError={false}
        />,
      );
      // Table absent before click.
      expect(container.querySelector('.profile-per-run-table')).toBeNull();
      fireEvent.click(screen.getByText(/Per-run table/));
      const table = container.querySelector('.profile-per-run-table');
      expect(table).not.toBeNull();
      // Three data rows for three runs.
      expect(table!.querySelectorAll('tbody tr')).toHaveLength(3);
    });

    it('shows "last —" when the final run has no value for a metric', () => {
      // Final run has gc_ran=false → js_heap_used is null. "last" must be
      // the literal final value (rendered as —), NOT a previous run's value.
      const data = {
        outcome: 'completed',
        requested_runs: 2,
        warmup: 0,
        methodology_warnings: [],
        raw_samples: [
          {
            run_index: 0,
            script_ms: 10,
            long_tasks: 0,
            wall_ms: 100,
            dom_nodes: 1000,
            gc_ran: true,
            js_heap_used: 2_000_000,
            react_status: 'absent',
            react_commits: null,
            react_actual_ms: null,
          },
          {
            run_index: 1,
            script_ms: 11,
            long_tasks: 0,
            wall_ms: 105,
            dom_nodes: 1000,
            gc_ran: false,
            js_heap_used: null,
            react_status: 'absent',
            react_commits: null,
            react_actual_ms: null,
          },
        ],
      };
      const { container } = render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={data}
          fallbackText=""
          isError={false}
        />,
      );
      const heapRow = Array.from(
        container.querySelectorAll('.profile-metric-row'),
      ).find((row) => row.textContent?.includes('JS heap'));
      expect(heapRow).toBeTruthy();
      expect(heapRow!.textContent).toMatch(/last —/);
    });

    it('clamps unknown outcome strings to a safe CSS-class suffix', () => {
      // Defensive: outcome flows into className. An unexpected string
      // (spaces, punctuation, future variants) must not become part of
      // the class selector.
      const { container } = render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={{
            outcome: 'weird value; .injected',
            requested_runs: 1,
            warmup: 0,
            methodology_warnings: [],
            raw_samples: [],
          }}
          fallbackText=""
          isError={false}
        />,
      );
      const chip = container.querySelector('.profile-action-chip');
      expect(chip).not.toBeNull();
      // Class list contains only profile-outcome-{completed|blocked|unknown}.
      const classes = chip!.className.split(/\s+/);
      const outcomeClasses = classes.filter((c) => c.startsWith('profile-outcome-'));
      expect(outcomeClasses).toHaveLength(1);
      expect(outcomeClasses[0]).toMatch(/^profile-outcome-(completed|blocked|unknown)$/);
    });

    it('exposes aria-expanded on the per-run table and raw-payload toggles', () => {
      render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={sampleData}
          fallbackText=""
          isError={false}
        />,
      );
      const perRunBtn = screen.getByRole('button', { name: /Per-run table/ });
      expect(perRunBtn).toHaveAttribute('aria-expanded', 'false');
      fireEvent.click(perRunBtn);
      expect(perRunBtn).toHaveAttribute('aria-expanded', 'true');

      const rawBtn = screen.getByRole('button', { name: /Raw payload/ });
      expect(rawBtn).toHaveAttribute('aria-expanded', 'false');
      fireEvent.click(rawBtn);
      expect(rawBtn).toHaveAttribute('aria-expanded', 'true');
    });

    it('breaks the sparkline polyline at null gaps so non-measured runs do not visually interpolate', () => {
      // Runs 0 and 2 measure heap (gc_ran=true); run 1 does NOT (gc_ran=false).
      // A single polyline through points 0 and 2 would draw a straight line
      // implying continuous heap growth across run 1 — a quiet lie. The fix
      // must split the polyline at the null gap so the absence is visible.
      const data = {
        outcome: 'completed',
        requested_runs: 3,
        warmup: 0,
        methodology_warnings: [],
        raw_samples: [
          {
            run_index: 0,
            script_ms: 10,
            long_tasks: 0,
            wall_ms: 100,
            dom_nodes: 1000,
            gc_ran: true,
            js_heap_used: 1_000_000,
            react_status: 'absent',
            react_commits: null,
            react_actual_ms: null,
          },
          {
            run_index: 1,
            script_ms: 11,
            long_tasks: 0,
            wall_ms: 105,
            dom_nodes: 1000,
            gc_ran: false,
            js_heap_used: null,
            react_status: 'absent',
            react_commits: null,
            react_actual_ms: null,
          },
          {
            run_index: 2,
            script_ms: 12,
            long_tasks: 0,
            wall_ms: 110,
            dom_nodes: 1000,
            gc_ran: true,
            js_heap_used: 3_000_000,
            react_status: 'absent',
            react_commits: null,
            react_actual_ms: null,
          },
        ],
      };
      const { container } = render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={data}
          fallbackText=""
          isError={false}
        />,
      );
      const heapRow = Array.from(container.querySelectorAll('.profile-metric-row')).find(
        (row) => row.textContent?.includes('JS heap'),
      );
      expect(heapRow).toBeTruthy();
      // No <polyline> in the heap row connects two points across a null gap.
      // Equivalent: no single polyline has both non-null endpoints — they
      // must be in separate polyline elements (each with ≤1 point) or a
      // path with M (move) breaks. We enforce: no polyline contains 2+ points.
      const polylines = heapRow!.querySelectorAll('polyline');
      for (const pl of polylines) {
        const points = (pl.getAttribute('points') ?? '').trim();
        const count = points === '' ? 0 : points.split(/\s+/).length;
        expect(count).toBeLessThanOrEqual(1);
      }
    });

    it('still draws a connected polyline when all points are contiguous', () => {
      // Sanity-check the fix doesn't over-restrict: an all-non-null metric
      // (script_ms) MUST still render one polyline that spans the runs.
      const { container } = render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={sampleData}
          fallbackText=""
          isError={false}
        />,
      );
      const scriptRow = Array.from(container.querySelectorAll('.profile-metric-row')).find(
        (row) => row.textContent?.startsWith('script'),
      );
      expect(scriptRow).toBeTruthy();
      const polylines = scriptRow!.querySelectorAll('polyline');
      // At least one polyline with ≥2 points (the line connecting runs).
      const multiPointSegments = Array.from(polylines).filter((pl) => {
        const pts = (pl.getAttribute('points') ?? '').trim();
        return pts !== '' && pts.split(/\s+/).length >= 2;
      });
      expect(multiPointSegments.length).toBeGreaterThanOrEqual(1);
    });

    it('renders a malformed payload without throwing (undefined / non-numeric metric values)', () => {
      // displayData is typed as Record<string, unknown> — an upstream
      // schema drift or a hand-crafted payload could deliver `undefined`
      // or a string where a number is expected. The renderer must not
      // call .toFixed() on those values — it should treat them as null
      // and render "—".
      const malformed = {
        outcome: 'completed',
        requested_runs: 1,
        warmup: 0,
        methodology_warnings: [],
        raw_samples: [
          {
            run_index: 0,
            // script_ms intentionally MISSING (undefined)
            long_tasks: 'oops', // wrong type
            wall_ms: 100,
            // dom_nodes intentionally MISSING
            gc_ran: true,
            js_heap_used: null,
            react_status: 'absent',
            react_commits: null,
            react_actual_ms: null,
          },
        ],
      };
      // Should not throw, and should render the table with "—" for the
      // non-numeric / undefined cells.
      const { container } = render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={malformed}
          fallbackText=""
          isError={false}
        />,
      );
      // Open the per-run table.
      fireEvent.click(screen.getByText(/Per-run table/));
      const tbody = container.querySelector('.profile-per-run-table tbody');
      expect(tbody).not.toBeNull();
      // No `undefined` text leaked. No NaN. (And no throw, implicit by
      // reaching this line.)
      expect(tbody!.textContent ?? '').not.toMatch(/undefined|NaN/);
    });

    it('renders blocked outcome with the failing step', () => {
      render(
        <BrowserProfileResponseView
          action="run_scenario"
          displayData={{
            outcome: 'blocked',
            blocked_step: 'wait_selector timed out: #ready',
            raw_samples: [],
            methodology_warnings: [],
          }}
          fallbackText=""
          isError={true}
        />,
      );
      expect(screen.getByText('✗ blocked')).toBeInTheDocument();
      expect(screen.getByText(/wait_selector timed out/)).toBeInTheDocument();
    });
  });

  describe('cpu_stop / cpu_summary', () => {
    const cpuData = {
      cpu_summary: {
        path: '/tmp/phoenix-cpu-profile-abc.json',
        hitcount_fallback: false,
        total: 401.5,
        top_by_self: [
          { label: 'busyLoop  app.js:42', value: 380.0, percent: 94.6 },
          { label: 'sqrt  (native)', value: 21.5, percent: 5.4 },
        ],
        top_by_total: [
          { label: '(root)', value: 401.5, percent: 100.0 },
          { label: 'busyLoop  app.js:42', value: 380.0, percent: 94.6 },
        ],
      },
    };

    it('renders sampled wall time and hot-function rows', () => {
      render(
        <BrowserProfileResponseView
          action="cpu_stop"
          displayData={cpuData}
          fallbackText=""
          isError={false}
        />,
      );
      expect(screen.getByText('CPU profile')).toBeInTheDocument();
      expect(screen.getByText(/sampled 401\.5 ms/)).toBeInTheDocument();
      // busyLoop appears in both top_by_self and top_by_total.
      expect(screen.getAllByText(/busyLoop\s+app\.js:42/).length).toBeGreaterThanOrEqual(2);
      expect(screen.getByText('Top by SELF time')).toBeInTheDocument();
      expect(screen.getByText('Top call-tree nodes by TOTAL time')).toBeInTheDocument();
    });

    it('flags hitcount fallback when samples absent', () => {
      render(
        <BrowserProfileResponseView
          action="cpu_summary"
          displayData={{
            cpu_summary: {
              path: '/tmp/x.json',
              hitcount_fallback: true,
              total: 7,
              top_by_self: [{ label: 'f a.js:1', value: 7.0, percent: 100.0 }],
              top_by_total: [],
            },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      expect(screen.getByText(/hitCount fallback/)).toBeInTheDocument();
      expect(screen.getByText('7.0hits')).toBeInTheDocument();
    });

    it('falls back to the text status line when display_data is missing', () => {
      render(
        <BrowserProfileResponseView
          action="cpu_stop"
          displayData={undefined}
          fallbackText="CPU profile is empty (no nodes — was the session too short?)."
          isError={false}
        />,
      );
      expect(screen.getByText(/CPU profile is empty/)).toBeInTheDocument();
    });

    it('survives malformed hot-function entries (drops the bad rows, clamps negative percent)', () => {
      // Schema drift / hand-rolled payload: bad value type, missing
      // percent, non-string label, negative percent. These must not
      // crash render or leak invalid CSS — `displayData` is untyped at
      // runtime so the renderer has to be defensive.
      const badPayload = {
        cpu_summary: {
          path: '/tmp/x.json',
          hitcount_fallback: false,
          total: 100,
          top_by_self: [
            { label: 'good fn', value: 50.0, percent: 50.0 },
            { label: 'bad value', value: 'oops', percent: 10.0 },
            { label: 'missing percent', value: 5.0 /* no percent */ },
            { label: 12345, value: 3.0, percent: 3.0 }, // wrong label type
            { label: 'negative pct', value: 1.0, percent: -5 },
            { label: 'huge pct', value: 1.0, percent: 999 },
          ],
          top_by_total: [],
        },
      };
      const { container } = render(
        <BrowserProfileResponseView
          action="cpu_stop"
          displayData={badPayload}
          fallbackText=""
          isError={false}
        />,
      );
      // No NaN/undefined leaked into the table.
      const cpuBlock = container.querySelector('.profile-cpu');
      expect(cpuBlock).not.toBeNull();
      expect(cpuBlock!.textContent ?? '').not.toMatch(/NaN|undefined/);
      // Good row still renders.
      expect(screen.getByText('good fn')).toBeInTheDocument();
      // Every bar-fill width is a valid percentage in [0, 100].
      const bars = cpuBlock!.querySelectorAll('.profile-hot-bar-fill');
      for (const bar of bars) {
        const w = (bar as HTMLElement).style.width;
        const m = w.match(/^([\d.]+)%$/);
        expect(m).not.toBeNull();
        const pct = parseFloat(m![1]!);
        expect(pct).toBeGreaterThanOrEqual(0);
        expect(pct).toBeLessThanOrEqual(100);
      }
    });

    it('fallback status chip preserves the actual action name', () => {
      const { rerender } = render(
        <BrowserProfileResponseView
          action="cpu_stop"
          displayData={undefined}
          fallbackText="empty"
          isError={false}
        />,
      );
      expect(screen.getByText(/profile · cpu_stop/)).toBeInTheDocument();
      rerender(
        <BrowserProfileResponseView
          action="cpu_summary"
          displayData={undefined}
          fallbackText="empty"
          isError={false}
        />,
      );
      expect(screen.getByText(/profile · cpu_summary/)).toBeInTheDocument();
    });
  });

  describe('trace_stop', () => {
    it('renders event count, long-task rows, and saved trace path', () => {
      render(
        <BrowserProfileResponseView
          action="trace_stop"
          displayData={{
            trace: {
              path: '/tmp/phoenix-trace-abc.json',
              event_count: 5234,
              long_task_count: 2,
              long_task_total_ms: 145.7,
              long_tasks: [
                { name: 'RunTask', ms: 100.2 },
                { name: 'ParseHTML', ms: 45.5 },
              ],
              timed_out: false,
            },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      expect(screen.getByText('trace')).toBeInTheDocument();
      expect(screen.getByText('5,234 events')).toBeInTheDocument();
      expect(screen.getByText(/2 long tasks/)).toBeInTheDocument();
      expect(screen.getByText('RunTask')).toBeInTheDocument();
      expect(screen.getByText('ParseHTML')).toBeInTheDocument();
      expect(screen.getByText(/chrome:\/\/tracing/)).toBeInTheDocument();
    });

    it('drops malformed long-task entries without crashing', () => {
      const tasks: unknown[] = [
        { name: 'good', ms: 75 },
        { name: 'missing ms' /* no ms */ },
        { name: 42, ms: 50 }, // wrong name type
        { name: 'string ms', ms: 'oops' },
        { name: 'NaN ms', ms: NaN },
        { name: 'good 2', ms: 60 },
      ];
      const { container } = render(
        <BrowserProfileResponseView
          action="trace_stop"
          displayData={{
            trace: {
              path: '/tmp/x.json',
              event_count: 10,
              long_task_count: tasks.length,
              long_task_total_ms: 185,
              long_tasks: tasks,
              timed_out: false,
            },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      const traceBlock = container.querySelector('.profile-trace');
      expect(traceBlock).not.toBeNull();
      // Good rows kept; bad rows dropped; no NaN/undefined leaked.
      expect(screen.getByText('good')).toBeInTheDocument();
      expect(screen.getByText('good 2')).toBeInTheDocument();
      expect(traceBlock!.textContent ?? '').not.toMatch(/NaN|undefined/);
    });

    it('omits the chrome://tracing footnote when path is absent', () => {
      render(
        <BrowserProfileResponseView
          action="trace_stop"
          displayData={{
            trace: {
              event_count: 10,
              long_task_count: 0,
              long_task_total_ms: 0,
              long_tasks: [],
              timed_out: false,
            },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      // No "undefined" string anywhere; no chrome://tracing hint.
      expect(screen.queryByText(/undefined/)).toBeNull();
      expect(screen.queryByText(/chrome:\/\/tracing/)).toBeNull();
    });

    it('shows partial-trace banner when tracingComplete timed out', () => {
      render(
        <BrowserProfileResponseView
          action="trace_stop"
          displayData={{
            trace: {
              path: '/tmp/x.json',
              event_count: 100,
              long_task_count: 0,
              long_task_total_ms: 0,
              long_tasks: [],
              timed_out: true,
            },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      expect(screen.getByText(/partial trace/)).toBeInTheDocument();
      expect(screen.getByText(/No long tasks/)).toBeInTheDocument();
    });

    it('collapses long-task list to top 5 with expand toggle', () => {
      const tasks = Array.from({ length: 8 }, (_, i) => ({
        name: `Task${i}`,
        ms: 100 - i,
      }));
      render(
        <BrowserProfileResponseView
          action="trace_stop"
          displayData={{
            trace: {
              path: '/tmp/x.json',
              event_count: 1,
              long_task_count: 8,
              long_task_total_ms: 800,
              long_tasks: tasks,
              timed_out: false,
            },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      // Collapsed: 5 tasks visible, 3 hidden.
      expect(screen.getByText('Task0')).toBeInTheDocument();
      expect(screen.getByText('Task4')).toBeInTheDocument();
      expect(screen.queryByText('Task5')).toBeNull();
      fireEvent.click(screen.getByText(/Show all 8 long tasks/));
      expect(screen.getByText('Task7')).toBeInTheDocument();
    });

    it('omits the empty parens when long_task_total_ms is missing', () => {
      render(
        <BrowserProfileResponseView
          action="trace_stop"
          displayData={{
            trace: {
              path: '/tmp/x.json',
              event_count: 1,
              long_task_count: 3,
              // long_task_total_ms intentionally omitted
              long_tasks: [],
              timed_out: false,
            },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      // No empty "( )" or "()" group floating after the long-task count.
      const content = document.body.textContent ?? '';
      expect(content).not.toMatch(/\(\s*\)/);
    });

    it('exposes aria-expanded on the long-task disclosure toggle', () => {
      const tasks = Array.from({ length: 8 }, (_, i) => ({ name: `T${i}`, ms: 100 - i }));
      render(
        <BrowserProfileResponseView
          action="trace_stop"
          displayData={{
            trace: {
              path: '/tmp/x.json',
              event_count: 1,
              long_task_count: 8,
              long_task_total_ms: 800,
              long_tasks: tasks,
              timed_out: false,
            },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      const toggle = screen.getByRole('button', { name: /Show all 8 long tasks/ });
      expect(toggle).toHaveAttribute('aria-expanded', 'false');
      fireEvent.click(toggle);
      expect(toggle).toHaveAttribute('aria-expanded', 'true');
    });
  });

  describe('heap_snapshot', () => {
    it('renders node + size deltas and detached-DOM warning when increasing', () => {
      render(
        <BrowserProfileResponseView
          action="heap_snapshot"
          displayData={{
            baseline: '/tmp/heap-a.heapsnapshot',
            post: '/tmp/heap-b.heapsnapshot',
            node_count_delta: 587,
            self_size_delta_bytes: 614 * 1024,
            retained_size_approximate: true,
            detached_dom_nodes: { baseline: 12, post: 47 },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      expect(screen.getByText('heap diff')).toBeInTheDocument();
      expect(screen.getByText('+587')).toBeInTheDocument();
      expect(screen.getByText(/\+614\.0 KB/)).toBeInTheDocument();
      expect(screen.getByText(/12 → 47/)).toBeInTheDocument();
      expect(screen.getByText(/retained-size approximated/)).toBeInTheDocument();
    });
  });

  describe('metrics', () => {
    it('renders Performance.getMetrics as an aligned table', () => {
      render(
        <BrowserProfileResponseView
          action="metrics"
          displayData={{
            metrics: {
              ScriptDuration: 0.123,
              JSHeapUsedSize: 2_500_000,
              Nodes: 1234,
            },
          }}
          fallbackText=""
          isError={false}
        />,
      );
      expect(screen.getByText('ScriptDuration')).toBeInTheDocument();
      expect(screen.getByText('0.123 s')).toBeInTheDocument();
      expect(screen.getByText(/2\.4 MB/)).toBeInTheDocument();
      expect(screen.getByText('Nodes')).toBeInTheDocument();
      expect(screen.getByText('1,234')).toBeInTheDocument();
    });
  });

  describe('status-line fallback', () => {
    it('renders unknown action as a status chip + raw text', () => {
      render(
        <BrowserProfileResponseView
          action="throttle"
          displayData={undefined}
          fallbackText="CPU throttling set to 4x slowdown."
          isError={false}
        />,
      );
      expect(screen.getByText(/profile · throttle/)).toBeInTheDocument();
      expect(screen.getByText(/4x slowdown/)).toBeInTheDocument();
    });

    it('renders error state with red chip', () => {
      const { container } = render(
        <BrowserProfileResponseView
          action="cpu_stop"
          displayData={undefined}
          fallbackText="CPU profiling is not active — call cpu_start first"
          isError={true}
        />,
      );
      expect(container.querySelector('.profile-status-error')).toBeInTheDocument();
      expect(screen.getByText(/not active/)).toBeInTheDocument();
    });
  });
});
