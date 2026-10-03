import { lazy, Suspense, useEffect, useRef, useState, type ReactNode } from 'react';
import { useLocation, useNavigate, useParams } from 'react-router-dom';
import { api, type LiveCoordinatorBashHandle } from '../api';
import { AutomaticContinuationControl } from '../components/AutomaticContinuationControl';
import { COORDINATOR_QUICK_ACTION } from './coordinatorBriefing';
import './CoordinatorPage.css';

const ConversationPage = lazy(() =>
  import('./ConversationPage').then((module) => ({ default: module.ConversationPage })),
);

interface CoordinatorPageFixtureData {
  coordinatorId: string;
  conversation: ReactNode;
}

function GlobalLiveCommands() {
  const [handles, setHandles] = useState<LiveCoordinatorBashHandle[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [stopping, setStopping] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      void api.listLiveCoordinatorBashHandles()
        .then((value) => { if (!cancelled) { setHandles(value); setError(null); } })
        .catch((reason: unknown) => { if (!cancelled) setError(reason instanceof Error ? reason.message : 'Failed to load live commands'); });
    };
    refresh();
    const timer = window.setInterval(refresh, 5_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, []);

  if (error) return <div className="global-live-commands-error">{error}</div>;
  if (handles.length === 0) return null;
  return (
    <section className="global-live-commands" aria-label="Running commands">
      <strong>Running</strong>
      {handles.map((handle) => (
        <div className="global-live-command" key={handle.handle_id}>
          <div>
            {handle.label && <strong>{handle.label}</strong>}
            <code>{handle.command}</code>
            <span>{handle.cwd} · started {new Date(handle.started_at_ms).toLocaleString()}</span>
            <span>{handle.handle_id}</span>
          </div>
          <div className="global-live-command-actions">
            <a href={`?viewer=inspect&handle=${encodeURIComponent(handle.handle_id)}`}>output →</a>
            {handle.can_stop && <button type="button" disabled={stopping === handle.handle_id} onClick={() => {
              setStopping(handle.handle_id);
              void api.stopLiveCoordinatorBashHandle(handle.handle_id)
                .then(() => undefined)
                .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'Failed to stop command'))
                .finally(() => setStopping(null));
            }}>{stopping === handle.handle_id ? 'stopping…' : 'stop'}</button>}
          </div>
        </div>
      ))}
    </section>
  );
}

export function CoordinatorPage({ fixtureData }: { fixtureData?: CoordinatorPageFixtureData }) {
  const navigate = useNavigate();
  const location = useLocation();
  const locationRef = useRef(location);
  locationRef.current = location;
  const { slug } = useParams<{ slug: string }>();
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(!fixtureData);
  const [resolvedCoordinatorId, setResolvedCoordinatorId] = useState<string | null>(fixtureData?.coordinatorId ?? null);
  const [topologyRevision, setTopologyRevision] = useState(0);
  const consumedTopologyRevision = useRef(0);

  useEffect(() => {
    const refresh = () => setTopologyRevision((value) => value + 1);
    window.addEventListener('phoenix:automatic-continuation-updated', refresh);
    return () => window.removeEventListener('phoenix:automatic-continuation-updated', refresh);
  }, []);

  useEffect(() => {
    if (fixtureData) return;
    setLoading(true);
    setResolvedCoordinatorId(null);
    let cancelled = false;
    api.ensureGlobalCoordinator()
      .then((coordinator) => {
        if (cancelled) return;
        window.dispatchEvent(new CustomEvent('phoenix:coordinator-ready', {
          detail: { conversation: coordinator.conversation },
        }));
        const topologyChanged = topologyRevision > consumedTopologyRevision.current;
        consumedTopologyRevision.current = topologyRevision;
        const pinnedSource = new URLSearchParams(locationRef.current.search).has('source_tool');
        if (topologyChanged && !pinnedSource && slug !== coordinator.conversation.id) {
          navigate(`/global/${coordinator.conversation.id}${locationRef.current.search}${locationRef.current.hash}`, { replace: true });
        } else if (!slug || slug === coordinator.conversation.id) {
          setResolvedCoordinatorId(coordinator.conversation.id);
          if (!slug) navigate(`/global/${coordinator.conversation.id}${locationRef.current.search}${locationRef.current.hash}`, { replace: true });
        } else {
          api.resolveCoordinatorRoute(slug)
            .then(({ coordinator_id }) => {
              if (cancelled) return;
              if (coordinator_id) {
                setResolvedCoordinatorId(slug);
              } else if (pinnedSource) {
                setError("Original source conversation unavailable");
              } else {
                navigate(`/global/${coordinator.conversation.id}${locationRef.current.search}${locationRef.current.hash}`, { replace: true });
              }
            })
            .catch(() => {
              if (!cancelled && pinnedSource) setError("Original source conversation unavailable");
              else if (!cancelled) navigate(`/global/${coordinator.conversation.id}${locationRef.current.search}${locationRef.current.hash}`, { replace: true });
            });
        }
        setError(null);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => { cancelled = true; };
  }, [fixtureData, navigate, slug, topologyRevision]);

  return (
    <main className="coordinator-page">
      {error && <div className="coordinator-error coordinator-page-status">{error}</div>}
      {loading ? <div className="coordinator-muted coordinator-page-status">Loading…</div> : null}

      {!loading && !error && resolvedCoordinatorId === slug && (
        <div className="coordinator-page__automatic-continuation">
          <AutomaticContinuationControl scope={{ kind: 'coordinator' }} />
        </div>
      )}

      {!fixtureData && !loading && !error && <GlobalLiveCommands />}

      <section className="coordinator-conversation" aria-label="Coordinator conversation">
        {slug === resolvedCoordinatorId ? fixtureData?.conversation ?? (
          <Suspense fallback={<div className="coordinator-muted">Loading Coordinator conversation…</div>}>
            <ConversationPage routePrefix="/global" composerQuickAction={COORDINATOR_QUICK_ACTION} />
          </Suspense>
        ) : null}
      </section>
    </main>
  );
}
