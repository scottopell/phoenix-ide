import { lazy, Suspense, useEffect, useState, type ReactNode } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { api } from '../api';
import { COORDINATOR_QUICK_ACTION } from './coordinatorBriefing';
import './CoordinatorPage.css';

const ConversationPage = lazy(() =>
  import('./ConversationPage').then((module) => ({ default: module.ConversationPage })),
);

interface CoordinatorPageFixtureData {
  coordinatorId: string;
  conversation: ReactNode;
}

export function CoordinatorPage({ fixtureData }: { fixtureData?: CoordinatorPageFixtureData }) {
  const navigate = useNavigate();
  const { slug } = useParams<{ slug: string }>();
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(!fixtureData);
  const [resolvedCoordinatorId, setResolvedCoordinatorId] = useState<string | null>(fixtureData?.coordinatorId ?? null);

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
    return () => { cancelled = true; };
  }, [fixtureData, navigate, slug]);

  return (
    <main className="coordinator-page">
      {error && <div className="coordinator-error coordinator-page-status">{error}</div>}
      {loading ? <div className="coordinator-muted coordinator-page-status">Loading…</div> : null}

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
