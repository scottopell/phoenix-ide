import { useState, useEffect, useCallback } from 'react';

interface Config {
  enabled: boolean; mcp_server: string; owner: string; workspace: string;
  timezone: string; phoenix_url: string; notification_command: string[] | null;
}
interface GatewayStatus {
  config: Config | null;
  scan?: { status: string; last_scan_at_unix_micros: number | null; last_error: string | null; baseline_complete: boolean; calls_last_pass: number; pages_last_pass: number; retry_at_us: number | null } | null;
  requests: { request_id: string; product_id: string | null; admitted: boolean; accepted_at_us: number; eyes_confirmed: boolean; failure: string | null; source: { source_url: string; kind: string; observed_at_unix_micros: number; message_ts: string } }[];
  returns: { return_id: string; state: string; destination: string; request_ids: string[]; failure: string | null; notification_receipt: string | null }[];
}
const initial = (): Config => ({ enabled: false, mcp_server: '', owner: '', workspace: '', timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, phoenix_url: window.location.origin, notification_command: null });

/** Owner-only setup. Saving enabled configuration activates real Slack operations. */
export function SlackGatewaySettings({ standalone = false }: { standalone?: boolean }) {
  const [expanded, setExpanded] = useState(standalone);
  const [config, setConfig] = useState<Config>(initial);
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [readCheck, setReadCheck] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    setBusy(true); setError(null);
    try { const r = await fetch('/api/slack-gateway'); if (!r.ok) throw Error('Could not read Slack gateway status'); const s: GatewayStatus = await r.json(); setStatus(s); setConfig(s.config ?? initial()); }
    catch (e) { setError(e instanceof Error ? e.message : 'Gateway unavailable'); }
    finally { setBusy(false); }
  }, []);
  useEffect(() => { if (standalone) void refresh(); }, [standalone, refresh]);
  async function save() {
    setBusy(true); setError(null);
    try { const r = await fetch('/api/slack-gateway', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(config) }); const s = await r.json(); if (!r.ok) throw Error(s.error ?? 'Could not save gateway'); setStatus(s); }
    catch (e) { setError(e instanceof Error ? e.message : 'Gateway unavailable'); }
    finally { setBusy(false); }
  }
  async function checkReadAccess() {
    setBusy(true); setError(null); setReadCheck(null);
    try {
      const r = await fetch('/api/slack-gateway/inspect', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(config) });
      const s = await r.json();
      if (!r.ok) throw Error(s.error ?? 'Slack read check failed');
      setReadCheck(`Owner verified; both discovery queries passed (${s.calls} MCP calls). No requests accepted or Slack changes made.`);
    } catch (e) { setError(e instanceof Error ? e.message : 'Slack unavailable'); }
    finally { setBusy(false); }
  }
  const fields = [ ['mcp_server', 'Existing MCP server name'], ['owner', 'Your Slack user ID'], ['workspace', 'Exact Slack permalink host'], ['timezone', 'IANA timezone'], ['phoenix_url', 'Phoenix URL'] ] as const;
  return <section className="settings-section">
    <button type="button" onClick={() => { if (!expanded) void refresh(); setExpanded(!expanded); }}>Slack gateway {expanded ? '▾' : '▸'}</button>
    {expanded && <>
      <p>🔥 on someone else’s message prepares a private briefing. A 🔥 prefix or your reaction on your own message requests a threaded reply. 👀 means durably accepted.</p>
      <p>Triggers begin after initialization. Discovery covers today and the previous three calendar dates.</p>
      <label><input type="checkbox" checked={config.enabled} onChange={e => setConfig({ ...config, enabled: e.target.checked })} /> Enable discovery of new requests</label>
      {fields.map(([key, label]) => <label key={key} style={{ display: 'block', marginTop: 8 }}>{label}<input aria-label={label} value={config[key]} onChange={e => setConfig({ ...config, [key]: e.target.value })} style={{ display: 'block', width: '100%' }} /></label>)}
      <label style={{ display: 'block', marginTop: 8 }}>Private notification executable (optional)<input aria-label="Private notification executable" value={config.notification_command?.[0] ?? ''} onChange={e => setConfig({ ...config, notification_command: e.target.value ? [e.target.value, ...(config.notification_command?.slice(1) ?? [])] : null })} style={{ display: 'block', width: '100%' }} /></label>
      <p>Saving enabled settings activates real Slack operations using your credentials. Accepted work continues when discovery is paused. No new agent permission mode is created.</p>
      <button disabled={busy} onClick={() => void save()}>Save</button> <button disabled={busy} onClick={() => void refresh()}>Refresh status</button>
      {' '}<button disabled={busy} onClick={() => void checkReadAccess()}>Check Slack read access</button>
      {readCheck && <p role="status">{readCheck}</p>}
      {error && <p role="alert">{error}</p>}
      {status && <>
        <p>Status: {status.config?.enabled ? status.scan?.status ?? 'Initializing' : 'Discovery paused'}</p>
        {status.scan?.last_error && <p role="alert">{status.scan.last_error}</p>}
        {status.scan?.last_scan_at_unix_micros && <p>Last complete scan: {new Date(status.scan.last_scan_at_unix_micros / 1000).toLocaleString()}</p>}
        <p>Last pass: {status.scan?.calls_last_pass ?? 0} MCP tool calls, {status.scan?.pages_last_pass ?? 0} search pages. {status.scan?.retry_at_us && <>Retry after {new Date(status.scan.retry_at_us / 1000).toLocaleString()}.</>}</p>
        <p>Reaction-add latency is unavailable from Slack search. A missing notification command leaves private notices pending.</p>
        <ul>{status.requests.map(r => <li key={r.request_id}><a href={r.source.source_url} target="_blank" rel="noreferrer">Slack {r.source.kind}</a> · {r.failure ?? (r.admitted ? 'Admitted' : 'Accepted')} · 👀 {r.eyes_confirmed ? 'confirmed' : 'pending'} · observed-to-accept {Math.max(0, (r.accepted_at_us - r.source.observed_at_unix_micros) / 1000).toFixed(0)} ms{r.product_id && <> · <a href={`/c/${r.product_id}`}>Open Phoenix</a></>}</li>)}</ul>
        <ul>{status.returns.map(r => <li key={r.return_id} id={'slack-return-' + r.return_id}>{r.destination} reply: {r.state} · covers {r.request_ids.length} request(s){r.failure && <> · {r.failure}</>}{r.notification_receipt && <> · Receipt: {r.notification_receipt}</>}</li>)}</ul>
      </>}
    </>}
  </section>;
}
