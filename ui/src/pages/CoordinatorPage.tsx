import { lazy, Suspense, useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router-dom';
import { api, type ActiveCoordinatorWatch, type LiveCoordinatorBashHandle } from '../api';
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

function humanizeState(state: string): string {
  return state.replaceAll('_', ' ').replace(/^./, (first) => first.toUpperCase());
}

type ActivityCount = number | null;

function GlobalActiveWatches({ onCount }: { onCount: (count: ActivityCount) => void }) {
  const [watches, setWatches] = useState<ActiveCoordinatorWatch[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;
    const refresh = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const value = await api.listActiveCoordinatorWatches();
        if (!cancelled) { setWatches(value); setError(null); onCount(value.length); }
      } catch {
        if (!cancelled) { setError('Could not refresh active watches'); onCount(null); }
      } finally {
        inFlight = false;
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [onCount]);

  if (watches.length === 0 && !error) return null;
  return (
    <section className="global-active-watches" aria-label="Active watches">
      <strong>Watching</strong>
      {error && <span className="coordinator-error" role="status">{error}</span>}
      {watches.map((watch) => (
        <div className="global-active-watch" key={watch.product_conversation_id}>
          <div>
            <Link to={`/c/${watch.product_conversation_id}`}>{watch.display_name}</Link>
            <span>{humanizeState(watch.state)}</span>
          </div>
          <Link to={`/c/${encodeURIComponent(watch.transcript_slug || watch.transcript_id)}`}>current transcript</Link>
          <details className="global-activity-metadata">
            <summary>Details</summary>
            {watch.project_path && <code>{watch.project_path}</code>}
            <code title="ProductConversation ID">{watch.product_conversation_id}</code>
          </details>
        </div>
      ))}
    </section>
  );
}

function GlobalLiveCommands({ onCount }: { onCount: (count: ActivityCount) => void }) {
  const location = useLocation();
  const [handles, setHandles] = useState<LiveCoordinatorBashHandle[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [stopping, setStopping] = useState<Set<string>>(() => new Set());

  useEffect(() => {
    let cancelled = false;
    let inFlight = false;
    const refresh = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const value = await api.listLiveCoordinatorBashHandles();
        if (!cancelled) { setHandles(value); setError(null); onCount(value.length); }
      } catch (reason) {
        if (!cancelled) { setError(reason instanceof Error ? reason.message : 'Failed to load live commands'); onCount(null); }
      } finally {
        inFlight = false;
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [onCount]);

  const inspectTarget = (handleId: string) => {
    const search = new URLSearchParams(location.search);
    search.set('viewer', 'inspect');
    search.set('handle', handleId);
    return { pathname: location.pathname, search: `?${search.toString()}`, hash: location.hash };
  };

  if (handles.length === 0 && !error) return null;
  return (
    <section className="global-live-commands" aria-label="Running commands">
      <strong>Running</strong>
      {error && <span className="global-live-commands-error" role="status">{error}</span>}
      {handles.map((handle) => (
        <div className="global-live-command" key={handle.handle_id}>
          <div>
            {handle.label && <strong>{handle.label}</strong>}
            <code>{handle.command}</code>
            <span>Started {new Date(handle.started_at_ms).toLocaleString()}</span>
            <details className="global-activity-metadata">
              <summary>Details</summary>
              <code>{handle.cwd}</code>
              <code>{handle.handle_id}</code>
            </details>
          </div>
          <div className="global-live-command-actions">
            <Link to={inspectTarget(handle.handle_id)}>output →</Link>
            {handle.can_stop && <button type="button" disabled={stopping.has(handle.handle_id)} onClick={() => {
              setStopping((current) => new Set(current).add(handle.handle_id));
              void api.stopLiveCoordinatorBashHandle(handle.handle_id)
                .then(() => undefined)
                .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'Failed to stop command'))
                .finally(() => setStopping((current) => {
                  const next = new Set(current);
                  next.delete(handle.handle_id);
                  return next;
                }));
            }}>{stopping.has(handle.handle_id) ? 'stopping…' : 'stop'}</button>}
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
  const [watchCount, setWatchCount] = useState<ActivityCount>(null);
  const [runningCount, setRunningCount] = useState<ActivityCount>(null);
  const [automaticContinuationStatus, setAutomaticContinuationStatus] = useState('…');
  const [automaticContinuationNeedsAttention, setAutomaticContinuationNeedsAttention] = useState(false);
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
    setResolvedCoordinatorId(null);
    setError(null);
    api.ensureGlobalCoordinator()
      .then(async (coordinator) => {
        if (cancelled) return;
        window.dispatchEvent(new CustomEvent('phoenix:coordinator-ready', {
          detail: { conversation: coordinator.conversation },
        }));
        const topologyChanged = topologyRevision > consumedTopologyRevision.current;
        consumedTopologyRevision.current = topologyRevision;
        const query = new URLSearchParams(locationRef.current.search);
        const pins = query.getAll('source_transcript');
        const pinnedSource = pins.length > 0 || query.has('source_tool');
        if (pins.length > 0) {
          const pin = pins[0];
          if (pins.length !== 1 || !pin || pin.trim() !== pin) {
            setError('Original source conversation unavailable');
            return;
          }
          const [owner, selected] = await Promise.all([
            api.resolveCoordinatorRoute(slug ?? coordinator.conversation.id),
            api.resolveCoordinatorRoute(pin),
          ]);
          if (cancelled) return;
          if (!owner.coordinator_id || owner.coordinator_id !== selected.coordinator_id) {
            setError('Original source conversation unavailable');
            return;
          }
          setResolvedCoordinatorId(pin);
          if (slug !== pin) navigate(`/global/${pin}${locationRef.current.search}${locationRef.current.hash}`, { replace: true });
          return;
        }
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
  }, [fixtureData, navigate, slug, topologyRevision, location.search]);

  const handleAutomaticContinuationStatus = useCallback((status: string, requiresAttention: boolean) => {
    setAutomaticContinuationStatus(status);
    setAutomaticContinuationNeedsAttention(requiresAttention);
  }, []);

  const stateBarExtension = fixtureData ? undefined : {
    summary: (
      <span className="global-statebar-summary">
        <span>Watching {watchCount ?? '…'}</span>
        <span>Running {runningCount ?? '…'}</span>
        <span className="global-statebar-summary__auto" title={`Auto-continue ${automaticContinuationStatus}`}>Auto {automaticContinuationStatus}</span>
      </span>
    ),
    details: (
      <div className="global-statebar-activity__details">
        <AutomaticContinuationControl
          scope={{ kind: 'coordinator' }}
          onStatusChange={handleAutomaticContinuationStatus}
        />
        <GlobalActiveWatches onCount={setWatchCount} />
        <GlobalLiveCommands onCount={setRunningCount} />
      </div>
    ),
    requiresAttention: automaticContinuationNeedsAttention,
  };

  return (
    <main className="coordinator-page">
      {error && <div className="coordinator-error coordinator-page-status">{error}</div>}
      {loading ? <div className="coordinator-muted coordinator-page-status">Loading…</div> : null}



      <section className="coordinator-conversation" aria-label="Coordinator conversation">
        {slug === resolvedCoordinatorId ? fixtureData?.conversation ?? (
          <Suspense fallback={<div className="coordinator-muted">Loading Coordinator conversation…</div>}>
            <ConversationPage routePrefix="/global" composerQuickAction={COORDINATOR_QUICK_ACTION} stateBarExtension={stateBarExtension} />
          </Suspense>
        ) : null}
      </section>
    </main>
  );
}
