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
import type { DeploymentInfo } from '../generated/DeploymentInfo';
import type { DeploymentDiskInfo } from '../generated/DeploymentDiskInfo';
import type { DiskSize } from '../generated/DiskSize';
import type { ManagedProcessRow } from '../generated/ManagedProcessRow';
import type { ManagedResourceCategory } from '../generated/ManagedResourceCategory';
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

const EMPTY_RESOURCES: ResourceState = {
  sample: null,
  history: [],
  loading: true,
  stale: false,
  error: null,
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
  nowMs = Date.now(),
): ResourceHistoryPoint[] {
  const cutoff = nowMs - RESOURCE_HISTORY_RETENTION_MS;
  const point: ResourceHistoryPoint = {
    sampledAt: snapshot.sampled_at,
    timeLabel: formatTimeLabel(snapshot.sampled_at),
    cpuPercent: snapshot.managed_total.cpu_percent,
    memoryBytes: snapshot.managed_total.memory_bytes,
  };
  const deduped = history.filter((entry) => entry.sampledAt !== point.sampledAt);
  const next = [...deduped, point].filter((entry) => {
    const parsed = Date.parse(entry.sampledAt);
    return !Number.isNaN(parsed) && parsed >= cutoff;
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
  if (!snapshot?.host.cpu_idle_percent && snapshot?.host.cpu_idle_percent !== 0) return 'Host busy state unavailable';
  const idle = snapshot.host.cpu_idle_percent;
  if (idle >= 70) return 'Host mostly idle';
  if (idle >= 35) return 'Host moderately busy';
  return 'Host busy';
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
        <h3 className="settings-section__title">Resources</h3>
        <button
          type="button"
          className="settings-inline-btn"
          onClick={refresh}
          disabled={state.loading}
        >
          {state.loading ? 'Refreshing resources…' : 'Refresh resources'}
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
                  <span>Idle</span>
                  <strong>{resourceText(sample.host.cpu_idle_percent, (value) => formatPercent(value))}</strong>
                </div>
                <div>
                  <span>User + system</span>
                  <strong>
                    {sample.host.cpu_user_percent !== null || sample.host.cpu_system_percent !== null
                      ? `${resourceText(sample.host.cpu_user_percent, (value) => formatPercent(value))} / ${resourceText(sample.host.cpu_system_percent, (value) => formatPercent(value))}`
                      : 'unavailable'}
                  </strong>
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
  const [cleanupPath, setCleanupPath] = useState<string | null>(null);
  const [resources, setResources] = useState<ResourceState>(EMPTY_RESOURCES);
  const resourcesInFlightRef = useRef(false);
  const resourcesTimerRef = useRef<number | null>(null);

  const handleReveal = useCallback((path: string) => {
    setRevealError(null);
    api.revealPath(path).catch((e) => {
      setRevealError(e instanceof Error ? e.message : String(e));
    });
  }, []);

  const loadDisk = useCallback(() => {
    setDiskLoading(true);
    setDiskError(null);
    return api
      .deploymentDiskInfo()
      .then((data) => setDiskInfo(data))
      .catch((e) => setDiskError(e instanceof Error ? e.message : String(e)))
      .finally(() => setDiskLoading(false));
  }, []);

  const fetchResources = useCallback(async () => {
    if (resourcesInFlightRef.current) return;
    resourcesInFlightRef.current = true;
    setResources((current) => ({ ...current, loading: true, error: current.sample ? current.error : null }));
    try {
      const sample = await api.deploymentResources();
      setResources((current) => ({
        sample,
        history: appendResourceHistory(current.history, sample),
        loading: false,
        stale: false,
        error: null,
      }));
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setResources((current) => ({
        ...current,
        loading: false,
        stale: current.sample !== null,
        error: message,
      }));
    } finally {
      resourcesInFlightRef.current = false;
    }
  }, []);

  useEffect(() => {
    let cancelled = false;

    const schedule = () => {
      if (resourcesTimerRef.current !== null) window.clearTimeout(resourcesTimerRef.current);
      resourcesTimerRef.current = window.setTimeout(async () => {
        if (!cancelled && document.visibilityState === 'visible') {
          await fetchResources();
        }
        if (!cancelled) schedule();
      }, RESOURCE_POLL_MS);
    };

    void fetchResources();
    schedule();

    const onVisibilityChange = () => {
      if (!cancelled && document.visibilityState === 'visible') {
        void fetchResources();
      }
    };

    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => {
      cancelled = true;
      if (resourcesTimerRef.current !== null) window.clearTimeout(resourcesTimerRef.current);
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, [fetchResources]);

  const handleCleanup = useCallback((path: string) => {
    setCleanupPath(path);
    setCleanupError(null);
    api.cleanupManagedWorktree(path)
      .then(() => loadDisk())
      .catch((e) => setCleanupError(e instanceof Error ? e.message : String(e)))
      .finally(() => setCleanupPath(null));
  }, [loadDisk]);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    return Promise.all([
      api
        .deploymentInfo()
        .then((data) => setInfo(data))
        .catch((e) => setError(e instanceof Error ? e.message : String(e)))
        .finally(() => setLoading(false)),
      loadDisk(),
      fetchResources(),
    ]);
  }, [fetchResources, loadDisk]);

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
                onClick={() => { void load(); }}
                disabled={loading}
              >
                {loading ? 'Refreshing…' : 'Refresh'}
              </button>
              <button type="button" className="settings-inline-btn" onClick={() => navigate(-1)}>
                Back
              </button>
            </div>
          </div>

          {error && <div className="settings-section__error">{error}</div>}
          {!info && loading && <div className="settings-section__hint">Loading…</div>}

          {info && (
            <>
              <section className="settings-section">
                <h3 className="settings-section__title">Build</h3>
                <Row label="Version"><code>{info.build.version}</code></Row>
                <Row label="Build"><code title="Git SHA">{info.build.git_sha}</code></Row>
                <Row label="Started">{formatDateTime(info.build.started_at)}</Row>
                <Row label="Uptime">{formatUptime(info.build.uptime_seconds)}</Row>
              </section>

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

              <ResourceMonitor state={resources} refresh={() => { void fetchResources(); }} />

              <section className="settings-section">
                <div className="settings-section__title-row">
                  <h3 className="settings-section__title">On disk</h3>
                  <button
                    type="button"
                    className="settings-inline-btn"
                    onClick={() => { void loadDisk(); }}
                    disabled={diskLoading}
                  >
                    {diskLoading ? 'Refreshing disk…' : 'Refresh disk'}
                  </button>
                </div>
                {diskError && <div className="settings-section__error">{diskError}</div>}
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
                                    {info.local_access && isRevealable(entry.path, entry.size) && (
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
                      {!info.local_access && (
                        <div className="settings-section__hint">
                          Reveal-in-file-manager is available only when viewing from the server host.
                        </div>
                      )}
                      <div className="settings-section__hint">Disk sampled at {formatDateTime(diskInfo.sampled_at)}</div>
                    </>
                  );
                })()}
              </section>

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
                {!info.log.stdout && !info.log.file && (
                  <div className="settings-section__hint">No log output configured.</div>
                )}
              </section>

              <div className="settings-section__hint">
                Sampled at {formatDateTime(info.sampled_at)}
              </div>
            </>
          )}
        </section>
      </main>
    </div>
  );
}
