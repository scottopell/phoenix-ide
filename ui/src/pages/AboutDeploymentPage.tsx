import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  Area,
  AreaChart,
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { api } from '../api';
import type { AboutResourcesSnapshot } from '../generated/AboutResourcesSnapshot';
import type { SqliteReportWindow } from '../generated/SqliteReportWindow';
import type { SqliteWorkloadReportResponse } from '../generated/SqliteWorkloadReportResponse';
import type { DeploymentInfo } from '../generated/DeploymentInfo';
import type { ReleaseUpdateSnapshot } from '../generated/ReleaseUpdateSnapshot';
import type { DeploymentDiskInfo } from '../generated/DeploymentDiskInfo';
import type { DiskSize } from '../generated/DiskSize';
import type { ManagedProcessRow } from '../generated/ManagedProcessRow';
import type { ManagedResourceCategory } from '../generated/ManagedResourceCategory';
import { ReleaseUpdatePanel } from './ReleaseUpdatePanel';
import './AboutDeploymentPage.css';

const RESOURCE_POLL_MS = 1_000;
const RESOURCE_HISTORY_RETENTION_MS = 5 * 60 * 1_000;
const AXIS = { stroke: 'var(--text-muted)', fontSize: 11 };
const GRID = 'var(--border-color)';

type ResourceHistoryPoint = {
  sampledAt: string;
  timeLabel: string;
  cpuPercent: number | null;
  memoryBytes: number | null;
};

type ResourceRollups = {
  currentCpuPercent: number | null;
  averageCpuPercent: number | null;
  peakCpuPercent: number | null;
  currentMemoryBytes: number | null;
  averageMemoryBytes: number | null;
  peakMemoryBytes: number | null;
};

type ResourceState = {
  sample: AboutResourcesSnapshot | null;
  history: ResourceHistoryPoint[];
  loading: boolean;
  stale: boolean;
  error: string | null;
};

type ActiveResourceRequest = {
  controller: AbortController;
  abort: () => void;
};

type SqliteState = {
  window: SqliteReportWindow;
  report: SqliteWorkloadReportResponse | null;
  loading: boolean;
  error: string | null;
  stale: boolean;
};

type SqliteWindowRequest = {
  generation: number;
  window: SqliteReportWindow;
};

const EMPTY_RESOURCES: ResourceState = {
  sample: null,
  history: [],
  loading: true,
  stale: false,
  error: null,
};

const EMPTY_SQLITE: SqliteState = {
  window: 'one_hour',
  report: null,
  loading: true,
  error: null,
  stale: false,
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

function formatPercent(value: number, fractionDigits = 1): string {
  return `${value.toFixed(fractionDigits)}%`;
}

function formatCpuTime(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);
  if (hours > 0) return `${hours}h ${minutes}m ${secs}s`;
  if (minutes > 0) return `${minutes}m ${secs}s`;
  return `${secs}s`;
}

function formatNumber(value: number): string {
  return value.toLocaleString();
}

function formatRatio(value: number, total: number): string {
  if (total <= 0) return '0%';
  return `${((value / total) * 100).toFixed(1)}%`;
}

function formatConfidenceDenominator(value: number, noun: string): string {
  return `${formatNumber(value)} ${noun}`;
}

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);
  const parts: string[] = [];
  if (days) parts.push(`${days}d`);
  if (hours || days) parts.push(`${hours}h`);
  if (minutes || hours || days) parts.push(`${minutes}m`);
  parts.push(`${secs}s`);
  return parts.join(' ');
}

function formatDateTime(iso: string | null): string {
  if (!iso) return 'unknown';
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

function formatTimeLabel(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? iso
    : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

function diskSizeLabel(size: DiskSize): string {
  switch (size.kind) {
    case 'measured':
      return formatBytes(size.bytes);
    case 'not_measured':
      return 'not measured';
    case 'absent':
      return 'absent';
    case 'inline_db':
      return 'stored in database';
  }
}

function installationOwnershipPresentation(
  ownership: DeploymentInfo['installation_ownership'],
): { label: string; detail: string; tone: 'managed' | 'neutral' | 'warning' } {
  switch (ownership.kind) {
    case 'launchd_managed':
      return { label: 'Managed by launchd', detail: 'Phoenix proved launchd owns this process.', tone: 'managed' };
    case 'systemd_managed':
      return { label: 'Managed by systemd', detail: 'Phoenix proved systemd owns this process.', tone: 'managed' };
    case 'bare_supervisor_managed':
      return { label: 'Managed by Phoenix supervisor', detail: 'Phoenix proved its bare Linux supervisor owns this process.', tone: 'managed' };
    case 'development':
      return { label: 'Development instance', detail: 'Production service management does not apply to this local development instance.', tone: 'neutral' };
    case 'unmanaged':
      return { label: 'Running without a proven manager', detail: ownership.reason, tone: 'neutral' };
    case 'ambiguous':
      return { label: 'Runtime manager is ambiguous', detail: ownership.reason, tone: 'warning' };
    case 'unsupported':
      return { label: 'Runtime manager unsupported', detail: `Phoenix cannot manage this process on ${ownership.platform}.`, tone: 'neutral' };
  }
}

function DeploymentSummary({ info }: { info: DeploymentInfo }) {
  const ownership = installationOwnershipPresentation(info.installation_ownership);
  const access = info.local_access
    ? {
        label: 'Viewing locally',
        detail: 'This browser is on the Phoenix host, so host-local actions can be available.',
        tone: 'local',
      } as const
    : {
        label: 'Viewing remotely',
        detail: 'This browser is not on the Phoenix host. You can inspect this deployment, but host-local actions are unavailable.',
        tone: 'remote',
      } as const;

  return (
    <section className="settings-section about-deployment-summary" aria-labelledby="deployment-summary-title">
      <div className="about-deployment-summary__identity">
        <div>
          <span className="about-deployment-summary__eyebrow">Running Phoenix</span>
          <h3 id="deployment-summary-title">Version {info.build.version}</h3>
        </div>
        <code aria-label={`Running git commit ${info.build.git_sha}`} title={info.build.git_sha}>{info.build.git_sha}</code>
      </div>
      <div className="about-deployment-summary__facts">
        <div className="about-deployment-summary__fact">
          <span className={`about-deployment-summary__badge about-deployment-summary__badge--${ownership.tone}`}>
            {ownership.label}
          </span>
          <p>{ownership.detail}</p>
        </div>
        <div className="about-deployment-summary__fact">
          <span className={`about-deployment-summary__badge about-deployment-summary__badge--${access.tone}`}>
            {access.label}
          </span>
          <p>{access.detail}</p>
        </div>
      </div>
      <div className="about-deployment-summary__runtime">
        <span>Listening at <code>{info.network.bind_address}</code></span>
        <span aria-hidden="true">·</span>
        <span>Started {formatDateTime(info.build.started_at)}</span>
        <span aria-hidden="true">·</span>
        <span>Up {formatUptime(info.build.uptime_seconds)}</span>
      </div>
    </section>
  );
}

function Freshness({
  state,
  sampledAt,
}: {
  state: 'loading' | 'current' | 'stale' | 'unavailable';
  sampledAt: string | undefined;
}) {
  const label = state === 'loading' ? 'Loading' : state === 'current' ? 'Current' : state === 'stale' ? 'Stale' : 'Unavailable';
  return (
    <span className={`about-freshness about-freshness--${state}`} title={sampledAt ? `Sampled ${formatDateTime(sampledAt)}` : undefined}>
      {label}{sampledAt ? ` · ${formatDateTime(sampledAt)}` : ''}
    </span>
  );
}

function resourceText(value: number | null, format: (n: number) => string): string {
  return value === null ? 'unavailable' : format(value);
}

function averageOf(values: Array<number | null>): number | null {
  const present = values.filter((value): value is number => value !== null);
  if (present.length === 0) return null;
  return present.reduce((sum, value) => sum + value, 0) / present.length;
}

function maxOf(values: Array<number | null>): number | null {
  const present = values.filter((value): value is number => value !== null);
  if (present.length === 0) return null;
  return Math.max(...present);
}

// Exported for deterministic history-window tests.
// eslint-disable-next-line react-refresh/only-export-components
export function appendResourceHistory(
  history: ResourceHistoryPoint[],
  snapshot: AboutResourcesSnapshot,
): ResourceHistoryPoint[] {
  const sampledAtMs = Date.parse(snapshot.sampled_at);
  const cutoff = sampledAtMs - RESOURCE_HISTORY_RETENTION_MS;
  const point: ResourceHistoryPoint = {
    sampledAt: snapshot.sampled_at,
    timeLabel: formatTimeLabel(snapshot.sampled_at),
    cpuPercent: snapshot.managed_total.cpu_percent,
    memoryBytes: snapshot.managed_total.memory_bytes,
  };
  const deduped = history.filter((entry) => entry.sampledAt !== point.sampledAt);
  const next = [...deduped, point].filter((entry) => {
    const parsed = Date.parse(entry.sampledAt);
    return !Number.isNaN(sampledAtMs) && !Number.isNaN(parsed) && parsed >= cutoff;
  });
  next.sort((a, b) => Date.parse(a.sampledAt) - Date.parse(b.sampledAt));
  return next;
}

// Exported for deterministic rollup tests.
// eslint-disable-next-line react-refresh/only-export-components
export function computeResourceRollups(history: ResourceHistoryPoint[]): ResourceRollups {
  const current = history.at(-1) ?? null;
  return {
    currentCpuPercent: current?.cpuPercent ?? null,
    averageCpuPercent: averageOf(history.map((entry) => entry.cpuPercent)),
    peakCpuPercent: maxOf(history.map((entry) => entry.cpuPercent)),
    currentMemoryBytes: current?.memoryBytes ?? null,
    averageMemoryBytes: averageOf(history.map((entry) => entry.memoryBytes)),
    peakMemoryBytes: maxOf(history.map((entry) => entry.memoryBytes)),
  };
}

function hostBusyLabel(snapshot: AboutResourcesSnapshot | null): string {
  const busy = snapshot?.host.cpu_busy_percent;
  const idle = snapshot?.host.cpu_idle_percent;
  if (busy === null || busy === undefined) {
    if (idle === null || idle === undefined) return 'Host busy state unavailable';
    if (idle >= 70) return 'Host mostly idle';
    if (idle >= 35) return 'Host moderately busy';
    return 'Host busy';
  }
  if (busy >= 65) return 'Host busy';
  if (busy >= 30) return 'Host moderately busy';
  return 'Host mostly idle';
}

function isAbortLikeError(error: unknown): boolean {
  return error instanceof DOMException
    ? error.name === 'AbortError'
    : error instanceof Error && error.name === 'AbortError';
}

function categoryUnavailableReason(category: ManagedResourceCategory): string {
  return category.reason ?? 'No reason reported';
}

function managedProcesses(categories: ManagedResourceCategory[]): ManagedProcessRow[] {
  return categories.flatMap((category) => category.processes.map((process) => ({ ...process, category: process.category ?? category.kind })));
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="deploy-row">
      <span className="deploy-label">{label}</span>
      <span className="deploy-value">{children}</span>
    </div>
  );
}

type MeasuredDiskEntry = DeploymentDiskInfo['disk'][number] & { size: { kind: 'measured'; bytes: number } };

type DiskSummary = {
  measuredCount: number;
  notMeasuredCount: number;
  absentCount: number;
  largestMeasured: MeasuredDiskEntry | null;
};

function diskSummary(entries: DeploymentDiskInfo['disk']): DiskSummary {
  let measuredCount = 0;
  let notMeasuredCount = 0;
  let absentCount = 0;
  let largestMeasured: MeasuredDiskEntry | null = null;

  for (const entry of entries) {
    switch (entry.size.kind) {
      case 'measured':
        measuredCount += 1;
        if (!largestMeasured || entry.size.bytes > largestMeasured.size.bytes) {
          largestMeasured = entry as MeasuredDiskEntry;
        }
        break;
      case 'not_measured':
        notMeasuredCount += 1;
        break;
      case 'absent':
        absentCount += 1;
        break;
      case 'inline_db':
        break;
    }
  }

  return { measuredCount, notMeasuredCount, absentCount, largestMeasured };
}

/** A path is revealable when it names a concrete location on disk: absolute,
 * present, and not a glob/aggregate pattern (e.g. the browser-profile or
 * PR-context rows, whose `path` is a `*` pattern spanning many directories). */
function isRevealable(path: string, size: DiskSize): boolean {
  if (size.kind === 'absent') return false;
  return path.startsWith('/') && !path.includes('*');
}

function ResourceTooltip({ active, payload, label }: { active?: boolean; payload?: Array<{ name?: string; value?: number; color?: string }>; label?: string }) {
  if (!active || !payload?.length) return null;
  return (
    <div className="about-resources-tip">
      <div className="about-resources-tip__title">{label}</div>
      {payload.map((entry) => (
        <div key={entry.name} className="about-resources-tip__row">
          <span className="about-resources-tip__swatch" style={{ background: entry.color }} />
          <span>{entry.name}</span>
          <span className="about-resources-tip__value">
            {entry.name?.includes('Memory') ? formatBytes(entry.value ?? 0) : formatPercent(entry.value ?? 0)}
          </span>
        </div>
      ))}
    </div>
  );
}

const SQLITE_WINDOWS: Array<{ value: SqliteReportWindow; label: string }> = [
  { value: 'one_hour', label: '1h' },
  { value: 'six_hours', label: '6h' },
  { value: 'twenty_four_hours', label: '24h' },
];

function SqliteDiagnostics({
  state,
  selectWindow,
  refresh,
}: {
  state: SqliteState;
  selectWindow: (window: SqliteReportWindow) => void;
  refresh: () => void;
}) {
  const report = state.report;
  const hasBaselineSamples = (report?.classification.baseline_statement_count ?? 0) > 0;
  const hasTypedSamples = (report?.classification.typed_outcome_count ?? 0) > 0;
  const hasNativeLoad = report?.reads.some((row) => row.total_profiled_read_execution_ms > 0 || row.peak_concurrency > 0) ?? false;
  const hasWriterOccupancy = report?.writer_categories.some((row) => row.writer_occupancy_percent > 0 || row.peak_concurrency > 0) ?? false;
  const hasReadFamilySamples = report?.read_families.some((row) => row.success_count + row.failure_count + row.abandoned_count > 0) ?? false;
  const hasCoverage = (report?.covered_uptime_micros ?? 0) > 0;
  return (
    <section className="settings-section about-sqlite-section" aria-labelledby="sqlite-diagnostics-title">
      <div className="settings-section__title-row">
        <div>
          <h3 id="sqlite-diagnostics-title" className="settings-section__title">SQLite workload</h3>
          <Freshness
            state={state.stale && report ? 'stale' : state.loading ? 'loading' : report ? 'current' : 'unavailable'}
            sampledAt={report?.sampled_at}
          />
          <div className="settings-section__hint">Read-only aggregate snapshot from in-memory collector buckets. Native baseline, typed outcomes, and occupancy are shown as separate source-qualified tables.</div>
        </div>
        <div className="about-sqlite-toolbar">
          <div className="about-sqlite-window-buttons" role="group" aria-label="SQLite report window">
            {SQLITE_WINDOWS.map((option) => (
              <button
                key={option.value}
                type="button"
                className={`about-sqlite-window-button${state.window === option.value ? ' about-sqlite-window-button--active' : ''}`}
                onClick={() => selectWindow(option.value)}
                disabled={state.loading && state.window === option.value}
              >
                {option.label}
              </button>
            ))}
          </div>
          <button type="button" className="settings-inline-btn" onClick={refresh} disabled={state.loading}>
            {state.loading ? 'Refreshing SQLite report…' : 'Refresh SQLite report'}
          </button>
        </div>
      </div>
      {state.error && <div className="settings-section__error">{report ? `SQLite report stale — ${state.error}` : `SQLite report unavailable — ${state.error}`}</div>}
      {state.loading && <div className="settings-section__hint">{report ? 'Refreshing SQLite report; displayed values are from the previous sample.' : 'Loading SQLite report…'}</div>}
      {report && !hasCoverage && (
        <div className="about-sqlite-empty">SQLite workload coverage is warming up; no covered interval is available yet.</div>
      )}
      {report && hasCoverage && (
        <>
          <dl className="about-sqlite-summary" aria-label="SQLite workload coverage">
            <div className="about-sqlite-summary__card">
              <dt>Coverage</dt>
              <dd>{report.coverage.label}</dd>
            </div>
            <div className="about-sqlite-summary__card">
              <dt>Process uptime</dt>
              <dd>{formatUptime(report.process_uptime_seconds)}</dd>
            </div>
            <div className="about-sqlite-summary__card">
              <dt>Covered uptime</dt>
              <dd>{formatUptime(report.covered_uptime_seconds)}</dd>
            </div>
            <div className="about-sqlite-summary__card">
              <dt>Confidence</dt>
              <dd>{report.coverage.fully_covered ? 'Full requested uptime available' : report.restart_truncated ? 'Restart truncated' : 'Alignment-shortened coverage'}</dd>
              <div className="settings-section__hint">
                {report.classification.typed_outcome_count} typed outcomes · {report.classification.typed_other_outcome_count} typed Other{report.classification.typed_other_outcome_share_percent != null ? ` (${formatPercent(report.classification.typed_other_outcome_share_percent, 1)})` : ''} · {report.classification.baseline_statement_count} native baseline statements · {report.classification.baseline_other_statement_count} baseline Other{report.classification.baseline_other_statement_share_percent != null ? ` (${formatPercent(report.classification.baseline_other_statement_share_percent, 1)})` : ''} · {report.classification.abandoned_count} abandoned · {report.classification.classification_gap_count} classification gaps · {report.classification.writer_occupancy_gap_count} writer occupancy gaps
              </div>
            </div>
          </dl>
          <div className="about-sqlite-tables">
            <section className="about-resources-chart-card">
              <div className="about-resources-chart-card__head">
                <h4>Native baseline statements by category</h4>
                <span>{report.baseline_categories.length} categories</span>
              </div>
              <table className="deploy-table about-sqlite-table">
                <thead>
                  <tr>
                    <th>Category</th>
                    <th>Native baseline statements</th>
                    <th>Native statement latency</th>
                    <th>Confidence denominator</th>
                  </tr>
                </thead>
                <tbody>
                  {!hasBaselineSamples && <tr><td colSpan={4}>No native baseline statements captured for this window yet.</td></tr>}
                  {report.baseline_categories.map((row) => (
                    <tr key={row.category}>
                      <td>{row.label}</td>
                      <td>{formatNumber(row.baseline_statement_count)}</td>
                      <td>avg {row.native_statement_latency.avg_ms ?? '—'} ms · p95≤ {row.native_statement_latency.p95_upper_bound_ms ?? '—'} ms</td>
                      <td>{formatConfidenceDenominator(row.native_statement_latency.sample_count, 'native statements')}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
            <section className="about-resources-chart-card">
              <div className="about-resources-chart-card__head">
                <h4>Typed write outcomes and occupancy by category</h4>
                <span>{report.writer_categories.length} categories</span>
              </div>
              <table className="deploy-table about-sqlite-table">
                <thead>
                  <tr>
                    <th>Category</th>
                    <th>Typed writes</th>
                    <th>Typed latency</th>
                    <th>Writer occupancy</th>
                    <th>Peak concurrency</th>
                    <th>Pool / admission wait</th>
                    <th>Retries</th>
                    <th>Failures</th>
                  </tr>
                </thead>
                <tbody>
                  {!hasTypedSamples && !hasWriterOccupancy && <tr><td colSpan={8}>No instrumented contention or writer occupancy samples captured for this window yet.</td></tr>}
                  {report.writer_categories.map((row) => (
                    <tr key={row.category}>
                      <td>{row.label}</td>
                      <td>{formatNumber(row.typed_operation_count)}</td>
                      <td>avg {row.typed_latency.avg_ms ?? '—'} ms · n={formatConfidenceDenominator(row.typed_latency.sample_count, 'typed writes')}</td>
                      <td>{formatPercent(row.writer_occupancy_percent, 2)} · n={formatConfidenceDenominator(report.bucket_count, 'covered buckets')}</td>
                      <td>{formatNumber(row.peak_concurrency)}</td>
                      <td>pool {row.pool_wait?.avg_ms ?? '—'} ms · admit {row.admission_wait?.avg_ms ?? '—'} ms</td>
                      <td>{row.retries ? formatNumber(row.retries.retry_count) : '—'}</td>
                      <td>busy {row.failures.busy} · locked {row.failures.locked} · timeout {row.failures.pool_timeout + row.failures.other_timeout} · fail {row.failures.other_failure} · abandoned {row.failures.abandoned}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
            <section className="about-resources-chart-card">
              <div className="about-resources-chart-card__head">
                <h4>Logical reads by source-defined family</h4>
                <span>{report.read_families.length} families</span>
              </div>
              <table className="deploy-table about-sqlite-table">
                <thead>
                  <tr>
                    <th>Read family</th>
                    <th>Attempts</th>
                    <th>Logical elapsed</th>
                    <th>Outcomes</th>
                  </tr>
                </thead>
                <tbody>
                  {!hasReadFamilySamples && <tr><td colSpan={4}>No source-defined logical reads captured for this window yet.</td></tr>}
                  {report.read_families.map((row) => (
                    <tr key={row.family}>
                      <td>{row.label}</td>
                      <td>{formatNumber(row.success_count + row.failure_count + row.abandoned_count)}</td>
                      <td>total {formatNumber(row.logical_elapsed.total_ms)} ms · avg {row.logical_elapsed.avg_ms ?? '—'} ms · p95≤ {row.logical_elapsed.p95_upper_bound_ms ?? '—'} ms</td>
                      <td>success {formatNumber(row.success_count)} · fail {formatNumber(row.failure_count)} · abandoned {formatNumber(row.abandoned_count)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <div className="settings-section__hint">Logical elapsed includes the complete public database method envelope, including pool wait, decoding, and attachment hydration. It is not native SQLite execution time.</div>
            </section>
            <section className="about-resources-chart-card">
              <div className="about-resources-chart-card__head">
                <h4>Native read load and instrumented contention by category</h4>
                <span>{report.reads.length} categories</span>
              </div>
              <table className="deploy-table about-sqlite-table">
                <thead>
                  <tr>
                    <th>Category</th>
                    <th>Typed reads</th>
                    <th>Typed latency</th>
                    <th>Native profiled read execution</th>
                    <th>Profiled statement latency</th>
                    <th>Peak concurrency</th>
                    <th>Retries / failures</th>
                  </tr>
                </thead>
                <tbody>
                  {!hasTypedSamples && !hasNativeLoad && <tr><td colSpan={7}>No native read load or instrumented read samples captured for this window yet.</td></tr>}
                  {report.reads.map((row) => (
                    <tr key={row.category}>
                      <td>{row.label}</td>
                      <td>{formatNumber(row.typed_operation_count)}</td>
                      <td>avg {row.typed_latency.avg_ms ?? '—'} ms · n={formatConfidenceDenominator(row.typed_latency.sample_count, 'typed reads')}</td>
                      <td>{row.total_profiled_read_execution_ms} ms</td>
                      <td>avg {row.profiled_statement_latency.avg_ms ?? '—'} ms · n={formatConfidenceDenominator(row.profiled_statement_latency.sample_count, 'profiled statements')}</td>
                      <td>{formatNumber(row.peak_concurrency)}</td>
                      <td>{row.retries ? `${row.retries.retry_count} retries` : 'retries —'} · pool {row.pool_wait?.avg_ms ?? '—'} ms · busy {row.failures.busy} · locked {row.failures.locked} · fail {row.failures.other_failure}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
          </div>
        </>
      )}
    </section>
  );
}

function ResourceMonitor({ state, refresh }: { state: ResourceState; refresh: () => void }) {
  const rollups = useMemo(() => computeResourceRollups(state.history), [state.history]);
  const sample = state.sample;
  const availableCategories = sample?.categories.filter((category) => category.attribution === 'available') ?? [];
  const unavailableCategories = sample?.categories.filter((category) => category.attribution === 'unavailable') ?? [];
  const processes = useMemo(() => managedProcesses(sample?.categories ?? []), [sample]);
  const hostMemoryUsed: number | null = sample?.host.used_memory_bytes ?? null;
  const hostMemoryTotal: number | null = sample?.host.total_memory_bytes ?? null;
  const hostMemoryAvailable: number | null = sample?.host.available_memory_bytes ?? null;

  return (
    <section className="settings-section about-resources-section">
      <div className="settings-section__title-row">
        <div>
          <h3 className="settings-section__title">Resources</h3>
          <Freshness
            state={state.stale && state.sample ? 'stale' : state.loading ? 'loading' : state.sample ? 'current' : 'unavailable'}
            sampledAt={state.sample?.sampled_at}
          />
          <div className="settings-section__hint">Refreshes automatically while this page is visible.</div>
        </div>
        <button
          type="button"
          className="settings-inline-btn"
          onClick={refresh}
        >
          {state.loading ? 'Refresh resources now' : 'Refresh resources'}
        </button>
      </div>
      {state.error && (
        <div className="settings-section__error">
          {state.sample ? `Live data stale — ${state.error}` : state.error}
        </div>
      )}
      {!sample && state.loading && <div className="settings-section__hint">Loading resource monitor…</div>}
      {!sample && !state.loading && !state.error && <div className="settings-section__hint">No resource sample available yet.</div>}
      {sample && (
        <>
          <div className="about-resources-grid" aria-label="Resource monitor summary">
            <section className="about-resources-card">
              <div className="about-resources-card__eyebrow">Host</div>
              <h4>{hostBusyLabel(sample)}</h4>
              <div className="about-resources-card__stat-row">
                <div>
                  <span>Busy</span>
                  <strong>{resourceText(sample.host.cpu_busy_percent, (value) => formatPercent(value))}</strong>
                </div>
                <div>
                  <span>Idle</span>
                  <strong>{resourceText(sample.host.cpu_idle_percent, (value) => formatPercent(value))}</strong>
                </div>
                <div>
                  <span>System</span>
                  <strong>{resourceText(sample.host.cpu_system_percent, (value) => formatPercent(value))}</strong>
                </div>
              </div>
              <div className="about-resources-card__stat-row">
                <div>
                  <span>Used memory</span>
                  <strong>{resourceText(hostMemoryUsed, formatBytes)}</strong>
                </div>
                <div>
                  <span>Available</span>
                  <strong>{resourceText(hostMemoryAvailable, formatBytes)}</strong>
                </div>
                <div>
                  <span>Total</span>
                  <strong>{resourceText(hostMemoryTotal, formatBytes)}</strong>
                </div>
              </div>
              <div className="settings-section__hint">
                {sample.host.logical_cpu_count === null
                  ? 'Logical CPU count unavailable.'
                  : `${sample.host.logical_cpu_count} logical CPUs`} · load avg {resourceText(sample.host.load_average_one, (value) => value.toFixed(2))} / {resourceText(sample.host.load_average_five, (value) => value.toFixed(2))} / {resourceText(sample.host.load_average_fifteen, (value) => value.toFixed(2))}
              </div>
            </section>

            <section className="about-resources-card">
              <div className="about-resources-card__eyebrow">Phoenix managed</div>
              <h4>{sample.managed_total.process_count} process{sample.managed_total.process_count === 1 ? '' : 'es'}</h4>
              <div className="about-resources-card__stat-row">
                <div>
                  <span>Deduplicated PIDs</span>
                  <strong>{formatNumber(sample.managed_total.deduplicated_pid_count)}</strong>
                </div>
                <div>
                  <span>Memory total</span>
                  <strong>{resourceText(sample.managed_total.memory_bytes, formatBytes)}</strong>
                </div>
                <div>
                  <span>CPU total</span>
                  <strong>{resourceText(sample.managed_total.cpu_percent, (value) => formatPercent(value))}</strong>
                </div>
              </div>
              {hostMemoryTotal !== null && sample.managed_total.memory_bytes !== null && (
                <div className="settings-section__hint">
                  Managed memory is {formatRatio(sample.managed_total.memory_bytes, hostMemoryTotal)} of host total.
                </div>
              )}
            </section>

            <section className="about-resources-card">
              <div className="about-resources-card__eyebrow">Rolling CPU</div>
              <h4>{resourceText(rollups.currentCpuPercent, (value) => formatPercent(value))}</h4>
              <div className="about-resources-card__stat-row">
                <div>
                  <span>Average</span>
                  <strong>{resourceText(rollups.averageCpuPercent, (value) => formatPercent(value))}</strong>
                </div>
                <div>
                  <span>Peak</span>
                  <strong>{resourceText(rollups.peakCpuPercent, (value) => formatPercent(value))}</strong>
                </div>
              </div>
              <div className="settings-section__hint">Bounded to the last 5 minutes of good samples.</div>
            </section>

            <section className="about-resources-card">
              <div className="about-resources-card__eyebrow">Rolling memory</div>
              <h4>{resourceText(rollups.currentMemoryBytes, formatBytes)}</h4>
              <div className="about-resources-card__stat-row">
                <div>
                  <span>Average</span>
                  <strong>{resourceText(rollups.averageMemoryBytes, formatBytes)}</strong>
                </div>
                <div>
                  <span>Peak</span>
                  <strong>{resourceText(rollups.peakMemoryBytes, formatBytes)}</strong>
                </div>
              </div>
              {state.stale && <div className="settings-section__hint settings-section__hint--warning">Showing the last good sample while refresh retries.</div>}
            </section>
          </div>

          <div className="about-resources-charts">
            <section className="about-resources-chart-card">
              <div className="about-resources-chart-card__head">
                <h4>Managed CPU over time</h4>
                <span>Recent samples</span>
              </div>
              <div className="about-resources-chart-card__body">
                <ResponsiveContainer width="100%" height={220}>
                  <AreaChart data={state.history}>
                    <CartesianGrid stroke={GRID} strokeDasharray="3 3" />
                    <XAxis dataKey="timeLabel" minTickGap={32} tick={AXIS} />
                    <YAxis tick={AXIS} tickFormatter={(value) => `${value}%`} />
                    <Tooltip content={<ResourceTooltip />} />
                    <Legend />
                    <Area type="monotone" dataKey="cpuPercent" name="Managed CPU" stroke="var(--accent-blue)" fill="color-mix(in srgb, var(--accent-blue) 28%, transparent)" connectNulls />
                  </AreaChart>
                </ResponsiveContainer>
              </div>
            </section>

            <section className="about-resources-chart-card">
              <div className="about-resources-chart-card__head">
                <h4>Managed memory over time</h4>
                <span>Recent samples</span>
              </div>
              <div className="about-resources-chart-card__body">
                <ResponsiveContainer width="100%" height={220}>
                  <LineChart data={state.history}>
                    <CartesianGrid stroke={GRID} strokeDasharray="3 3" />
                    <XAxis dataKey="timeLabel" minTickGap={32} tick={AXIS} />
                    <YAxis tick={AXIS} tickFormatter={(value) => formatBytes(Number(value))} width={80} />
                    <Tooltip content={<ResourceTooltip />} />
                    <Legend />
                    <Line type="monotone" dataKey="memoryBytes" name="Managed Memory" stroke="var(--accent-green)" strokeWidth={2} dot={false} connectNulls />
                  </LineChart>
                </ResponsiveContainer>
              </div>
            </section>
          </div>

          <div className="about-resources-lists">
            <section className="about-resources-chart-card">
              <div className="about-resources-chart-card__head">
                <h4>Categories</h4>
                <span>{sample.categories.length} tracked</span>
              </div>
              <table className="deploy-table about-resources-table">
                <thead>
                  <tr>
                    <th>Category</th>
                    <th>Status</th>
                    <th>Processes</th>
                    <th>CPU</th>
                    <th>Memory</th>
                    <th>Reason</th>
                  </tr>
                </thead>
                <tbody>
                  {availableCategories.map((category) => (
                    <tr key={category.kind}>
                      <td>{category.label}</td>
                      <td><span className="about-resources-badge">available</span></td>
                      <td>{formatNumber(category.totals.process_count)}</td>
                      <td>{resourceText(category.totals.cpu_percent, (value) => formatPercent(value))}</td>
                      <td>{resourceText(category.totals.memory_bytes, formatBytes)}</td>
                      <td>{category.reason ?? '—'}</td>
                    </tr>
                  ))}
                  {unavailableCategories.map((category) => (
                    <tr key={category.kind} className="about-resources-table__row--unavailable">
                      <td>{category.label}</td>
                      <td><span className="about-resources-badge about-resources-badge--muted">unavailable</span></td>
                      <td>0</td>
                      <td>unavailable</td>
                      <td>unavailable</td>
                      <td>{categoryUnavailableReason(category)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>

            <section className="about-resources-chart-card">
              <div className="about-resources-chart-card__head">
                <h4>Processes</h4>
                <span>{processes.length} rows</span>
              </div>
              <div className="about-resources-process-table-wrap">
                <table className="deploy-table about-resources-table about-resources-table--processes">
                  <thead>
                    <tr>
                      <th>Name</th>
                      <th>Kind</th>
                      <th>PID</th>
                      <th>CPU</th>
                      <th>Memory</th>
                      <th>Threads</th>
                      <th>CPU time</th>
                    </tr>
                  </thead>
                  <tbody>
                    {processes.length === 0 ? (
                      <tr>
                        <td colSpan={7} className="about-resources-table__empty">No managed processes reported.</td>
                      </tr>
                    ) : processes.map((process) => (
                      <tr key={`${process.pid}-${process.name}`}>
                        <td>{process.name}</td>
                        <td>{process.category}</td>
                        <td><code>{process.pid}</code></td>
                        <td>{resourceText(process.cpu_percent, (value) => formatPercent(value))}</td>
                        <td>{resourceText(process.memory_bytes, formatBytes)}</td>
                        <td>{resourceText(process.thread_count, formatNumber)}</td>
                        <td>{resourceText(process.cpu_time_seconds, formatCpuTime)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </section>
          </div>

          <div className="settings-section__hint">
            Resource sample captured {formatDateTime(sample.sampled_at)}{state.stale ? ' — stale' : ''}.
          </div>
        </>
      )}
    </section>
  );
}

export function AboutDeploymentPage() {
  const navigate = useNavigate();
  const [info, setInfo] = useState<DeploymentInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [revealError, setRevealError] = useState<string | null>(null);
  const [diskInfo, setDiskInfo] = useState<DeploymentDiskInfo | null>(null);
  const [diskError, setDiskError] = useState<string | null>(null);
  const [diskLoading, setDiskLoading] = useState(true);
  const [expandedWorktrees, setExpandedWorktrees] = useState(false);
  const [cleanupError, setCleanupError] = useState<string | null>(null);
  const [pendingDeploymentSignal, setPendingDeploymentSignal] = useState(0);
  const [cleanupPath, setCleanupPath] = useState<string | null>(null);
  const [resources, setResources] = useState<ResourceState>(EMPTY_RESOURCES);
  const [sqlite, setSqlite] = useState<SqliteState>(EMPTY_SQLITE);
  const resourcesInFlightRef = useRef(false);
  const resourcesTimerRef = useRef<number | null>(null);
  const resourcesMountedRef = useRef(false);
  const sqliteRequestRef = useRef<SqliteWindowRequest>({ generation: 0, window: 'one_hour' });
  const sqliteMountedRef = useRef(false);
  const resourcesGenerationRef = useRef(0);
  const identityRefreshRef = useRef<string | null>(null);
  const infoRef = useRef<DeploymentInfo | null>(null);
  const pendingDeploymentRefreshRef = useRef<Pick<ReleaseUpdateSnapshot, 'current_version' | 'current_git_sha' | 'installation_ownership'> | null>(null);
  infoRef.current = info;
  const activeResourceRequestRef = useRef<ActiveResourceRequest | null>(null);

  const invalidateActiveResourceRequest = useCallback(() => {
    resourcesGenerationRef.current += 1;
    const activeRequest = activeResourceRequestRef.current;
    if (!activeRequest) {
      resourcesInFlightRef.current = false;
      if (resourcesMountedRef.current) {
        setResources((current) => ({
          ...current,
          loading: false,
          stale: current.sample !== null,
        }));
      }
      return;
    }
    activeRequest.abort();
    if (activeResourceRequestRef.current === activeRequest) {
      activeResourceRequestRef.current = null;
    }
    resourcesInFlightRef.current = false;
    if (resourcesMountedRef.current) {
      setResources((current) => ({
        ...current,
        loading: false,
        stale: current.sample !== null,
      }));
    }
  }, []);

  const handleReveal = useCallback((path: string) => {
    setRevealError(null);
    api.revealPath(path).catch((e) => {
      setRevealError(e instanceof Error ? e.message : String(e));
    });
  }, []);

  const refreshDeployment = useCallback((snapshot: Pick<ReleaseUpdateSnapshot, 'current_version' | 'current_git_sha' | 'installation_ownership'>) => {
    const identity = `${snapshot.current_version}:${snapshot.current_git_sha}:${JSON.stringify(snapshot.installation_ownership)}`;
    const current = infoRef.current;
    if (!current) {
      pendingDeploymentRefreshRef.current = snapshot;
      setPendingDeploymentSignal((value) => value + 1);
      return;
    }
    if (
      (
        current.build.version === snapshot.current_version
        && current.build.git_sha === snapshot.current_git_sha
        && JSON.stringify(current.installation_ownership) === JSON.stringify(snapshot.installation_ownership)
      )
      || identityRefreshRef.current === identity
    ) return;
    identityRefreshRef.current = identity;
    api.deploymentInfo()
      .then((data) => setInfo(data))
      .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
      .finally(() => {
        if (identityRefreshRef.current === identity) identityRefreshRef.current = null;
      });
  }, []);

  useEffect(() => {
    if (!info || !pendingDeploymentRefreshRef.current) return;
    const pending = pendingDeploymentRefreshRef.current;
    pendingDeploymentRefreshRef.current = null;
    refreshDeployment(pending);
  }, [info, pendingDeploymentSignal, refreshDeployment]);

  const loadDisk = useCallback(() => {
    setDiskLoading(true);
    setDiskError(null);
    return api
      .deploymentDiskInfo()
      .then((data) => setDiskInfo(data))
      .catch((e) => setDiskError(e instanceof Error ? e.message : String(e)))
      .finally(() => setDiskLoading(false));
  }, []);

  const fetchResources = useCallback((supersede = false) => {
    if (document.visibilityState !== 'visible') return;
    if (resourcesInFlightRef.current) {
      if (!supersede) return;
      invalidateActiveResourceRequest();
    }
    resourcesInFlightRef.current = true;
    const generation = resourcesGenerationRef.current;
    const controller = new AbortController();
    const activeRequest: ActiveResourceRequest = {
      controller,
      abort: () => {
        controller.abort();
      },
    };
    activeResourceRequestRef.current = activeRequest;
    if (resourcesMountedRef.current) {
      setResources((current) => ({ ...current, loading: true, error: current.sample ? current.error : null }));
    }

    void api.deploymentResources({ signal: controller.signal })
      .then((sample) => {
        if (!resourcesMountedRef.current || generation !== resourcesGenerationRef.current) return;
        setResources((current) => ({
          sample,
          history: appendResourceHistory(current.history, sample),
          loading: false,
          stale: false,
          error: null,
        }));
      })
      .catch((e) => {
        if (isAbortLikeError(e) || !resourcesMountedRef.current || generation !== resourcesGenerationRef.current) return;
        const message = e instanceof Error ? e.message : String(e);
        setResources((current) => ({
          ...current,
          loading: false,
          stale: current.sample !== null,
          error: message,
        }));
      })
      .finally(() => {
        if (generation === resourcesGenerationRef.current) {
          resourcesInFlightRef.current = false;
        }
        if (activeResourceRequestRef.current === activeRequest) {
          activeResourceRequestRef.current = null;
        }
      });
  }, [invalidateActiveResourceRequest]);

  useEffect(() => {
    resourcesMountedRef.current = true;

    const schedule = () => {
      if (resourcesTimerRef.current !== null) window.clearTimeout(resourcesTimerRef.current);
      resourcesTimerRef.current = window.setTimeout(() => {
        if (document.visibilityState === 'visible') {
          fetchResources();
        }
        if (resourcesMountedRef.current) schedule();
      }, RESOURCE_POLL_MS);
    };

    if (document.visibilityState === 'visible') {
      fetchResources();
    } else {
      setResources((current) => ({
        ...current,
        loading: false,
        stale: current.sample !== null,
      }));
    }
    schedule();

    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') {
        fetchResources();
        return;
      }
      invalidateActiveResourceRequest();
    };

    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => {
      resourcesMountedRef.current = false;
      invalidateActiveResourceRequest();
      if (resourcesTimerRef.current !== null) window.clearTimeout(resourcesTimerRef.current);
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, [fetchResources, invalidateActiveResourceRequest]);

  const loadSqlite = useCallback((window: SqliteReportWindow) => {
    const generation = sqliteRequestRef.current.generation + 1;
    sqliteRequestRef.current = { generation, window };
    setSqlite((current) => ({
      ...current,
      window,
      report: current.window === window ? current.report : null,
      stale: current.window === window && current.stale,
      loading: true,
      error: null,
    }));
    void api.deploymentSqliteWorkload(window)
      .then((report) => {
        if (!sqliteMountedRef.current || sqliteRequestRef.current.generation !== generation || sqliteRequestRef.current.window !== window) return;
        setSqlite({ window, report, loading: false, error: null, stale: false });
      })
      .catch((cause) => {
        if (!sqliteMountedRef.current || sqliteRequestRef.current.generation !== generation || sqliteRequestRef.current.window !== window) return;
        const message = cause instanceof Error ? cause.message : String(cause);
        setSqlite((current) => ({
          ...current,
          window,
          loading: false,
          stale: current.report !== null,
          error: message,
        }));
      });
  }, []);

  useEffect(() => {
    sqliteMountedRef.current = true;
    loadSqlite('one_hour');
    return () => {
      sqliteMountedRef.current = false;
      sqliteRequestRef.current = {
        ...sqliteRequestRef.current,
        generation: sqliteRequestRef.current.generation + 1,
      };
    };
  }, [loadSqlite]);

  const handleCleanup = useCallback((path: string) => {
    setCleanupPath(path);
    setCleanupError(null);
    api.cleanupManagedWorktree(path)
      .then(() => loadDisk())
      .catch((e) => setCleanupError(e instanceof Error ? e.message : String(e)))
      .finally(() => setCleanupPath(null));
  }, [loadDisk]);

  const loadDeployment = useCallback(() => {
    setLoading(true);
    setError(null);
    return api
      .deploymentInfo()
      .then((data) => setInfo(data))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    let cancelled = false;
    api
      .deploymentInfo()
      .then((data) => { if (!cancelled) setInfo(data); })
      .catch((e) => { if (!cancelled) setError(e instanceof Error ? e.message : String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    api
      .deploymentDiskInfo()
      .then((data) => { if (!cancelled) setDiskInfo(data); })
      .catch((e) => { if (!cancelled) setDiskError(e instanceof Error ? e.message : String(e)); })
      .finally(() => { if (!cancelled) setDiskLoading(false); });
    return () => { cancelled = true; };
  }, []);

  return (
    <div id="app" className="list-page">
      <main id="main-area">
        <section className="view active">
          <div className="view-header">
            <h2>About this deployment</h2>
            <div className="view-header-actions">
              <button
                type="button"
                className="settings-inline-btn"
                onClick={() => { void loadDeployment(); }}
                disabled={loading}
              >
                {loading ? 'Refreshing deployment facts…' : 'Refresh deployment facts'}
              </button>
              <button type="button" className="settings-inline-btn" onClick={() => navigate('/')}>
                Conversations
              </button>
            </div>
          </div>

          {error && (
            <div className="settings-section__error">
              {info ? `Deployment facts are stale — ${error}` : `Deployment facts unavailable — ${error}`}
            </div>
          )}
          {!info && loading && <div className="settings-section__hint">Loading…</div>}

          <>
              {info && (
                <>
                  <DeploymentSummary info={info} />
                  <div className="about-page-freshness" aria-label="Diagnostics freshness">
                    <Freshness state={error ? 'stale' : loading ? 'loading' : 'current'} sampledAt={info.sampled_at} />
                    <span>Deployment facts</span>
                  </div>
                </>
              )}

              <ReleaseUpdatePanel onDeploymentChange={refreshDeployment} />

              {info && (
                <section className="settings-section">
                  <h3 className="settings-section__title">Network &amp; TLS</h3>
                <Row label="Bind address"><code>{info.network.bind_address}</code></Row>
                <Row label="Socket activated">{info.network.socket_activated ? 'yes' : 'no'}</Row>
                {info.network.tls.enabled ? (
                  <>
                    <Row label="TLS">enabled ({info.network.tls.mode})</Row>
                    {info.network.tls.cert_path && (
                      <Row label="Certificate"><code className="deploy-path">{info.network.tls.cert_path}</code></Row>
                    )}
                    {info.network.tls.key_path && (
                      <Row label="Key"><code className="deploy-path">{info.network.tls.key_path}</code></Row>
                    )}
                    {info.network.tls.ca_cert_path && (
                      <Row label="CA certificate"><code className="deploy-path">{info.network.tls.ca_cert_path}</code></Row>
                    )}
                    {info.network.tls.hosts.length > 0 && (
                      <Row label="Hosts">{info.network.tls.hosts.join(', ')}</Row>
                    )}
                  </>
                ) : (
                  <Row label="TLS">disabled — serving plain HTTP</Row>
                )}
                </section>
              )}

              <ResourceMonitor state={resources} refresh={() => { fetchResources(true); }} />

              <SqliteDiagnostics
                state={sqlite}
                selectWindow={loadSqlite}
                refresh={() => loadSqlite(sqlite.window)}
              />

              <section className="settings-section">
                <div className="settings-section__title-row">
                  <div>
                    <h3 className="settings-section__title">On disk</h3>
                    <Freshness
                      state={diskError && diskInfo ? 'stale' : diskLoading ? 'loading' : diskInfo ? 'current' : 'unavailable'}
                      sampledAt={diskInfo?.sampled_at}
                    />
                  </div>
                  <button
                    type="button"
                    className="settings-inline-btn"
                    onClick={() => { void loadDisk(); }}
                    disabled={diskLoading}
                  >
                    {diskLoading ? 'Refreshing disk…' : 'Refresh disk'}
                  </button>
                </div>
                {diskError && (
                  <div className="settings-section__error">
                    {diskInfo ? `Disk inventory is stale — ${diskError}` : `Disk inventory unavailable — ${diskError}`}
                  </div>
                )}
                {!diskInfo && diskLoading && <div className="settings-section__hint">Loading disk usage…</div>}
                {diskInfo && (() => {
                  const summary = diskSummary(diskInfo.disk);
                  return (
                    <>
                      <div className="deploy-disk-summary" aria-label="Disk usage health">
                        <div className="deploy-disk-summary__item">
                          <span>Largest measured</span>
                          <strong>{summary.largestMeasured ? formatBytes(summary.largestMeasured.size.bytes) : 'none'}</strong>
                        </div>
                        <div className="deploy-disk-summary__item">
                          <span>Measured rows</span>
                          <strong>{summary.measuredCount}</strong>
                        </div>
                        <div className={summary.notMeasuredCount > 0 ? 'deploy-disk-summary__item deploy-disk-summary__item--warn' : 'deploy-disk-summary__item'}>
                          <span>Not measured</span>
                          <strong>{summary.notMeasuredCount}</strong>
                        </div>
                        <div className="deploy-disk-summary__item">
                          <span>Absent</span>
                          <strong>{summary.absentCount}</strong>
                        </div>
                      </div>
                      {summary.largestMeasured && (
                        <div className="settings-section__hint">
                          {summary.largestMeasured.category === 'managed_worktrees'
                            ? 'Phoenix-managed worktrees are the largest measured disk category.'
                            : `Largest measured disk category: ${summary.largestMeasured.label} (${formatBytes(summary.largestMeasured.size.bytes)}).`}
                        </div>
                      )}
                      {summary.notMeasuredCount > 0 && (
                        <div className="settings-section__hint settings-section__hint--warning">
                          {summary.notMeasuredCount} disk {summary.notMeasuredCount === 1 ? 'row is' : 'rows are'} path-only; measured rows may also overlap, so this section highlights categories rather than summing them.
                        </div>
                      )}
                      <table className="deploy-table">
                        <tbody>
                          {diskInfo.disk.map((entry) => {
                            const isLargest = summary.largestMeasured?.category === entry.category && summary.largestMeasured?.label === entry.label;
                            const isManaged = entry.category === 'managed_worktrees';
                            return (
                              <Fragment key={entry.label}>
                                <tr
                                  className={isLargest ? 'deploy-table__row--largest' : undefined}
                                >
                                  <td className="deploy-table__label">{entry.label}</td>
                                  <td className="deploy-table__path"><code>{entry.path}</code></td>
                                  <td className="deploy-table__size">{diskSizeLabel(entry.size)}</td>
                                  <td className="deploy-table__action">
                                    {isManaged && diskInfo.managed_worktrees.length > 0 && (
                                      <button
                                        type="button"
                                        className="deploy-reveal-btn"
                                        onClick={() => setExpandedWorktrees((v) => !v)}
                                      >
                                        {expandedWorktrees ? 'Hide worktrees' : 'Show worktrees'}
                                      </button>
                                    )}
                                    {info?.local_access && isRevealable(entry.path, entry.size) && (
                                      <button
                                        type="button"
                                        className="deploy-reveal-btn"
                                        title="Open the containing folder in the file manager"
                                        onClick={() => handleReveal(entry.path)}
                                      >
                                        Reveal
                                      </button>
                                    )}
                                  </td>
                                </tr>
                                {isManaged && expandedWorktrees && diskInfo.managed_worktrees.map((wt) => {
                                  const disposition = wt.disposition;
                                  return (
                                    <tr key={wt.path} className="deploy-table__detail-row">
                                      <td className="deploy-table__label">↳ {diskSizeLabel(wt.size)}</td>
                                      <td className="deploy-table__path">
                                        <code>{wt.path}</code>
                                        {wt.repository && <div className="settings-section__hint">repo: {wt.repository}</div>}
                                        {wt.branch_name && <div className="settings-section__hint">branch: {wt.branch_name}</div>}
                                      </td>
                                      <td className="deploy-table__size">
                                        {disposition.kind === 'live'
                                          ? `Live: ${disposition.title ?? disposition.conversation_id} (${disposition.state})`
                                          : `Leftover: ${disposition.source_conversation_id} (${disposition.source_state})`}
                                      </td>
                                      <td className="deploy-table__action">
                                        {disposition.kind === 'live' ? (
                                          <button type="button" className="deploy-reveal-btn" onClick={() => navigate(`/c/${disposition.slug ?? disposition.conversation_id}`)}>
                                            Open conversation
                                          </button>
                                        ) : disposition.cleanup_allowed ? (
                                          <button
                                            type="button"
                                            className="deploy-reveal-btn"
                                            onClick={() => handleCleanup(wt.path)}
                                            disabled={cleanupPath === wt.path}
                                          >
                                            {cleanupPath === wt.path ? 'Cleaning…' : 'Clean up leftover'}
                                          </button>
                                        ) : null}
                                      </td>
                                    </tr>
                                  );
                                })}
                              </Fragment>
                            );
                          })}
                        </tbody>
                      </table>
                      {revealError && <div className="settings-section__error">{revealError}</div>}
                      {cleanupError && <div className="settings-section__error">{cleanupError}</div>}
                      {info && !info.local_access && (
                        <div className="settings-section__hint">
                          Reveal actions require viewing this page on the Phoenix host. Cleanup availability is shown separately per worktree.
                        </div>
                      )}
                    </>
                  );
                })()}
              </section>

              {info && (
                <section className="settings-section">
                  <h3 className="settings-section__title">Logs</h3>
                <Row label="stdout">
                  {info.log.stdout ? 'on (captured by the supervising process)' : 'off'}
                </Row>
                {info.log.file ? (
                  <Row label="Log file"><code className="deploy-path">{info.log.file}</code></Row>
                ) : (
                  <Row label="Log file">none</Row>
                )}
                {info.log.fatal_file ? (
                  <Row label="Fatal diagnostics"><code className="deploy-path">{info.log.fatal_file}</code></Row>
                ) : (
                  <Row label="Fatal diagnostics">none</Row>
                )}
                {!info.log.stdout && !info.log.file && !info.log.fatal_file && (
                  <div className="settings-section__hint">No log output configured.</div>
                )}
                </section>
              )}

              {info && (
                <div className="settings-section__hint">
                  Sampled at {formatDateTime(info.sampled_at)}
                </div>
              )}
            </>
        </section>
      </main>
    </div>
  );
}
