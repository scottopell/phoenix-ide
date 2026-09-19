import { useContext, useEffect, useRef, useState } from 'react';
import { SelectionDialog } from './SelectionDialog';
import type { SvgArtifact } from './svgArtifact';
import './SvgArtifactCard.css';
import { SvgArtifactAccessContext } from '../contexts/SvgArtifactAccessContext';

function ArtifactImage({ artifact, url, zoom = 1 }: { artifact: SvgArtifact; url: string; zoom?: number }) {
  const [state, setState] = useState<'loading' | 'ready' | 'error'>('loading');
  return <div className="svg-artifact-image" style={{ width: zoom === 1 ? '100%' : `${zoom * 100}%` }}>
    {state === 'loading' && <span role="status">Loading visualization…</span>}
    {state === 'error' ? <p role="alert">Visualization unavailable or unreadable. {artifact.description}</p> : <img
      src={url} alt={artifact.description} width={artifact.width} height={artifact.height}
      onLoad={() => setState('ready')} onError={() => setState('error')}
    />}
  </div>;
}

function ArtifactSource({ url }: { url: string }) {
  const [source, setSource] = useState<{ text: string } | { error: true } | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    void fetch(`${url}/source`, { signal: controller.signal, credentials: 'same-origin' }).then(async (response) => {
      if (!response.ok) throw new Error('Source unavailable');
      const text = await response.text();
      if (!controller.signal.aborted) setSource({ text });
    }).catch(() => { if (!controller.signal.aborted) setSource({ error: true }); });
    return () => controller.abort();
  }, [url]);
  if (!source) return <p role="status">Loading source…</p>;
  return 'error' in source ? <p role="alert">Source unavailable. Try opening it again.</p> : <pre className="svg-artifact-source"><code>{source.text}</code></pre>;
}

export function SvgArtifactCard({ artifact }: { artifact: SvgArtifact }) {
  const [view, setView] = useState<'image' | 'source' | null>(null);
  const [zoom, setZoom] = useState(1);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const access = useContext(SvgArtifactAccessContext);
  const base = access.kind === 'share'
    ? `/api/share/${encodeURIComponent(access.token)}`
    : `/api/conversations/${encodeURIComponent(artifact.conversation_id)}`;
  const url = `${base}/svg-artifacts/${encodeURIComponent(artifact.artifact_id)}`;
  return <section className="svg-artifact-card" aria-label={artifact.title}>
    <h3>{artifact.title}</h3>
    <p>{artifact.description}</p>
    <ArtifactImage artifact={artifact} url={url} />
    <div className="svg-artifact-controls">
      <button type="button" onClick={(event) => { triggerRef.current = event.currentTarget; setZoom(1); setView('image'); }}>Expand visualization</button>
      <button type="button" onClick={(event) => { triggerRef.current = event.currentTarget; setView('source'); }}>View source</button>
      <a href={`${url}/download`} download>Download SVG</a>
    </div>
    {view && <SelectionDialog title={view === 'source' ? `Source: ${artifact.title}` : artifact.title} description={artifact.description}
      onClose={() => setView(null)} restoreFocusRef={triggerRef} className="svg-artifact-dialog">
      {view === 'source' ? <ArtifactSource url={url} /> : <>
        <div className="svg-artifact-controls">
          <button type="button" disabled={zoom <= 1} onClick={() => setZoom((value) => Math.max(1, value - 0.5))}>Zoom out</button>
          <output aria-label="Zoom level">{Math.round(zoom * 100)}%</output>
          <button type="button" disabled={zoom >= 4} onClick={() => setZoom((value) => Math.min(4, value + 0.5))}>Zoom in</button>
        </div>
        <div className="svg-artifact-viewport"><ArtifactImage artifact={artifact} url={url} zoom={zoom} /></div>
      </>}
    </SelectionDialog>}
  </section>;
}
