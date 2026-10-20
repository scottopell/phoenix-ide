import { useCallback, useEffect, useId, useRef, useState, type FormEvent, type Ref } from 'react';
import * as v from 'valibot';
import './FederationConnections.css';

const PeerSchema = v.object({
  peer_instance_id: v.string(),
  peer_display_name: v.string(),
  base_url: v.string(),
  created_at: v.string(),
});
type Peer = v.InferOutput<typeof PeerSchema>;
const ImportPeerResponseSchema = v.object({ peer_instance_id: v.string() });
const NotesSchema = v.object({ notes: v.string() });
const NOTES_BYTES = 32 * 1024;

async function responseJson(response: Response): Promise<unknown> {
  if (!response.ok) throw new Error(`Request failed (${response.status})`);
  return response.json();
}

async function fetchPeers(signal?: AbortSignal): Promise<Peer[]> {
  const response = await fetch('/api/federation/peers', signal ? { signal } : undefined);
  return v.parse(v.array(PeerSchema), await responseJson(response));
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

function EnrollmentImportForm({ onImported }: { onImported: (peerInstanceId: string) => Promise<void> }) {
  const [bundle, setBundle] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const inputId = useId();

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy || !bundle.trim()) return;
    let enrollment: unknown;
    try {
      enrollment = JSON.parse(bundle) as unknown;
    } catch {
      setError('Could not import connection. Check the enrollment bundle and retry.');
      setStatus(null);
      return;
    }

    setBusy(true);
    setError(null);
    setStatus(null);
    let accepted = false;
    try {
      const response = await fetch('/api/federation/peers/import', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ enrollment }),
      });
      if (!response.ok) throw new Error('Import rejected');
      accepted = true;
      setBundle('');
      const imported = v.parse(ImportPeerResponseSchema, await response.json());
      await onImported(imported.peer_instance_id);
      setStatus('Connection imported and selected.');
    } catch {
      setError(accepted
        ? 'Connection was imported, but Connections could not be refreshed. Reopen Settings to retry.'
        : 'Could not import connection. Check the enrollment bundle and destination, then retry.');
    } finally {
      setBusy(false);
    }
  }

  return <form className="federation-import" onSubmit={(event) => { void submit(event); }}>
    <label htmlFor={inputId}>Receiver-issued enrollment bundle</label>
    <div className="settings-section__hint">Treat this one-time JSON bundle as a secret. Paste the complete bundle; its name and URL are authoritative.</div>
    <textarea
      id={inputId}
      value={bundle}
      onChange={(event) => { setBundle(event.target.value); setError(null); setStatus(null); }}
      disabled={busy}
      spellCheck={false}
      autoComplete="off"
    />
    <button type="submit" className="settings-inline-btn" disabled={busy || !bundle.trim()}>{busy ? 'Importing…' : 'Import connection'}</button>
    {error && <div role="alert" className="settings-section__error">{error}</div>}
    {status && <div role="status" className="settings-section__success">{status}</div>}
  </form>;
}

function ConnectionsList() {
  const [peers, setPeers] = useState<Peer[] | null>(null);
  const [error, setError] = useState(false);
  const [selected, setSelected] = useState('');
  const requestGeneration = useRef(0);
  const loadPeers = useCallback(async (signal?: AbortSignal, selectAfterLoad?: string) => {
    const generation = ++requestGeneration.current;
    try {
      const nextPeers = await fetchPeers(signal);
      if (signal?.aborted || generation !== requestGeneration.current) return;
      setPeers(nextPeers);
      setError(false);
      if (selectAfterLoad) setSelected(selectAfterLoad);
    } catch (loadError) {
      if (signal?.aborted || generation !== requestGeneration.current) return;
      setError(true);
      throw loadError;
    }
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    void loadPeers(controller.signal).catch(() => undefined);
    return () => controller.abort();
  }, [loadPeers]);
  const peer = peers?.find((item) => item.peer_instance_id === selected);
  return <div className="federation-connections__content">
    <EnrollmentImportForm onImported={(peerInstanceId) => loadPeers(undefined, peerInstanceId)} />
    {error ? <div role="alert">Could not load connections. Reopen to retry.</div> : peers === null ? <div>Loading…</div> : peers.length === 0 ? <div>No saved peer connections.</div> : <>
      <label>Connection <select value={selected} onChange={(event) => setSelected(event.target.value)}>
        <option value="">Select a peer…</option>
        {peers.map((item) => <option key={item.peer_instance_id} value={item.peer_instance_id}>{item.peer_display_name} · {item.peer_instance_id}</option>)}
      </select></label>
      {peer && <NotesEditor key={peer.peer_instance_id} peer={peer} />}
    </>}
  </div>;
}

interface FederationConnectionsProps {
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  triggerRef?: Ref<HTMLButtonElement>;
}

export function FederationConnections({ open, onOpenChange, triggerRef }: FederationConnectionsProps = {}) {
  const [internalOpen, setInternalOpen] = useState(false);
  const expanded = open ?? internalOpen;
  const contentId = useId();
  function setExpanded(next: boolean) {
    if (open === undefined) setInternalOpen(next);
    onOpenChange?.(next);
  }
  return <section className="settings-section federation-connections">
    <button ref={triggerRef} type="button" className="settings-inline-btn" aria-expanded={expanded} aria-controls={contentId} onClick={() => setExpanded(!expanded)}>Connections {expanded ? '▾' : '▸'}</button>
    {expanded && <div id={contentId}><ConnectionsList /></div>}
  </section>;
}
