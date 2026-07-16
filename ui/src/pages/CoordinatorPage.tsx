import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { api } from '../api';
import { useMediaQuery } from '../hooks';
import type { GlobalOpenWorkResponse } from '../api';
import './CoordinatorPage.css';

const ConversationPage = lazy(() =>
  import('./ConversationPage').then((module) => ({ default: module.ConversationPage })),
);

type CoordinatorView = 'conversation' | 'fleet';

export function CoordinatorPage() {
  const navigate = useNavigate();
  const { slug } = useParams<{ slug: string }>();
  const compactLayout = useMediaQuery('(max-width: 1024px)');
  const [activeView, setActiveView] = useState<CoordinatorView>('conversation');
  const [openWork, setOpenWork] = useState<GlobalOpenWorkResponse | null>(null);
  const [openWorkLoadingMore, setOpenWorkLoadingMore] = useState(false);
  const [expandedReferences, setExpandedReferences] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [fleetError, setFleetError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [resolvedCoordinatorId, setResolvedCoordinatorId] = useState<string | null>(null);
  const [copiedReference, setCopiedReference] = useState<string | null>(null);

  const refreshOpenWork = useCallback(() => {
    setFleetError(null);
    api.getGlobalOpenWork()
      .then((work) => {
        setOpenWork(work);
        setFleetError(null);
      })
      .catch((e) => setFleetError(e instanceof Error ? e.message : String(e)));
  }, []);

  useEffect(() => {
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
    api.getGlobalOpenWork()
      .then((work) => {
        if (!cancelled) {
          setOpenWork(work);
          setFleetError(null);
        }
      })
      .catch((e) => {
        if (!cancelled) setFleetError(e instanceof Error ? e.message : String(e));
      });
    return () => { cancelled = true; };
  }, [navigate, slug]);

  const itemCount = useMemo(
    () => openWork?.groups.reduce((sum, group) => sum + group.items.length, 0) ?? 0,
    [openWork],
  );

  const loadMoreOpenWork = async () => {
    if (!openWork || !openWork.has_more || openWorkLoadingMore) return;
    setOpenWorkLoadingMore(true);
    setError(null);
    try {
      const page = await api.getGlobalOpenWork(itemCount);
      setOpenWork((prev) => prev ? {
        generated_at: page.generated_at,
        has_more: page.has_more,
        groups: mergeOpenWorkGroups(prev.groups, page.groups),
      } : page);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setOpenWorkLoadingMore(false);
    }
  };

  const copyReference = async (reference: string) => {
    setError(null);
    setCopiedReference(null);
    try {
      await navigator.clipboard.writeText(reference);
      setCopiedReference(reference);
    } catch (e) {
      setError(e instanceof Error ? `Failed to copy reference: ${e.message}` : 'Failed to copy reference');
    }
  };

  const toggleExpanded = (reference: string) => {
    setExpandedReferences((prev) => {
      const next = new Set(prev);
      if (next.has(reference)) next.delete(reference);
      else next.add(reference);
      return next;
    });
  };

  return (
    <div className={`coordinator-page coordinator-page--${activeView}`}>
      <header className="coordinator-header">
        <button
          type="button"
          className="coordinator-back"
          onClick={() => navigate(-1)}
          aria-label="Back"
        >
          <span aria-hidden="true">←</span>
        </button>
        <div className="coordinator-heading">
          <h1>Coordinator</h1>
          <p>The durable Phoenix-wide coordinator plus a compact, explainable fleet snapshot.</p>
        </div>
        <div className="coordinator-actions">
          <button type="button" onClick={refreshOpenWork}>Refresh fleet</button>
        </div>
        <div className="coordinator-view-switch" role="group" aria-label="Coordinator view">
          <button
            type="button"
            aria-pressed={activeView === 'conversation'}
            onClick={() => setActiveView('conversation')}
          >
            Conversation
          </button>
          <button
            type="button"
            aria-pressed={activeView === 'fleet'}
            onClick={() => setActiveView('fleet')}
          >
            Fleet <span className="coordinator-view-count">{itemCount}</span>
          </button>
        </div>
      </header>

      {error && <div className="coordinator-error">{error}</div>}
      <div className="coordinator-sr-only" aria-live="polite">
        {copiedReference ? `Copied ${copiedReference}` : ''}
      </div>
      {loading ? <div className="coordinator-muted">Loading…</div> : null}

      {slug === resolvedCoordinatorId && (
        <section
          className="coordinator-conversation"
          aria-label="Coordinator conversation"
          hidden={compactLayout && activeView !== 'conversation'}
        >
          <Suspense fallback={<div className="coordinator-muted">Loading Coordinator conversation…</div>}>
            <ConversationPage routePrefix="/global" />
          </Suspense>
        </section>
      )}

      <div
        className="coordinator-fleet-pane"
        hidden={compactLayout && activeView !== 'fleet'}
      >
        <section className="coordinator-open-work">
        {fleetError && <div className="coordinator-error">Fleet unavailable: {fleetError}</div>}
        <div className="coordinator-section-title">
          <h2>Fleet</h2>
          <span>{itemCount} item{itemCount === 1 ? '' : 's'}</span>
        </div>
        <p className="coordinator-muted">Compact rows show project, title, presentation state, recency, and key task or branch identity. Expand a row for audit detail.</p>
        {openWork?.groups.length === 0 ? <p className="coordinator-muted">No active work found.</p> : null}
        {openWork?.groups.map((group) => (
          <section className="coordinator-project" key={group.project_id ?? 'none'}>
            <div className="coordinator-project-header">
              <div>
                <h3>{group.project_name}</h3>
                {group.canonical_path && <div className="coordinator-path">{group.canonical_path}</div>}
              </div>
              <span className="coordinator-project-count">{group.items.length}</span>
            </div>
            <div className="coordinator-items">
              {group.items.map((item) => {
                const expanded = expandedReferences.has(item.reference);
                return (
                  <article className="coordinator-item" key={item.reference}>
                    <div className="coordinator-item-row">
                      <div className="coordinator-item-main">
                        <div className="coordinator-item-title-row">
                          <Link to={item.href}>{item.title}</Link>
                          <span className="coordinator-state-pill">{item.state}</span>
                        </div>
                        <div className="coordinator-compact-meta">
                          <span>{item.source}</span>
                          <span>{item.mode}</span>
                          <span>{new Date(item.updated_at).toLocaleString()}</span>
                          {item.task_id && <span>TASK {item.task_id}</span>}
                          {!item.task_id && item.branch_name && <span>BRANCH {item.branch_name}</span>}
                        </div>
                      </div>
                      <div className="coordinator-item-actions">
                        <button type="button" onClick={() => toggleExpanded(item.reference)} aria-expanded={expanded}>
                          {expanded ? 'Hide details' : 'Show details'}
                        </button>
                        <button type="button" onClick={() => { void copyReference(item.reference); }}>
                          Copy ref
                        </button>
                      </div>
                    </div>
                    {expanded && (
                      <div className="coordinator-item-details">
                        <div className="coordinator-signals">
                          {item.signals.map((signal) => <span key={signal}>{signal}</span>)}
                        </div>
                        <div className="coordinator-work-meta">
                          <span>CURRENT {shortId(item.current_conversation_id)}</span>
                          {item.source === 'chain' && <span>ROOT {shortId(item.root_conversation_id)}</span>}
                          {item.member_count > 1 && <span>{item.member_count} convs</span>}
                          {item.worktree_path && <span>WORKTREE {item.worktree_path}</span>}
                          {item.task_title && <span>TASK {item.task_id ?? ''} {item.task_status ? `· ${item.task_status}` : ''} · {item.task_title}</span>}
                          {item.branch_name && <span>BRANCH {item.branch_name}{item.base_branch ? ` ← ${item.base_branch}` : ''}</span>}
                          <span>REF {item.reference}</span>
                        </div>
                      </div>
                    )}
                  </article>
                );
              })}
            </div>
          </section>
        ))}
        {openWork?.has_more && (
          <button type="button" onClick={() => { void loadMoreOpenWork(); }} disabled={openWorkLoadingMore}>
            {openWorkLoadingMore ? 'Loading…' : 'Load more fleet items'}
          </button>
        )}
        </section>
      </div>
    </div>
  );
}

function shortId(id: string): string {
  return id.slice(0, 8);
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
