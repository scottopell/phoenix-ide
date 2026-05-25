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
