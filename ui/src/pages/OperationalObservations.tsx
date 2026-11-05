import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../api';
import type { DeploymentObservations } from '../generated/DeploymentObservations';
import type { FailureBoundary } from '../generated/FailureBoundary';

const LABELS: Record<FailureBoundary, string> = {
  ordinary_tool_settlement: 'Required tool settlement',
  startup_required_authority: 'Required startup authority',
  settlement_owner_disappeared: 'Settlement owner disappeared',
  other_local_authority: 'Other local authority boundary',
  structured_log_write: 'Structured log write',
  structured_log_flush: 'Structured log flush',
  stdout_write: 'Stdout write',
  stdout_flush: 'Stdout flush',
  fatal_diagnostic_write: 'Fatal diagnostic write',
  trace_export: 'Trace export',
};

function age(sampledAt: string): string {
  const seconds = Math.max(0, Math.floor((Date.now() - Date.parse(sampledAt)) / 1000));
  return seconds < 60 ? `${seconds}s old` : `${Math.floor(seconds / 60)}m old`;
}

function availableBytes(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(2)} GiB available (${bytes.toLocaleString()} bytes)`;
}

function configured(boundary: FailureBoundary, sinks: DeploymentObservations['sinks']): boolean {
  switch (boundary) {
    case 'structured_log_write':
    case 'structured_log_flush': return sinks.structured_file_configured;
    case 'stdout_write':
    case 'stdout_flush': return sinks.stdout_configured;
    case 'fatal_diagnostic_write': return sinks.fatal_diagnostic_configured;
    case 'trace_export': return sinks.trace_failure_coverage === 'otlp_export_results';
    default: return true;
  }
}

export function OperationalObservations() {
  const [state, setState] = useState<{ sample: DeploymentObservations | null; loading: boolean; error: string | null }>({ sample: null, loading: true, error: null });
  const request = useRef(0);

  const load = useCallback(async () => {
    const generation = ++request.current;
    setState((current) => ({ ...current, loading: true }));
    try {
      const sample = await api.deploymentObservations();
      if (request.current === generation) setState({ sample, loading: false, error: null });
    } catch (cause) {
      if (request.current === generation) setState((current) => ({ ...current, loading: false, error: cause instanceof Error ? cause.message : String(cause) }));
    }
  }, []);

  useEffect(() => {
    void load();
    return () => { request.current += 1; };
  }, [load]);

  const { sample, loading, error } = state;
  return (
    <section className="settings-section" aria-label="Operational observations">
      <div className="settings-section__title-row">
        <h3 className="settings-section__title">Space &amp; diagnostic observations</h3>
        <button type="button" className="settings-inline-btn" disabled={loading} onClick={() => { void load(); }}>
          {loading ? 'Refreshing observations…' : 'Refresh observations'}
        </button>
      </div>
      <p className="settings-section__hint">Point-in-time server-local observations. Available space gives no write-success guarantee.</p>
      {error && <div className="settings-section__error">{sample ? 'Observations stale' : 'Observations unavailable'} — {error}</div>}
      {!sample && loading && <p className="settings-section__hint">Loading observations…</p>}
      {sample && (
        <>
          <p className="settings-section__hint">{error ? 'Stale' : loading ? 'Refreshing' : 'Observed'} at {new Date(sample.observed_at).toLocaleString()}</p>
          <p className="settings-section__hint">Low-space warning threshold: {(sample.low_space_warning_bytes / 1024 ** 3).toFixed(2)} GiB.</p>
          {sample.filesystems.map((filesystem) => (
            <div className="deploy-space-observation" key={filesystem.locations.map((location) => location.path).join('|')}>
              <strong>{availableBytes(filesystem.available_bytes)}</strong>
              <span className="settings-section__hint">Sample age at refresh: {age(filesystem.sampled_at)} · {new Date(filesystem.sampled_at).toLocaleString()}</span>
              {filesystem.low_space && <p className="settings-section__hint settings-section__hint--warning">Low space observed on this server filesystem.</p>}
              <ul className="deploy-space-locations">
                {filesystem.locations.map((location) => <li key={`${location.label}:${location.path}`}>
                  {location.label}: <code>{location.path}</code>
                  {location.sampled_path !== location.path && <span> — sampled existing ancestor <code>{location.sampled_path}</code></span>}
                </li>)}
              </ul>
            </div>
          ))}
          {sample.measurement_errors.map((failure) => <p className="settings-section__error" key={`${failure.label}:${failure.path}`}>
            {failure.label}: <code>{failure.path}</code> — space unavailable ({failure.error.replaceAll('_', ' ')})
            {failure.sampled_path && <>; sampled path <code>{failure.sampled_path}</code></>}
          </p>)}
          <h4>Failures observed in this process</h4>
          <p className="settings-section__hint">Counts reset when the process is replaced. No observed failure is not proof of health or successful persistence, logging, or export. Authority categories identify a settlement boundary, not the exact failed SQL operation.</p>
          {sample.local_authority_failure_boundary && <p className="settings-section__error">Local authority failure boundary: <code>{sample.local_authority_failure_boundary}</code></p>}
          <ul className="deploy-space-locations">
            {sample.failures.filter((failure) => configured(failure.boundary, sample.sinks)).map((failure) => <li key={failure.boundary}>
              {LABELS[failure.boundary]}: {failure.count === 0 ? 'no failure observed' : `${failure.count} observed`}
              {failure.last_observed_at && <> · latest {new Date(failure.last_observed_at).toLocaleString()}</>}
            </li>)}
          </ul>
          <p className="settings-section__hint">Structured log queue drops: {sample.sinks.structured_queue_drops ?? 'not configured'}. Queue drops are separate from sink write and flush errors.</p>
          {!sample.sinks.stdout_configured && <p className="settings-section__hint">Stdout sink not configured.</p>}
          {!sample.sinks.structured_file_configured && <p className="settings-section__hint">Structured file sink not configured.</p>}
          {!sample.sinks.fatal_diagnostic_configured && <p className="settings-section__hint">Fatal diagnostic sink not configured.</p>}
          {sample.sinks.trace_failure_coverage === 'not_configured' && <p className="settings-section__hint">Trace exporter not configured.</p>}
          {sample.sinks.trace_failure_coverage === 'datadog_unavailable' && <p className="settings-section__hint">Datadog exporter configured; failure observation unavailable in its SDK.</p>}
        </>
      )}
    </section>
  );
}
