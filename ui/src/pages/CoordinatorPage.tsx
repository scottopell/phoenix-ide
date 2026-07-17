import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router-dom';
import { api, type ConversationState, type GlobalOpenWorkResponse } from '../api';
import { useMediaQuery } from '../hooks';
import { useConversationSnapshot } from '../conversation';
import './CoordinatorPage.css';

const ConversationPage = lazy(() =>
  import('./ConversationPage').then((module) => ({ default: module.ConversationPage })),
);

type CoordinatorView = 'conversation' | 'work';

interface CoordinatorPageFixtureData {
  coordinatorId: string;
  openWork: GlobalOpenWorkResponse;
  initialView: CoordinatorView;
  workError?: string;
  conversation: ReactNode;
}

interface OpenWorkState {
  data: GlobalOpenWorkResponse | null;
  error: string | null;
  loading: boolean;
  loadingMore: boolean;
  queryInput: string;
  appliedQuery: string;
  refreshedAt: string | null;
}

const WORKING_STATE_TYPES = new Set<ConversationState['type']>([
  'awaiting_llm',
  'llm_requesting',
  'seeded_llm_requesting',
  'tool_executing',
]);

export function CoordinatorPage({ fixtureData }: { fixtureData?: CoordinatorPageFixtureData }) {
  const navigate = useNavigate();
  const { slug } = useParams<{ slug: string }>();
  const location = useLocation();
  const compactLayout = useMediaQuery('(max-width: 1024px)');
  const [activeView, setActiveView] = useState<CoordinatorView>(fixtureData?.initialView ?? 'conversation');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(!fixtureData);
  const [resolvedCoordinatorId, setResolvedCoordinatorId] = useState<string | null>(fixtureData?.coordinatorId ?? null);
  const [openWork, setOpenWork] = useState<OpenWorkState>({
    data: fixtureData?.openWork ?? null,
    error: fixtureData?.workError ?? null,
    loading: !fixtureData,
    loadingMore: false,
    queryInput: '',
    appliedQuery: '',
    refreshedAt: fixtureData?.openWork.generated_at ?? null,
  });
  const coordinatorConversation = useConversationSnapshot(slug ?? null);
  const previousCoordinatorState = useRef<ConversationState['type'] | null>(coordinatorConversation?.state?.type ?? null);

  const refreshOpenWork = useCallback(async (
    options?: { offset?: number; query?: string; append?: boolean },
  ) => {
    if (fixtureData) return;
    const offset = options?.offset ?? 0;
    const query = options?.query ?? openWork.appliedQuery;
    const append = options?.append ?? false;

    setOpenWork((prev) => ({
      ...prev,
      error: null,
      loading: !append,
      loadingMore: append,
      ...(append ? {} : { appliedQuery: query }),
    }));

    try {
      const page = await api.getGlobalOpenWork(offset, query);
      setOpenWork((prev) => ({
        ...prev,
        data: append && prev.data
          ? {
            generated_at: page.generated_at,
            has_more: page.has_more,
            groups: mergeOpenWorkGroups(prev.data.groups, page.groups),
          }
          : page,
        error: null,
        loading: false,
        loadingMore: false,
        appliedQuery: query,
        refreshedAt: page.generated_at,
      }));
    } catch (e) {
      setOpenWork((prev) => ({
        ...prev,
        error: e instanceof Error ? e.message : String(e),
        loading: false,
        loadingMore: false,
        appliedQuery: query,
      }));
    }
  }, [fixtureData, openWork.appliedQuery]);

  useEffect(() => {
    if (fixtureData) return;
    setLoading(true);
    let cancelled = false;
    api.ensureGlobalCoordinator()
      .then((coordinator) => {
        if (cancelled) return;
        window.dispatchEvent(new CustomEvent('phoenix:coordinator-ready', {
          detail: { conversation: coordinator.conversation },
        }));
        if (!slug || slug === coordinator.conversation.id) {
          setResolvedCoordinatorId(coordinator.conversation.id);
          if (!slug) navigate(`/global/${coordinator.conversation.id}`, { replace: true });
        } else {
          api.resolveCoordinatorRoute(slug)
            .then(({ coordinator_id }) => {
              if (cancelled) return;
              if (coordinator_id) setResolvedCoordinatorId(slug);
              else navigate(`/global/${coordinator.conversation.id}`, { replace: true });
            })
            .catch(() => {
              if (!cancelled) navigate(`/global/${coordinator.conversation.id}`, { replace: true });
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
    void refreshOpenWork({ query: '' });
    return () => { cancelled = true; };
  }, [fixtureData, navigate, refreshOpenWork, slug]);

  useEffect(() => {
    if (!fixtureData && compactLayout) setActiveView('conversation');
  }, [compactLayout, fixtureData, location.key]);

  useEffect(() => {
    if (fixtureData) return;
    const refreshIfVisible = () => {
      if (document.visibilityState === 'visible') {
        void refreshOpenWork();
      }
    };
    window.addEventListener('focus', refreshIfVisible);
    document.addEventListener('visibilitychange', refreshIfVisible);
    return () => {
      window.removeEventListener('focus', refreshIfVisible);
      document.removeEventListener('visibilitychange', refreshIfVisible);
    };
  }, [fixtureData, refreshOpenWork]);

  useEffect(() => {
    if (fixtureData) return;
    const previous = previousCoordinatorState.current;
    const current = coordinatorConversation?.state?.type ?? null;
    previousCoordinatorState.current = current;
    if (previous && current && WORKING_STATE_TYPES.has(previous) && !WORKING_STATE_TYPES.has(current)) {
      void refreshOpenWork();
    }
  }, [coordinatorConversation?.state?.type, fixtureData, refreshOpenWork]);

  const itemCount = useMemo(
    () => openWork.data?.groups.reduce((sum, group) => sum + group.items.length, 0) ?? 0,
    [openWork.data],
  );
  const attentionSummary = useMemo(() => summarizeAttention(openWork.data), [openWork.data]);
  const freshnessLabel = useMemo(
    () => formatFreshness(openWork.refreshedAt ?? openWork.data?.generated_at ?? null),
    [openWork.data?.generated_at, openWork.refreshedAt],
  );
  const queryDirty = openWork.queryInput !== openWork.appliedQuery;

  const submitQuery = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const submitted = new FormData(form).get('query');
    const query = typeof submitted === 'string' ? submitted.trim() : openWork.queryInput.trim();
    setOpenWork((prev) => ({ ...prev, queryInput: query }));
    void refreshOpenWork({ query });
  };

  const clearQuery = () => {
    setOpenWork((prev) => ({ ...prev, queryInput: '' }));
    void refreshOpenWork({ query: '' });
  };

  const loadMoreOpenWork = () => {
    if (!openWork.data || !openWork.data.has_more || openWork.loadingMore) return;
    void refreshOpenWork({
      offset: itemCount,
      query: openWork.appliedQuery,
      append: true,
    });
  };

  return (
    <div className={`coordinator-page coordinator-page--${activeView}`}>
      <header className="coordinator-header">
        <Link className="coordinator-back" to="/" aria-label="Back to conversations">
          <span aria-hidden="true">←</span>
        </Link>
        <div className="coordinator-heading">
          <h1>Coordinator</h1>
          <p>Keep the chat in view, then pivot into current work when something needs attention.</p>
        </div>
        <div className="coordinator-view-switch" role="tablist" aria-label="Coordinator view">
          <button
            type="button"
            role="tab"
            aria-selected={activeView === 'conversation'}
            className="coordinator-view-tab"
            onClick={() => setActiveView('conversation')}
          >
            Conversation
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={activeView === 'work'}
            className="coordinator-view-tab"
            onClick={() => setActiveView('work')}
          >
            Work <span className="coordinator-view-count">{itemCount}</span>
          </button>
        </div>
      </header>

      {error && <div className="coordinator-error">{error}</div>}
      {loading ? <div className="coordinator-muted">Loading…</div> : null}

      <section
        className="coordinator-conversation"
        aria-label="Coordinator conversation"
        hidden={compactLayout && activeView !== 'conversation'}
      >
        {slug === resolvedCoordinatorId ? fixtureData?.conversation ?? (
          <Suspense fallback={<div className="coordinator-muted">Loading Coordinator conversation…</div>}>
            <ConversationPage routePrefix="/global" />
          </Suspense>
        ) : null}
      </section>

      <aside
        className="coordinator-work-pane"
        aria-label="Coordinator work"
        hidden={compactLayout && activeView !== 'work'}
      >
        <section className="coordinator-attention-card">
          <div>
            <div className="coordinator-kicker">Attention</div>
            <h2>{attentionSummary.title}</h2>
            <p>{attentionSummary.detail}</p>
          </div>
          <button type="button" onClick={() => { void refreshOpenWork(); }} disabled={openWork.loading || openWork.loadingMore}>
            Refresh
          </button>
        </section>

        <section className="coordinator-find-work">
          <div className="coordinator-find-work-header">
            <div>
              <div className="coordinator-kicker">Find work</div>
              <h2>Deterministic query</h2>
            </div>
            <div className="coordinator-freshness">{freshnessLabel}</div>
          </div>

          <form className="coordinator-query-form" onSubmit={submitQuery}>
            <label className="coordinator-query-field">
              <span className="coordinator-sr-only">Search open work</span>
              <input
                type="search"
                name="query"
                value={openWork.queryInput}
                onChange={(event) => {
                  const value = event.target.value;
                  setOpenWork((prev) => ({ ...prev, queryInput: value }));
                }}
                placeholder="task id, branch, title, signal"
              />
            </label>
            <button type="submit" disabled={openWork.loading || openWork.loadingMore}>Apply</button>
            <button type="button" disabled={!openWork.queryInput && !openWork.appliedQuery} onClick={clearQuery}>Clear</button>
          </form>
          <div className="coordinator-query-status" aria-live="polite">
            {openWork.appliedQuery ? `Showing results for “${openWork.appliedQuery}”` : 'Showing all open work'}
            {queryDirty ? ' · unapplied edits' : ''}
          </div>

          {openWork.error && (
            <div className="coordinator-error coordinator-work-error">
              <span>Work unavailable: {openWork.error}</span>
              <button type="button" onClick={() => { void refreshOpenWork(); }}>Retry</button>
            </div>
          )}

          {openWork.loading ? <div className="coordinator-muted">Refreshing work…</div> : null}
          {openWork.data?.groups.length === 0 ? <p className="coordinator-muted">No open work found.</p> : null}

          <div className="coordinator-projects">
            {openWork.data?.groups.map((group) => (
              <section className="coordinator-project" key={group.project_id ?? 'none'}>
                <div className="coordinator-project-header">
                  <div>
                    <h3>{group.project_name}</h3>
                    {group.canonical_path && <div className="coordinator-path">{group.canonical_path}</div>}
                  </div>
                  <span className="coordinator-project-count">{group.items.length}</span>
                </div>
                <div className="coordinator-items">
                  {group.items.map((item) => (
                    <Link className="coordinator-item" key={item.reference} to={item.href}>
                      <div className="coordinator-item-topline">
                        <span className={`coordinator-state-pill coordinator-state-pill--${attentionTone(item)}`}>{item.state}</span>
                        <span>{formatUpdatedAt(item.updated_at)}</span>
                      </div>
                      <strong className="coordinator-item-title">{item.title}</strong>
                      <div className="coordinator-item-meta">
                        <span>{item.source === 'chain' ? 'Chain' : 'Conversation'}</span>
                        <span>{item.mode}</span>
                        {item.task_id ? <span>TASK {item.task_id}</span> : null}
                        {!item.task_id && item.branch_name ? <span>{item.branch_name}</span> : null}
                      </div>
                      <div className="coordinator-item-signals">
                        {item.signals.length > 0
                          ? item.signals.slice(0, 3).map((signal) => <span key={signal}>{signal}</span>)
                          : <span>No special signals</span>}
                      </div>
                    </Link>
                  ))}
                </div>
              </section>
            ))}
          </div>

          {openWork.data?.has_more && (
            <button type="button" onClick={loadMoreOpenWork} disabled={openWork.loadingMore}>
              {openWork.loadingMore ? 'Loading…' : 'Load more work'}
            </button>
          )}
        </section>
      </aside>
    </div>
  );
}

function mergeOpenWorkGroups(
  current: GlobalOpenWorkResponse['groups'],
  incoming: GlobalOpenWorkResponse['groups'],
): GlobalOpenWorkResponse['groups'] {
  const groups = current.map((group) => ({ ...group, items: [...group.items] }));
  for (const incomingGroup of incoming) {
    const existing = groups.find((group) => group.project_id === incomingGroup.project_id);
    if (existing) {
      existing.items.push(...incomingGroup.items.filter((item) => !existing.items.some((currentItem) => currentItem.reference === item.reference)));
    } else {
      groups.push(incomingGroup);
    }
  }
  return groups;
}

function summarizeAttention(openWork: GlobalOpenWorkResponse | null): { title: string; detail: string } {
  const items = openWork?.groups.flatMap((group) => group.items) ?? [];
  if (items.length === 0) {
    return { title: 'Nothing is asking for attention', detail: 'No open work is currently visible in the shared work snapshot.' };
  }

  const needsAction = items.filter((item) => item.state === 'needs_action' || item.signals.some((signal) => /needs action|awaiting user|blocked/i.test(signal)));
  if (needsAction.length > 0) {
    return {
      title: `${needsAction.length} conversation${needsAction.length === 1 ? '' : 's'} need attention`,
      detail: needsAction.slice(0, 2).map((item) => item.title).join(' · '),
    };
  }

  const errors = items.filter((item) => item.state === 'error' || item.signals.some((signal) => /error|failed/i.test(signal)));
  if (errors.length > 0) {
    return {
      title: `${errors.length} conversation${errors.length === 1 ? '' : 's'} are blocked`,
      detail: errors.slice(0, 2).map((item) => item.title).join(' · '),
    };
  }

  const active = items.filter((item) => item.state === 'working' || item.signals.some((signal) => /active|running/i.test(signal)));
  if (active.length > 0) {
    return {
      title: `${active.length} conversation${active.length === 1 ? '' : 's'} are active`,
      detail: active.slice(0, 2).map((item) => item.title).join(' · '),
    };
  }

  return {
    title: `${items.length} open conversation${items.length === 1 ? '' : 's'}`,
    detail: items.slice(0, 2).map((item) => item.title).join(' · '),
  };
}

function formatFreshness(timestamp: string | null): string {
  if (!timestamp) return 'Not refreshed yet';
  return `Snapshot ${new Date(timestamp).toLocaleString()}`;
}

function formatUpdatedAt(timestamp: string): string {
  return new Date(timestamp).toLocaleString();
}

function attentionTone(item: GlobalOpenWorkResponse['groups'][number]['items'][number]): 'urgent' | 'warning' | 'working' | 'idle' {
  if (item.state === 'needs_action' || item.signals.some((signal) => /needs action|awaiting user|blocked/i.test(signal))) return 'urgent';
  if (item.state === 'error' || item.signals.some((signal) => /error|failed/i.test(signal))) return 'warning';
  if (item.state === 'working' || item.signals.some((signal) => /active|running/i.test(signal))) return 'working';
  return 'idle';
}
