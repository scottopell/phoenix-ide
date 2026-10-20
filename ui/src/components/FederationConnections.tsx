import { useEffect, useState } from 'react';
import * as v from 'valibot';
import './FederationConnections.css';

const PeerSchema = v.object({
  peer_instance_id: v.string(),
  peer_display_name: v.string(),
  base_url: v.string(),
  created_at: v.string(),
});
type Peer = v.InferOutput<typeof PeerSchema>;
const NotesSchema = v.object({ notes: v.string() });
const NOTES_BYTES = 32 * 1024;

async function responseJson(response: Response): Promise<unknown> {
  if (!response.ok) throw new Error(`Request failed (${response.status})`);
  return response.json();
}

function NotesEditor({ peer }: { peer: Peer }) {
  const [text, setText] = useState<string | null>(null);
  const [saved, setSaved] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const path = `/api/federation/peers/${encodeURIComponent(peer.peer_instance_id)}/notes`;
  useEffect(() => {
    const controller = new AbortController();
    fetch(path, { signal: controller.signal }).then(responseJson).then((json) => {
      const document = v.parse(NotesSchema, json);
      setText(document.notes);
      setSaved(document.notes);
    }).catch(() => { if (!controller.signal.aborted) setError('Could not load notes. Reopen Connections to retry.'); });
    return () => controller.abort();
  }, [path]);
  const bytes = text === null ? 0 : new TextEncoder().encode(text).length;
  async function save() {
    if (text === null || bytes > NOTES_BYTES) return;
    setBusy(true);
    setError(null);
    try {
      const response = await fetch(path, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ notes: text }),
      });
      const document = v.parse(NotesSchema, await responseJson(response));
      setSaved(document.notes);
    } catch {
      setError('Could not save notes. Your edits are retained; retry Save.');
    } finally { setBusy(false); }
  }
  return <div className="federation-notes">
    <div>{peer.base_url}</div>
    <code>{peer.peer_instance_id}</code>
    <div className="settings-section__hint">Notes are observations, not authority. Do not store secrets.</div>
    {text === null && error === null ? <div>Loading notes…</div> : text !== null ? <>
      <textarea aria-label={`Notes for ${peer.peer_display_name}`} value={text} onChange={(event) => setText(event.target.value)} disabled={busy} />
      <div className="federation-notes__actions">
        <span>{bytes} / {NOTES_BYTES} UTF-8 bytes {text === saved ? '✓' : '· unsaved'}</span>
        <button type="button" className="settings-inline-btn" disabled={busy || bytes > NOTES_BYTES || text === saved} onClick={() => { void save(); }}>{busy ? 'Saving…' : 'Save'}</button>
      </div>
      {bytes > NOTES_BYTES && <div role="alert" className="settings-section__error">Notes exceed the 32 KiB limit.</div>}
    </> : null}
    {error && <div role="alert" className="settings-section__error">{error}</div>}
  </div>;
}

function ConnectionsList() {
  const [peers, setPeers] = useState<Peer[] | null>(null);
  const [error, setError] = useState(false);
  const [selected, setSelected] = useState('');
  useEffect(() => {
    const controller = new AbortController();
    fetch('/api/federation/peers', { signal: controller.signal }).then(responseJson).then((json) => {
      setPeers(v.parse(v.array(PeerSchema), json));
    }).catch(() => { if (!controller.signal.aborted) setError(true); });
    return () => controller.abort();
  }, []);
  const peer = peers?.find((item) => item.peer_instance_id === selected);
  return <>
    {error ? <div role="alert">Could not load connections. Reopen to retry.</div> : peers === null ? <div>Loading…</div> : peers.length === 0 ? <div>No saved peer connections.</div> : <>
      <label>Connection <select value={selected} onChange={(event) => setSelected(event.target.value)}>
        <option value="">Select a peer…</option>
        {peers.map((item) => <option key={item.peer_instance_id} value={item.peer_instance_id}>{item.peer_display_name} · {item.peer_instance_id}</option>)}
      </select></label>
      {peer && <NotesEditor key={peer.peer_instance_id} peer={peer} />}
    </>}
  </>;
}

export function FederationConnections() {
  const [open, setOpen] = useState(false);
  return <section className="settings-section federation-connections">
    <button type="button" className="settings-inline-btn" aria-expanded={open} onClick={() => setOpen(!open)}>Connections {open ? '▾' : '▸'}</button>
    {open && <ConnectionsList />}
  </section>;
}
