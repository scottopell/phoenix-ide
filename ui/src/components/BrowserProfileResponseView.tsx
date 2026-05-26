/**
 * BrowserProfileResponseView
 *
 * Renders the structured `display_data` from the `browser_profile` tool
 * (REQ-BT-019). One renderer per action — scenarios get a sparkline grid
 * and per-run table, heap_snapshot a delta card, metrics an aligned
 * table, cpu_stop / cpu_summary hot-function tables, trace_stop an
 * event-count + long-task list. Actions with no structured payload fall
 * through to a status-line wrapper around the existing text output.
 *
 * Discriminator is the input's `action` field, NOT a key in
 * `display_data` — the Rust tool dispatches on action and produces
 * action-specific payloads.
 */

import { useState, useMemo } from 'react';

/** Actions that have a dedicated structured renderer. Other browser_profile
 *  actions (e.g. why_render, throttle, gc_heap) should fall through to the
 *  parent's generic short-vs-long text rendering so long outputs keep their
 *  collapse / copy controls. */
export const STRUCTURED_PROFILE_ACTIONS = new Set([
  'run_scenario',
  'heap_snapshot',
  'metrics',
  'cpu_stop',
  'cpu_summary',
  'trace_stop',
]);

interface Props {
  action: string;
  displayData: Record<string, unknown> | undefined;
  fallbackText: string;
  isError: boolean;
}

export function BrowserProfileResponseView({ action, displayData, fallbackText, isError }: Props) {
  if (isError) {
    // Blocked scenarios still carry a structured payload. Everything else
    // is a plain error message — show the text as-is with an action chip.
    if (action === 'run_scenario' && displayData) {
      return <ScenarioView data={displayData} isError={true} fallbackText={fallbackText} />;
    }
    return <StatusLine action={action} text={fallbackText} variant="error" />;
  }

  switch (action) {
    case 'run_scenario':
      return <ScenarioView data={displayData} isError={false} fallbackText={fallbackText} />;
    case 'heap_snapshot':
      return <HeapDiffView data={displayData} fallbackText={fallbackText} />;
    case 'metrics':
      return <MetricsView data={displayData} fallbackText={fallbackText} />;
    case 'cpu_stop':
    case 'cpu_summary':
      return <CpuSummaryView action={action} data={displayData} fallbackText={fallbackText} />;
    case 'trace_stop':
      return <TraceSummaryView data={displayData} fallbackText={fallbackText} />;
    default:
      return <StatusLine action={action} text={fallbackText} variant="success" />;
  }
}

// ============================================================================
// run_scenario
// ============================================================================

interface RunSample {
  run_index: number;
  script_ms: number;
  long_tasks: number;
  wall_ms: number | null;
  dom_nodes: number;
  gc_ran: boolean;
  js_heap_used: number | null;
  react_status: 'measured' | 'present_not_measured' | 'no_profiling_build' | 'absent';
  react_commits: number | null;
  react_actual_ms: number | null;
}

function ScenarioView({
  data,
  isError,
  fallbackText,
}: {
  data: Record<string, unknown> | undefined;
  isError: boolean;
  fallbackText: string;
}) {
  const [showRawJson, setShowRawJson] = useState(false);
  const [tableExpanded, setTableExpanded] = useState(false);

  if (!data) {
    return <StatusLine action="run_scenario" text={fallbackText} variant={isError ? 'error' : 'success'} />;
  }

  const rawOutcome = typeof data['outcome'] === 'string' ? data['outcome'] : 'unknown';
  // `outcome` flows into a className; map to a known set so an unexpected
  // backend string can't inject spaces / punctuation into the class list.
  const outcome: 'completed' | 'blocked' | 'unknown' =
    rawOutcome === 'completed' || rawOutcome === 'blocked' ? rawOutcome : 'unknown';
  const requestedRuns = typeof data['requested_runs'] === 'number' ? data['requested_runs'] : null;
  const warmup = typeof data['warmup'] === 'number' ? data['warmup'] : null;
  const blockedStep = typeof data['blocked_step'] === 'string' ? data['blocked_step'] : null;
  const warnings = Array.isArray(data['methodology_warnings'])
    ? (data['methodology_warnings'] as unknown[]).filter((w): w is string => typeof w === 'string')
    : [];
  const rawSamples = Array.isArray(data['raw_samples'])
    ? (data['raw_samples'] as unknown[]).filter(
        (s): s is RunSample => !!s && typeof s === 'object',
      )
    : [];
  const samplesPath = typeof data['samples_path'] === 'string' ? data['samples_path'] : null;

  return (
    <div className="profile-response profile-scenario">
      <div className="profile-response-header">
        <span className={`profile-action-chip profile-outcome-${outcome}`}>
          {outcome === 'completed' ? '✓ completed' : outcome === 'blocked' ? '✗ blocked' : rawOutcome}
        </span>
        {requestedRuns !== null && (
          <span className="profile-meta">
            {rawSamples.length}/{requestedRuns} runs
          </span>
        )}
        {warmup !== null && warmup > 0 && (
          <span className="profile-meta">+{warmup} warmup discarded</span>
        )}
        {warnings.length > 0 && (
          <span className="profile-warnings-count">
            {warnings.length} warning{warnings.length !== 1 ? 's' : ''}
          </span>
        )}
      </div>

      {blockedStep && (
        <div className="profile-blocked-reason">
          <strong>Blocked step:</strong> {blockedStep}
        </div>
      )}

      {rawSamples.length > 0 && <SparklineGrid samples={rawSamples} />}

      {rawSamples.length > 0 && (
        <div className="profile-per-run">
          <button
            type="button"
            className="profile-toggle"
            aria-expanded={tableExpanded}
            onClick={() => setTableExpanded((v) => !v)}
          >
            {tableExpanded ? '▾' : '▸'} Per-run table ({rawSamples.length} run
            {rawSamples.length !== 1 ? 's' : ''})
          </button>
          {tableExpanded && <PerRunTable samples={rawSamples} />}
        </div>
      )}

      {warnings.length > 0 && (
        <div className="profile-warnings">
          <div className="profile-warnings-label">Methodology warnings</div>
          <ul>
            {warnings.map((w, i) => (
              <li key={i}>{w}</li>
            ))}
          </ul>
        </div>
      )}

      {samplesPath && (
        <div className="profile-footnote">
          Raw samples written to <code>{samplesPath}</code> — exceeded inline output limit, use{' '}
          <code>cat</code> to read the full JSON.
        </div>
      )}

      <button
        type="button"
        className="profile-toggle profile-toggle-raw"
        aria-expanded={showRawJson}
        onClick={() => setShowRawJson((v) => !v)}
      >
        {showRawJson ? '▾' : '▸'} Raw payload
      </button>
      {showRawJson && (
        <pre className="profile-raw-json">{JSON.stringify(data, null, 2)}</pre>
      )}
    </div>
  );
}

interface MetricDef {
  key: keyof RunSample;
  label: string;
  unit: string;
  nullWhen: (s: RunSample) => boolean;
}

/** Type-safe numeric read from an untrusted sample. `displayData` is
 *  `Record<string, unknown>`; a missing key, `undefined`, or a non-numeric
 *  value all collapse to `null` so downstream `.toFixed()` never sees
 *  garbage. */
function numericValue(s: RunSample, m: MetricDef): number | null {
  if (m.nullWhen(s)) return null;
  const v: unknown = s[m.key];
  return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

const SCENARIO_METRICS: MetricDef[] = [
  { key: 'script_ms', label: 'script', unit: 'ms', nullWhen: () => false },
  { key: 'wall_ms', label: 'wall', unit: 'ms', nullWhen: (s) => s.wall_ms === null },
  { key: 'long_tasks', label: 'long tasks', unit: '', nullWhen: () => false },
  { key: 'dom_nodes', label: 'DOM nodes', unit: '', nullWhen: () => false },
  { key: 'js_heap_used', label: 'JS heap', unit: 'B', nullWhen: (s) => !s.gc_ran },
  {
    key: 'react_actual_ms',
    label: 'React actual',
    unit: 'ms',
    nullWhen: (s) => s.react_status !== 'measured',
  },
  {
    key: 'react_commits',
    label: 'React commits',
    unit: '',
    nullWhen: (s) => s.react_commits === null,
  },
];

function SparklineGrid({ samples }: { samples: RunSample[] }) {
  // Only show metric rows where at least one sample has a non-null value;
  // a metric that's null across the board (no React, no GC) clutters more
  // than it informs.
  const visible = SCENARIO_METRICS.filter((m) =>
    samples.some((s) => numericValue(s, m) !== null),
  );
  if (visible.length === 0) return null;
  return (
    <div className="profile-sparkline-grid">
      {visible.map((m) => (
        <MetricRow key={String(m.key)} metric={m} samples={samples} />
      ))}
    </div>
  );
}

function MetricRow({ metric, samples }: { metric: MetricDef; samples: RunSample[] }) {
  const values: (number | null)[] = samples.map((s) => numericValue(s, metric));
  const real = values.filter((v): v is number => v !== null);
  if (real.length === 0) return null;
  const min = Math.min(...real);
  const max = Math.max(...real);
  // `last` is the LITERAL final-run value (which may be null when that run
  // didn't measure this metric — e.g. React absent, GC disabled). Showing
  // an earlier non-null value as "last" would be a quiet lie.
  const last: number | null = values.length > 0 ? (values[values.length - 1] ?? null) : null;
  return (
    <div className="profile-metric-row">
      <span className="profile-metric-label">{metric.label}</span>
      <Sparkline values={values} />
      <span className="profile-metric-stats">
        min {fmtVal(min, metric.unit)} · max {fmtVal(max, metric.unit)} · last{' '}
        {last === null ? '—' : fmtVal(last, metric.unit)}
      </span>
    </div>
  );
}

function Sparkline({ values }: { values: (number | null)[] }) {
  const w = 96;
  const h = 18;
  const real = values.filter((v): v is number => v !== null);
  if (real.length === 0) return <svg width={w} height={h} className="profile-sparkline" />;
  const min = Math.min(...real);
  const max = Math.max(...real);
  const range = max - min || 1;
  const stepX = values.length > 1 ? w / (values.length - 1) : 0;
  // Split into contiguous non-null segments so a null gap doesn't get
  // visually interpolated by a straight connecting line (the absence of
  // a measurement is information, not noise).
  const segments: Array<Array<{ x: number; y: number }>> = [];
  let current: Array<{ x: number; y: number }> = [];
  values.forEach((v, i) => {
    if (v === null) {
      if (current.length > 0) {
        segments.push(current);
        current = [];
      }
      return;
    }
    const x = i * stepX;
    const y = h - ((v - min) / range) * (h - 2) - 1;
    current.push({ x, y });
  });
  if (current.length > 0) segments.push(current);
  return (
    <svg width={w} height={h} className="profile-sparkline">
      {segments.map((seg, si) => (
        <polyline
          key={si}
          points={seg.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' ')}
          fill="none"
          stroke="currentColor"
          strokeWidth="1.2"
        />
      ))}
      {segments.flatMap((seg, si) =>
        seg.map((p, pi) => (
          <circle key={`${si}-${pi}`} cx={p.x} cy={p.y} r="1.5" fill="currentColor" />
        )),
      )}
    </svg>
  );
}

function PerRunTable({ samples }: { samples: RunSample[] }) {
  const visible = SCENARIO_METRICS.filter((m) =>
    samples.some((s) => numericValue(s, m) !== null),
  );
  return (
    <table className="profile-per-run-table">
      <thead>
        <tr>
          <th>run</th>
          {visible.map((m) => (
            <th key={String(m.key)}>
              {m.label}
              {m.unit && <span className="profile-th-unit"> ({m.unit})</span>}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {samples.map((s) => (
          <tr key={s.run_index}>
            <td>{s.run_index}</td>
            {visible.map((m) => {
              const v = numericValue(s, m);
              return (
                <td key={String(m.key)} className={v === null ? 'profile-cell-null' : ''}>
                  {v === null ? (
                    <span title={nullReason(m, s)}>—</span>
                  ) : (
                    fmtVal(v, m.unit)
                  )}
                </td>
              );
            })}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function nullReason(m: MetricDef, s: RunSample): string {
  if (m.key === 'js_heap_used' && !s.gc_ran) return 'GC disabled for this run';
  if (m.key === 'react_actual_ms' && s.react_status !== 'measured')
    return `React status: ${s.react_status}`;
  if (m.key === 'react_commits' && s.react_commits === null) return 'React absent on page';
  return 'not measured';
}

// ============================================================================
// heap_snapshot diff
// ============================================================================

function HeapDiffView({
  data,
  fallbackText,
}: {
  data: Record<string, unknown> | undefined;
  fallbackText: string;
}) {
  if (!data || data['baseline'] === undefined) {
    return <StatusLine action="heap_snapshot" text={fallbackText} variant="success" />;
  }
  const baseline = String(data['baseline'] ?? '');
  const post = String(data['post'] ?? '');
  const nodeDelta = typeof data['node_count_delta'] === 'number' ? data['node_count_delta'] : 0;
  const sizeDelta =
    typeof data['self_size_delta_bytes'] === 'number' ? data['self_size_delta_bytes'] : 0;
  const detached = (data['detached_dom_nodes'] as { baseline?: number; post?: number }) || {};
  const detachedBaseline = typeof detached.baseline === 'number' ? detached.baseline : 0;
  const detachedPost = typeof detached.post === 'number' ? detached.post : 0;
  const detachedDelta = detachedPost - detachedBaseline;
  const approximate = data['retained_size_approximate'] === true;

  return (
    <div className="profile-response profile-heap-diff">
      <div className="profile-response-header">
        <span className="profile-action-chip">heap diff</span>
        <span className="profile-meta" title={`baseline: ${baseline}\npost: ${post}`}>
          {shortPath(baseline)} → {shortPath(post)}
        </span>
      </div>
      <table className="profile-heap-table">
        <tbody>
          <tr>
            <td>nodes</td>
            <td className={`profile-delta ${deltaClass(nodeDelta)}`}>
              {fmtSigned(nodeDelta)}
            </td>
          </tr>
          <tr>
            <td>self size</td>
            <td className={`profile-delta ${deltaClass(sizeDelta)}`}>
              {fmtSignedBytes(sizeDelta)}
            </td>
          </tr>
          <tr>
            <td>detached DOM</td>
            <td className={`profile-delta ${deltaClass(detachedDelta, true)}`}>
              {detachedBaseline} → {detachedPost} ({fmtSigned(detachedDelta)})
            </td>
          </tr>
        </tbody>
      </table>
      {approximate && (
        <div className="profile-footnote">
          retained-size approximated by self_size delta; true retained needs dominator-tree walk.
        </div>
      )}
      <div className="profile-footnote">
        baseline: <code>{baseline}</code>
        <br />
        post: <code>{post}</code>
      </div>
    </div>
  );
}

// ============================================================================
// metrics snapshot
// ============================================================================

function MetricsView({
  data,
  fallbackText,
}: {
  data: Record<string, unknown> | undefined;
  fallbackText: string;
}) {
  const metrics =
    data && typeof data['metrics'] === 'object' && data['metrics'] !== null
      ? (data['metrics'] as Record<string, unknown>)
      : null;
  if (!metrics) {
    return <StatusLine action="metrics" text={fallbackText} variant="success" />;
  }
  const rows = Object.entries(metrics)
    .filter(([, v]) => typeof v === 'number')
    .map(([k, v]) => ({ name: k, value: v as number }));
  return (
    <div className="profile-response profile-metrics">
      <div className="profile-response-header">
        <span className="profile-action-chip">metrics</span>
        <span className="profile-meta">Performance.getMetrics</span>
      </div>
      <table className="profile-metrics-table">
        <tbody>
          {rows.map(({ name, value }) => (
            <tr key={name}>
              <td className="profile-metric-name">{name}</td>
              <td className="profile-metric-value">{fmtMetricValue(name, value)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ============================================================================
// cpu_stop / cpu_summary
// ============================================================================

interface CpuHotEntry {
  label: string;
  value: number;
  percent: number;
}

/** Drop entries that don't conform to the typed shape — `displayData` is
 *  `Record<string, unknown>` at runtime, so a schema-drift / hand-rolled
 *  payload could carry strings or undefined where numbers are expected.
 *  Filtering at the boundary keeps the renderer's body assumption-free. */
function sanitizeCpuEntries(entries: unknown): CpuHotEntry[] {
  if (!Array.isArray(entries)) return [];
  return entries.flatMap((e): CpuHotEntry[] => {
    if (!e || typeof e !== 'object') return [];
    const o = e as Record<string, unknown>;
    const label = typeof o['label'] === 'string' ? o['label'] : null;
    const value = typeof o['value'] === 'number' && Number.isFinite(o['value']) ? o['value'] : null;
    const percent =
      typeof o['percent'] === 'number' && Number.isFinite(o['percent']) ? o['percent'] : null;
    if (label === null || value === null || percent === null) return [];
    return [{ label, value, percent }];
  });
}

function CpuSummaryView({
  action,
  data,
  fallbackText,
}: {
  action: string;
  data: Record<string, unknown> | undefined;
  fallbackText: string;
}) {
  const summary = data?.['cpu_summary'] as
    | {
        path?: string;
        hitcount_fallback?: boolean;
        total?: number;
        top_by_self?: unknown;
        top_by_total?: unknown;
      }
    | undefined;
  if (!summary || !Array.isArray(summary.top_by_self)) {
    return <StatusLine action={action} text={fallbackText} variant="success" />;
  }
  const topBySelf = sanitizeCpuEntries(summary.top_by_self);
  const topByTotal = sanitizeCpuEntries(summary.top_by_total);
  const unit = summary.hitcount_fallback ? 'hits' : 'ms';
  const path = summary.path;
  return (
    <div className="profile-response profile-cpu">
      <div className="profile-response-header">
        <span className="profile-action-chip">CPU profile</span>
        {summary.hitcount_fallback ? (
          <span className="profile-meta profile-meta-warn">
            hitCount fallback — relative weight only
          </span>
        ) : (
          typeof summary.total === 'number' && (
            <span className="profile-meta">sampled {summary.total.toFixed(1)} ms</span>
          )
        )}
        {path && (
          <span className="profile-meta profile-path" title={path}>
            {shortPath(path)}
          </span>
        )}
      </div>
      <HotFunctionTable
        title="Top by SELF time"
        subtitle="aggregated per function — where CPU is actually spent"
        entries={topBySelf}
        unit={unit}
      />
      {topByTotal.length > 0 && (
        <HotFunctionTable
          title="Top call-tree nodes by TOTAL time"
          subtitle="self + descendants — may double-count recursion"
          entries={topByTotal}
          unit={unit}
        />
      )}
      {path && (
        <div className="profile-footnote">
          Saved to <code>{path}</code> — pass to <code>cpu_summary</code> or load in DevTools →
          Performance.
        </div>
      )}
    </div>
  );
}

function HotFunctionTable({
  title,
  subtitle,
  entries,
  unit,
}: {
  title: string;
  subtitle: string;
  entries: CpuHotEntry[];
  unit: string;
}) {
  return (
    <div className="profile-hot-block">
      <div className="profile-hot-title">{title}</div>
      <div className="profile-hot-subtitle">{subtitle}</div>
      <table className="profile-hot-table">
        <tbody>
          {entries.map((e, i) => (
            <tr key={i}>
              <td className="profile-hot-bar">
                <div
                  className="profile-hot-bar-fill"
                  style={{ width: `${clampPercent(e.percent)}%` }}
                />
              </td>
              <td className="profile-hot-value">
                {e.value.toFixed(1)}
                {unit}
              </td>
              <td className="profile-hot-percent">{e.percent.toFixed(1)}%</td>
              <td className="profile-hot-label">{e.label}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ============================================================================
// trace_stop
// ============================================================================

interface LongTask {
  name: string;
  ms: number;
}

function sanitizeLongTasks(entries: unknown): LongTask[] {
  if (!Array.isArray(entries)) return [];
  return entries.flatMap((e): LongTask[] => {
    if (!e || typeof e !== 'object') return [];
    const o = e as Record<string, unknown>;
    const name = typeof o['name'] === 'string' ? o['name'] : null;
    const ms = typeof o['ms'] === 'number' && Number.isFinite(o['ms']) ? o['ms'] : null;
    if (name === null || ms === null) return [];
    return [{ name, ms }];
  });
}

function TraceSummaryView({
  data,
  fallbackText,
}: {
  data: Record<string, unknown> | undefined;
  fallbackText: string;
}) {
  const trace = data?.['trace'] as
    | {
        path?: string;
        event_count?: number;
        long_task_count?: number;
        long_task_total_ms?: number;
        long_tasks?: unknown;
        timed_out?: boolean;
      }
    | undefined;
  if (!trace) {
    return <StatusLine action="trace_stop" text={fallbackText} variant="success" />;
  }
  const longTasks = sanitizeLongTasks(trace.long_tasks);
  return (
    <div className="profile-response profile-trace">
      <div className="profile-response-header">
        <span className="profile-action-chip">trace</span>
        {typeof trace.event_count === 'number' && (
          <span className="profile-meta">{trace.event_count.toLocaleString()} events</span>
        )}
        {typeof trace.long_task_count === 'number' && (
          <span className="profile-meta">
            {trace.long_task_count} long task{trace.long_task_count !== 1 ? 's' : ''}
            {typeof trace.long_task_total_ms === 'number' &&
              ` (${trace.long_task_total_ms.toFixed(1)} ms total)`}
          </span>
        )}
        {trace.timed_out && (
          <span className="profile-meta profile-meta-warn">timed out — partial trace</span>
        )}
        {trace.path && (
          <span className="profile-meta profile-path" title={trace.path}>
            {shortPath(trace.path)}
          </span>
        )}
      </div>
      {longTasks.length > 0 && <LongTaskTable tasks={longTasks} />}
      {longTasks.length === 0 && (
        <div className="profile-footnote">No long tasks (&gt;50ms) recorded.</div>
      )}
      {trace.path && (
        <div className="profile-footnote">
          Load <code>{trace.path}</code> into <code>chrome://tracing</code> or DevTools Performance
          for the full event timeline.
        </div>
      )}
    </div>
  );
}

function LongTaskTable({ tasks }: { tasks: LongTask[] }) {
  const [expanded, setExpanded] = useState(false);
  const visible = expanded ? tasks : tasks.slice(0, 5);
  const max = useMemo(() => tasks.reduce((m, t) => (t.ms > m ? t.ms : m), 0), [tasks]);
  return (
    <div className="profile-long-tasks">
      <table className="profile-hot-table">
        <tbody>
          {visible.map((t, i) => (
            <tr key={i}>
              <td className="profile-hot-bar">
                <div
                  className="profile-hot-bar-fill profile-hot-bar-fill-warn"
                  style={{ width: `${max > 0 ? Math.min(100, (t.ms / max) * 100) : 0}%` }}
                />
              </td>
              <td className="profile-hot-value">{t.ms.toFixed(1)}ms</td>
              <td className="profile-hot-label">{t.name}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {tasks.length > 5 && (
        <button
          type="button"
          className="profile-toggle"
          aria-expanded={expanded}
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? `▾ Show top 5` : `▸ Show all ${tasks.length} long tasks`}
        </button>
      )}
    </div>
  );
}

// ============================================================================
// Status line (fallback for actions without structured payloads)
// ============================================================================

function StatusLine({
  action,
  text,
  variant,
}: {
  action: string;
  text: string;
  variant: 'success' | 'error';
}) {
  return (
    <div className={`profile-response profile-status-line profile-status-${variant}`}>
      <span className="profile-action-chip">profile · {action || '?'}</span>
      {text && <pre className="profile-status-text">{text}</pre>}
    </div>
  );
}

// ============================================================================
// Formatting helpers
// ============================================================================

function fmtVal(v: number, unit: string): string {
  if (unit === 'B') return fmtBytes(v);
  if (unit === 'ms') return `${v.toFixed(1)} ms`;
  // counts — keep integers integers
  return Number.isInteger(v) ? v.toLocaleString() : v.toFixed(1);
}

function fmtBytes(b: number): string {
  if (Math.abs(b) >= 1024 * 1024) return `${(b / (1024 * 1024)).toFixed(1)} MB`;
  if (Math.abs(b) >= 1024) return `${(b / 1024).toFixed(1)} KB`;
  return `${b} B`;
}

function fmtSigned(n: number): string {
  if (n > 0) return `+${n.toLocaleString()}`;
  return n.toLocaleString();
}

function fmtSignedBytes(b: number): string {
  const formatted = fmtBytes(Math.abs(b));
  if (b > 0) return `+${formatted}`;
  if (b < 0) return `-${formatted}`;
  return formatted;
}

function deltaClass(n: number, detached = false): string {
  if (n === 0) return 'profile-delta-zero';
  if (detached && n > 0) return 'profile-delta-bad';
  if (n > 0) return 'profile-delta-up';
  return 'profile-delta-down';
}

function clampPercent(n: number): number {
  if (!Number.isFinite(n)) return 0;
  if (n < 0) return 0;
  if (n > 100) return 100;
  return n;
}

function shortPath(p: string): string {
  if (p.length <= 40) return p;
  return `…${p.slice(-37)}`;
}

function fmtMetricValue(name: string, v: number): string {
  if (/HeapUsedSize|HeapTotalSize|Size$/.test(name)) return fmtBytes(v);
  if (/Duration|Time$/.test(name)) return `${v.toFixed(3)} s`;
  return Number.isInteger(v) ? v.toLocaleString() : v.toFixed(3);
}
