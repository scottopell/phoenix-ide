import { useState, useEffect, useCallback, useId } from 'react';
import '../pages/SlackGatewayPage.css';

interface Config {
  enabled: boolean; mcp_server: string; owner: string; workspace: string;
  timezone: string; phoenix_url: string; notification_command: string[] | null;
}
interface GatewayStatus {
  config: Config | null;
  scan?: { status: string; last_scan_at_unix_micros: number | null; last_error: string | null; baseline_complete: boolean; calls_last_pass: number; pages_last_pass: number; retry_at_us: number | null } | null;
  requests: { request_id: string; product_id: string | null; conversation_available: boolean; admitted: boolean; accepted_at_us: number; eyes_confirmed: boolean; failure: string | null; source: { source_url: string; kind: string; trigger_kind: 'prefix' | 'reaction' | 'unknown_historical'; observed_at_unix_micros: number; message_ts: string } }[];
  returns: { return_id: string; state: string; destination: string; request_ids: string[]; failure: string | null; notification_receipt: string | null }[];
}
const initial = (): Config => ({ enabled: false, mcp_server: '', owner: '', workspace: '', timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, phoenix_url: window.location.origin, notification_command: null });

/** Owner-only setup. Saving enabled configuration activates real Slack operations. */
export function SlackGatewaySettings({ standalone = false }: { standalone?: boolean }) {
  const fieldId = useId();
  const [expanded, setExpanded] = useState(standalone);
  const [config, setConfig] = useState<Config>(initial);
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
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
    try { if (config.notification_command && !config.notification_command[0]?.trim()) throw Error('Enter a notification program, or remove the notification command.'); const r = await fetch('/api/slack-gateway', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(config) }); const s = await r.json(); if (!r.ok) throw Error(s.error ?? 'Could not save gateway'); setStatus(s); setSaved(true); }
    catch (e) { setError(e instanceof Error ? e.message : 'Gateway unavailable'); }
    finally { setBusy(false); }
  }
  async function checkReadAccess() {
    setBusy(true); setError(null); setReadCheck(null);
    try {
      const r = await fetch('/api/slack-gateway/inspect', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(config) });
      const s = await r.json();
      if (!r.ok) throw Error(s.error ?? 'Slack read check failed');
      const checks: { query: string; first_page_hits: number; verified_sources: number; more_pages: boolean }[] = s.checks ?? [];
      const demonstrated = checks.length === 2 && checks.every(c => c.verified_sources > 0) && s.reaction_read_verified === true;
      const samples = checks.map(c => `${c.query}: ${c.first_page_hits} sampled hit(s), ${c.verified_sources} verified source(s)${c.more_pages ? ', more pages available' : ''}`).join('; ');
      setReadCheck(`Owner verified. ${demonstrated ? 'Sampled source and reaction reads demonstrated' : 'Inconclusive: source and reaction access not fully demonstrated'}. ${samples} (${s.calls} MCP calls). No requests accepted or Slack changes made.`);
    } catch (e) { setError(e instanceof Error ? e.message : 'Slack unavailable'); }
    finally { setBusy(false); }
  }
  const fields = [ ['mcp_server', 'Slack MCP server', 'Connection name, such as slack'], ['owner', 'Your Slack user ID', 'The owner whose gestures Phoenix watches'], ['workspace', 'Slack workspace host', 'Permalink host only, without https://'], ['timezone', 'Timezone', 'Used for the four-calendar-date discovery window'], ['phoenix_url', 'Phoenix URL', 'Used in links back to this Phoenix instance'] ] as const;
  const discoveryState = !status?.config ? 'Not configured' : !status.config.enabled ? 'Paused' : !status.scan?.baseline_complete ? 'Initializing' : (status.scan.status ?? 'Initializing').replaceAll('_', ' ');
  const command = config.notification_command ?? [];
  const setCommandPart = (index: number, value: string) => {
    const next = [...command]; next[index] = value;
    setConfig({ ...config, notification_command: next }); setSaved(false);
  };
  return <div className="sg-settings">
    {!standalone && <button type="button" className="btn-secondary" aria-expanded={expanded} onClick={() => { if (!expanded) void refresh(); setExpanded(!expanded); }}>Slack gateway {expanded ? '▾' : '▸'}</button>}
    {expanded && <>
      <div className="sg-gesture-guide" aria-label="Slack gestures">
        <div><span aria-hidden="true">🔥</span><div><strong>Flag a discussion</strong><p>React to someone else’s message for a private briefing.</p></div></div>
        <div><span aria-hidden="true">↗</span><div><strong>Ask for a reply</strong><p>Start with 🔥, or react 🔥 to your own message.</p></div></div>
        <div><span aria-hidden="true">👀</span><div><strong>Know it’s accepted</strong><p>Eyes means Phoenix has durably accepted your request.</p></div></div>
      </div>
      <form onSubmit={e => { e.preventDefault(); void save(); }} onChange={() => setSaved(false)}>
        <fieldset className="sg-form-fields" disabled={busy}>
        <section className="sg-card" aria-labelledby="sg-discovery-title">
          <div className="sg-card-heading"><div><h2 id="sg-discovery-title">Discovery</h2><p>Triggers begin after initialization. Watches today and the previous three calendar dates.</p></div><span className={'sg-badge ' + (discoveryState === 'healthy' ? 'sg-badge--success' : '')}>{discoveryState}</span></div>
          <label className="sg-discovery-toggle"><input type="checkbox" role="switch" checked={config.enabled} onChange={e => setConfig({ ...config, enabled: e.target.checked })} /><span><strong>Discover new requests</strong><span>Pausing discovery lets accepted work and replies finish.</span></span></label>
          <div className="sg-fields">{fields.map(([key, label, hint]) => <div key={key} className={key === 'phoenix_url' ? 'sg-field sg-field--wide' : 'sg-field'}><label htmlFor={fieldId + key}>{label}</label><input id={fieldId + key} aria-describedby={fieldId + key + '-hint'} value={config[key]} onChange={e => setConfig({ ...config, [key]: e.target.value })} spellCheck={false} autoCapitalize="none" autoCorrect="off" /><small id={fieldId + key + '-hint'}>{hint}</small></div>)}</div>
        </section>
        <section className="sg-card" aria-labelledby="sg-notifications-title">
          <div className="sg-card-heading"><div><h2 id="sg-notifications-title">Private notifications</h2><p>Get a private alert with a Phoenix link when a briefing needs your attention.</p></div><span className={'sg-badge ' + (command[0] ? 'sg-badge--info' : '')}>{command[0] ? 'Configured' : 'Not configured'}</span></div>
          <p className="sg-hint">{command[0] ? 'Delivery uses your local notification command. Configuration alone does not verify delivery.' : 'Optional. New private notices wait in Phoenix until a delivery command is configured.'}</p>
          <details className="sg-command"><summary>Edit notification command</summary>
            <p className="sg-hint">The program starts the notifier; its arguments select the script or adapter. For example, uv is a launcher, not the notification provider.</p>
            <p className="sg-hint">Changes apply to new deliveries. Existing retries keep their original command and link.</p>
            <div className="sg-field"><label htmlFor={fieldId + '-program'}>Program or launcher</label><input id={fieldId + '-program'} aria-describedby={fieldId + '-program-hint'} value={command[0] ?? ''} onChange={e => setCommandPart(0, e.target.value)} placeholder="/absolute/path/to/notifier" spellCheck={false} /><small id={fieldId + '-program-hint'}>Absolute executable path. Keep credentials in the local notifier.</small></div>
            <div className="sg-arguments">{command.slice(1).map((argument, index) => <div className="sg-argument" key={index}><div className="sg-field"><label htmlFor={fieldId + '-arg-' + index}>Argument {index + 1}</label><textarea id={fieldId + '-arg-' + index} rows={1} value={argument} onChange={e => setCommandPart(index + 1, e.target.value)} spellCheck={false} /></div><button type="button" className="btn-secondary" aria-label={'Remove argument ' + (index + 1)} onClick={() => { setConfig({ ...config, notification_command: command.filter((_, i) => i !== index + 1) }); setSaved(false); }}>Remove</button></div>)}</div>
            <div className="sg-command-actions"><button type="button" className="btn-secondary" disabled={!command[0]} onClick={() => { setConfig({ ...config, notification_command: [...command, ''] }); setSaved(false); }}>Add argument</button>{command[0] && <button type="button" className="sg-text-button" onClick={() => { setConfig({ ...config, notification_command: null }); setSaved(false); }}>Remove notification command</button>}</div>
          </details>
        </section>
        <div className="sg-actions"><div><button type="submit" className="btn-primary" disabled={busy}>Save settings</button><button type="button" className="btn-secondary" disabled={busy} onClick={() => void checkReadAccess()}>Check Slack read access</button><button type="button" className="sg-text-button" disabled={busy} onClick={() => void refresh()}>Refresh status</button></div><p>Saving enabled discovery activates Slack operations using your existing permissions.</p></div>
        {saved && <p className="sg-feedback" role="status">Settings saved.</p>}
        </fieldset>
      </form>
      {readCheck && <p className="sg-feedback" role="status">{readCheck}</p>}
      {error && <p className="sg-feedback sg-feedback--error" role="alert">{error}</p>}
      {status && <section className="sg-card" aria-labelledby="sg-activity-title">
        <div className="sg-card-heading"><div><h2 id="sg-activity-title">Activity</h2><p>Accepted work and delivery evidence, even while discovery is paused.</p></div></div>
        {status.scan?.last_error && <p className="sg-feedback sg-feedback--error" role="alert">{status.scan.last_error}</p>}
        <dl className="sg-stats"><div><dt>Last complete scan</dt><dd>{status.scan?.last_scan_at_unix_micros ? new Date(status.scan.last_scan_at_unix_micros / 1000).toLocaleString() : 'No complete scan yet'}</dd></div><div><dt>Calls in last pass</dt><dd>{status.scan?.calls_last_pass ?? 0}<small> MCP tool calls</small></dd></div><div><dt>Pages in last pass</dt><dd>{status.scan?.pages_last_pass ?? 0}<small> search pages</small></dd></div></dl>
        {status.scan?.retry_at_us && <p className="sg-hint">Retry after {new Date(status.scan.retry_at_us / 1000).toLocaleString()}.</p>}
        <h3 className="sg-list-title">Requests <span>{status.requests.length}</span></h3>
        {status.requests.length === 0 ? <p className="sg-empty">No accepted requests yet. Once initialization finishes, use a fire gesture in Slack to get started.</p> : <ul className="sg-activity-list">{status.requests.map(r => <li key={r.request_id}><div className="sg-activity-heading"><a href={r.source.source_url} target="_blank" rel="noreferrer">Slack {r.source.kind}</a><span className="sg-badge">{r.failure ? 'Failed' : r.admitted ? 'Admitted' : 'Accepted'}</span>{r.conversation_available && r.product_id && <a className="sg-open-link" href={'/c/' + r.product_id}>Open Phoenix</a>}</div><p className="sg-hint">👀 {r.eyes_confirmed ? 'confirmed' : 'pending'} · observed-to-accept {Math.max(0, (r.accepted_at_us - r.source.observed_at_unix_micros) / 1000).toFixed(0)} ms{r.source.trigger_kind === 'prefix' && <> · prefix-message age at observation {Math.max(0, r.source.observed_at_unix_micros / 1000 - Number(r.source.message_ts) * 1000).toFixed(0)} ms</>}</p>{r.failure && <p className="sg-error-text">{r.failure}</p>}</li>)}</ul>}
        <h3 className="sg-list-title">Replies &amp; notifications <span>{status.returns.length}</span></h3>
        {status.returns.length === 0 ? <p className="sg-empty">No replies or private notices yet.</p> : <ul className="sg-activity-list">{status.returns.map(r => <li key={r.return_id} id={'slack-return-' + r.return_id}><div className="sg-activity-heading"><strong>{r.destination === 'thread' ? 'Thread reply' : 'Private notice'}</strong><span className={'sg-badge ' + (r.state === 'confirmed' ? 'sg-badge--success' : '')}>{r.state.replaceAll('_', ' ')}</span><span className="sg-hint">covers {r.request_ids.length} request(s)</span></div>{r.failure && <p className="sg-error-text">{r.failure}</p>}{r.notification_receipt && <details className="sg-receipt"><summary>Delivery receipt</summary><code>{r.notification_receipt}</code></details>}</li>)}</ul>}
        <p className="sg-hint sg-activity-footnote">Reaction-add latency is unavailable from Slack search. A missing notification command leaves new private notices pending.</p>
      </section>}
    </>}
  </div>;
}
