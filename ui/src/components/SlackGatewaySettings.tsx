import { useState, useEffect, useCallback, useId } from 'react';
import type { McpServerStatus } from '../api';
import { slackGatewayToolProblem, type SlackToolSchema } from './slackGatewayToolCompatibility';
import '../pages/SlackGatewayPage.css';

interface Config {
  enabled: boolean; mcp_server: string; owner: string; workspace: string;
  timezone: string; phoenix_url: string; trigger_emoji: string; notification_command: string[] | null;
}
interface GatewayStatus {
  config: Config | null;
  connection?: McpServerStatus | null;
  stale_scan_warning?: boolean;
  scan?: { status: string; last_scan_at_unix_micros: number | null; last_error: string | null; baseline_complete: boolean; calls_last_pass: number; pages_last_pass: number; retry_at_us: number | null } | null;
  requests: { request_id: string; product_id: string | null; conversation_available: boolean; admitted: boolean; accepted_at_us: number; eyes_confirmed: boolean; eyes_failure: string | null; failure: string | null; source: { source_url: string; kind: string; trigger_kind: 'prefix' | 'reaction'; observed_at_unix_micros: number; message_ts: string } }[];
  returns: { return_id: string; state: string; destination: string; request_ids: string[]; failure: string | null; notification_receipt: string | null }[];
}
const initial = (): Config => ({ enabled: false, mcp_server: '', owner: '', workspace: '', timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, phoenix_url: window.location.origin, trigger_emoji: 'fire', notification_command: null });
const emojiName = (name: string) => name.trim().replace(/^:([^:]+):$/, '$1').toLowerCase();

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
  const [toolProblem, setToolProblem] = useState<string | null>(null);
  const updateStatus = useCallback(async (s: GatewayStatus) => {
    let problem: string | null = null;
    if (s.config && s.connection?.state === 'ready' && s.connection.enabled) {
      try {
        const r = await fetch('/api/slack-gateway/capabilities');
        if (!r.ok) throw Error('Catalog unavailable');
        const catalog: { tools?: SlackToolSchema[]; catalogs?: { server: string; current: boolean }[] } = await r.json();
        const tools = catalog.tools ?? [];
        const server = s.config.mcp_server;
        problem = !catalog.catalogs?.some(c => c.server === server && c.current)
          ? 'Cannot verify required tool schemas: catalog unavailable or outdated.'
          : slackGatewayToolProblem(tools, server);
      } catch { problem = 'Cannot verify required tool schemas: catalog unavailable.'; }
    }
    setToolProblem(problem); setStatus(s);
  }, []);
  const refresh = useCallback(async () => {
    setBusy(true); setError(null);
    try { const r = await fetch('/api/slack-gateway'); if (!r.ok) throw Error('Could not read Slack gateway status'); const s: GatewayStatus = await r.json(); await updateStatus(s); setConfig(s.config ? { ...initial(), ...s.config } : initial()); }
    catch (e) { setError(e instanceof Error ? e.message : 'Gateway unavailable'); }
    finally { setBusy(false); }
  }, [updateStatus]);
  useEffect(() => { if (standalone) void refresh(); }, [standalone, refresh]);
  async function save() {
    setBusy(true); setError(null);
    try {
      const name = emojiName(config.trigger_emoji);
      if (!/^[a-z0-9_+-]{1,64}$/.test(name)) throw Error('Enter one Slack emoji name, such as phoenix-please-look.');
      if (name === 'eyes') throw Error('eyes is reserved for Phoenix acknowledgments.');
      const r = await fetch('/api/slack-gateway', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ...config, trigger_emoji: name }) });
      const s = await r.json(); if (!r.ok) throw Error(s.error ?? 'Could not save gateway'); await updateStatus(s); setConfig(s.config ? { ...initial(), ...s.config } : { ...config, trigger_emoji: name }); setSaved(true);
    }
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
  const connection = status?.connection;
  const connectionProblem = status?.config && status.connection === null ? 'Connection unavailable' : connection && !connection.enabled ? 'Connection disabled' : connection?.state === 'unauthorized' ? 'Sign-in required' : connection && connection.state !== 'ready' ? connection.state.replaceAll('_', ' ') : toolProblem;
  const discoveryState = !status?.config ? 'Not configured' : !status.config.enabled ? 'Paused' : connectionProblem ?? (!status.scan?.baseline_complete ? 'Initializing' : (status.scan.status ?? 'Initializing').replaceAll('_', ' '));
  const command = config.notification_command ?? [];
  const exampleEmoji = emojiName(config.trigger_emoji) || 'emoji-name';
  const setCommandPart = (index: number, value: string) => {
    const next = [...command]; next[index] = value;
    setConfig({ ...config, notification_command: next }); setSaved(false);
  };
  return <div className="sg-settings">
    {!standalone && <button type="button" className="btn-secondary" aria-expanded={expanded} onClick={() => { if (!expanded) void refresh(); setExpanded(!expanded); }}>Slack gateway {expanded ? '▾' : '▸'}</button>}
    {expanded && <>
      <form onSubmit={e => { e.preventDefault(); void save(); }} onChange={() => setSaved(false)}>
        <fieldset className="sg-form-fields" disabled={busy}>
        <section className="sg-card" aria-labelledby={fieldId + '-triggers-title'}>
          <div className="sg-card-heading"><div><h2 id={fieldId + '-triggers-title'}>Trigger emoji</h2><p>Choose one emoji for all three workflows. A dedicated custom name keeps social reactions separate.</p></div></div>
          <div className="sg-fields"><div className="sg-field"><label htmlFor={fieldId + '-emoji'}>Slack emoji name</label><input id={fieldId + '-emoji'} value={config.trigger_emoji} onChange={e => setConfig({ ...config, trigger_emoji: e.target.value })} placeholder="phoenix-please-look" maxLength={66} required spellCheck={false} autoCapitalize="none" autoCorrect="off" /></div></div>
          <p className="sg-hint">Enter a Slack name with or without colons. Upload custom emoji in Slack first. Choosing a custom emoji replaces 🔥 as the trigger. 👀 is reserved for acknowledgment.</p>
          <p className="sg-hint">Changing the emoji restarts initialization. Existing matches and gestures made during initialization become a baseline, without starting work. Wait until Discovery shows healthy before triggering a new request. Accepted work continues.</p>
          <h3 className="sg-list-title">Three workflows</h3>
          <p className="sg-hint">Your saved emoji triggers these rules. These examples preview your edits; save to apply them.</p>
          <div className="sg-gesture-guide" aria-label="Slack workflows">
            <div><span aria-hidden="true">↗</span><div><strong>1. Message prefix</strong><p>Start your own message with your configured emoji. Phoenix acts on your request and replies in that Slack thread.</p><code>:{exampleEmoji}: phoenix, investigate this</code></div></div>
            <div><span aria-hidden="true">↩</span><div><strong>2. React to your own message</strong><p>Add your configured emoji reaction to your own message. Phoenix treats that message as your request and replies in its Slack thread.</p></div></div>
            <div><span aria-hidden="true">▤</span><div><strong>3. React to someone else’s message</strong><p>Add your configured emoji reaction to another person’s message. Phoenix prepares a private briefing in Phoenix and attempts your configured private notification.</p></div></div>
          </div>
          <p className="sg-hint">👀 means Phoenix durably accepted the request. Acknowledgment is best effort. Plain follow-ups do not trigger work; reacting to an already accepted message does not run it again.</p>
        </section>
        <section className="sg-card" aria-labelledby="sg-discovery-title">
          <div className="sg-card-heading"><div><h2 id="sg-discovery-title">Discovery</h2><p>Triggers begin after initialization. Watches today and the previous three calendar dates.</p></div><span className={'sg-badge ' + (discoveryState === 'healthy' ? 'sg-badge--success' : '')}>{discoveryState}</span></div>
          <label className="sg-discovery-toggle"><input type="checkbox" role="switch" checked={config.enabled} onChange={e => setConfig({ ...config, enabled: e.target.checked })} /><span><strong>Discover new requests</strong><span>Pausing discovery lets accepted work and replies finish.</span></span></label>
          <div className="sg-fields">{fields.map(([key, label, hint]) => <div key={key} className={key === 'phoenix_url' ? 'sg-field sg-field--wide' : 'sg-field'}><label htmlFor={fieldId + key}>{label}</label><input id={fieldId + key} aria-describedby={fieldId + key + '-hint'} value={config[key]} onChange={e => setConfig({ ...config, [key]: e.target.value })} spellCheck={false} autoCapitalize="none" autoCorrect="off" /><small id={fieldId + key + '-hint'}>{hint}</small></div>)}</div>
        </section>
        <section className="sg-card" aria-labelledby="sg-notifications-title">
          <div className="sg-card-heading"><div><h2 id="sg-notifications-title">Private notifications</h2><p>Get a private alert with a Phoenix link when a briefing needs your attention.</p></div><span className={'sg-badge ' + (command[0] ? 'sg-badge--info' : '')}>{command[0] ? 'Configured' : 'Not configured'}</span></div>
          <p className="sg-hint">{command[0] ? 'Each notice is attempted once. Submission does not verify delivery; there are no automatic retries or delivery checks.' : 'Optional. Without a command, private alerts fail visibly; your work remains in Phoenix.'}</p>
          <details className="sg-command"><summary>Edit notification command</summary>
            <p className="sg-hint">The program starts the notifier; its arguments select the script or adapter. For example, uv is a launcher, not the notification provider.</p>
            <p className="sg-hint">Changes apply to notices that have not been attempted. Existing attempts are never retried.</p>
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
        {connectionProblem && <p className="sg-feedback" role="status">Slack connection: {connectionProblem}. {connection?.last_error}{connection?.next_retry_at != null && <> Next retry: {new Date(connection.next_retry_at * 1000).toLocaleTimeString()}.</>}</p>}
        {connection?.enabled && connection.state === 'unauthorized' && connection.pending_oauth_url && <p className="sg-feedback"><a href={connection.pending_oauth_url} target="_blank" rel="noopener noreferrer">Sign in to Slack</a></p>}
        {status.stale_scan_warning && <p className="sg-feedback" role="status">The last completed scan predates the current four-calendar-date window. Older triggers may have been missed.</p>}
        {status.scan?.last_error && <p className="sg-feedback sg-feedback--error" role="alert">{status.scan.last_error}</p>}
        <dl className="sg-stats"><div><dt>Last complete scan</dt><dd>{status.scan?.last_scan_at_unix_micros ? new Date(status.scan.last_scan_at_unix_micros / 1000).toLocaleString() : 'No complete scan yet'}</dd></div><div><dt>Calls in last pass</dt><dd>{status.scan?.calls_last_pass ?? 0}<small> MCP tool calls</small></dd></div><div><dt>Pages in last pass</dt><dd>{status.scan?.pages_last_pass ?? 0}<small> search pages</small></dd></div></dl>
        {status.scan?.retry_at_us && <p className="sg-hint">Retry after {new Date(status.scan.retry_at_us / 1000).toLocaleString()}.</p>}
        <h3 className="sg-list-title">Requests <span>{status.requests.length}</span></h3>
        {status.requests.length === 0 ? <p className="sg-empty">No accepted requests yet. Once initialization finishes, use a configured emoji in Slack to get started.</p> : <ul className="sg-activity-list">{status.requests.map(r => <li key={r.request_id}><div className="sg-activity-heading"><a href={r.source.source_url} target="_blank" rel="noreferrer">Slack {r.source.kind}</a><span className="sg-badge">{r.failure ? 'Failed' : r.admitted ? 'Admitted' : 'Accepted'}</span>{r.conversation_available && r.product_id && <a className="sg-open-link" href={'/c/' + r.product_id}>Open Phoenix</a>}</div><p className="sg-hint">👀 {r.eyes_failure ? 'unconfirmed' : r.eyes_confirmed ? 'confirmed' : 'pending'} · observed-to-accept {Math.max(0, (r.accepted_at_us - r.source.observed_at_unix_micros) / 1000).toFixed(0)} ms{r.source.trigger_kind === 'prefix' && <> · prefix-message age at observation {Math.max(0, r.source.observed_at_unix_micros / 1000 - Number(r.source.message_ts) * 1000).toFixed(0)} ms</>}</p>{r.eyes_failure && <p className="sg-error-text">{r.eyes_failure}</p>}{r.failure && <p className="sg-error-text">{r.failure}</p>}</li>)}</ul>}
        <h3 className="sg-list-title">Replies &amp; notifications <span>{status.returns.length}</span></h3>
        {status.returns.length === 0 ? <p className="sg-empty">No replies or private notices yet.</p> : <ul className="sg-activity-list">{status.returns.map(r => <li key={r.return_id} id={'slack-return-' + r.return_id}><div className="sg-activity-heading"><strong>{r.destination === 'thread' ? 'Thread reply' : 'Private notice'}</strong><span className={'sg-badge ' + (r.state === 'confirmed' ? 'sg-badge--success' : '')}>{r.destination === 'private' && r.state === 'submitted' ? 'submitted · delivery unchecked' : r.state.replaceAll('_', ' ')}</span><span className="sg-hint">covers {r.request_ids.length} request(s)</span></div>{r.failure && <p className="sg-error-text">{r.failure}</p>}{r.notification_receipt && <details className="sg-receipt"><summary>Submission receipt</summary><code>{r.notification_receipt}</code></details>}</li>)}</ul>}
        <p className="sg-hint sg-activity-footnote">Reaction-add latency is unavailable from Slack search. Private alerts and eyes are best effort. Find accepted work here even when an alert or reaction is missing.</p>
      </section>}
    </>}
  </div>;
}
