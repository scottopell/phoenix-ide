import { Fragment, useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import type { DeploymentInfo } from '../generated/DeploymentInfo';
import type { DeploymentDiskInfo } from '../generated/DeploymentDiskInfo';
import type { DiskSize } from '../generated/DiskSize';

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

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="deploy-row">
      <span className="deploy-label">{label}</span>
      <span className="deploy-value">{children}</span>
    </div>
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
    ]);
  }, [loadDisk]);

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

              <section className="settings-section">
                <h3 className="settings-section__title">Resources</h3>
                <Row label="Process memory (RSS)">
                  {resourceText(info.resources.process_memory_bytes, formatBytes)}
                </Row>
                <Row label="Process CPU">
                  {resourceText(info.resources.process_cpu_percent, (n) => `${n.toFixed(1)}%`)}
                </Row>
                <Row label="System memory available">
                  {resourceText(info.resources.system_available_memory_bytes, formatBytes)}
                </Row>
                <Row label="System memory total">
                  {resourceText(info.resources.system_total_memory_bytes, formatBytes)}
                </Row>
                <Row label="Logical CPUs">
                  {resourceText(info.resources.logical_cpu_count, (n) => String(n))}
                </Row>
              </section>

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
                                          <button type="button" className="deploy-reveal-btn" onClick={() => navigate(`/c/${disposition.conversation_id}`)}>
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
