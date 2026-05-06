import { useState, useEffect, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import { refreshModels } from '../modelsPoller';
import type { ChainView, Conversation } from '../api';
import { useModels, useAutoAuth } from '../hooks';
import {
  useConversationsList,
  useConversationsRefresh,
} from '../conversation';
import { NewConversationPage } from './NewConversationPage';
import { ConversationList } from '../components/ConversationList';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { ChainDeleteConfirm } from '../components/ChainDeleteConfirm';
import { RenameDialog } from '../components/RenameDialog';
import { StorageStatus } from '../components/StorageStatus';

const AlertTriangle = () => (
  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" style={{ verticalAlign: '-4px', marginRight: '8px' }}>
    <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
    <line x1="12" y1="9" x2="12" y2="13" />
    <line x1="12" y1="17" x2="12.01" y2="17" />
  </svg>
);
import { Toast } from '../components/Toast';
import { ConversationListSkeleton } from '../components/Skeleton';
import { computeChainRoots } from '../utils/chains';
import { useAppMachine } from '../hooks/useAppMachine';
import { useToast } from '../hooks/useToast';
import { CredentialHelperPanel } from '../components/CredentialHelperPanel';

export function ConversationListPage() {
  const navigate = useNavigate();
  const [isDesktop, setIsDesktop] = useState(() => window.matchMedia('(min-width: 1025px)').matches);

  // Task 08684: ConversationStore is the single source of truth. The
  // shared `useConversationsRefresh` (mounted in ConversationProvider)
  // owns the cache hydration, the 5s poll, and the cache writeback. This
  // page reads the derived list straight off the store — no parallel
  // useState arrays, no per-page polling timer.
  const { refresh } = useConversationsRefresh();
  const { active: conversations, archived: archivedConversations } = useConversationsList();
  const [showArchived, setShowArchived] = useState(false);

  const [refreshing, setRefreshing] = useState(false);
  const scrollRestoredRef = useRef(false);
  const pullStartY = useRef<number | null>(null);
  const mainRef = useRef<HTMLElement>(null);

  // App state for offline/sync status
  const { isOnline, isReady, initError, pendingOpsCount, queueOperation } = useAppMachine();
  const { toasts, dismissToast, showWarning, showError } = useToast();

  // Loading is derived: we're loading until we have at least one conversation
  // observed *or* the cache hydration + first poll have completed (signalled
  // by `isReady` being true and the list being populated, OR isReady true
  // with no rows server-side which is also a valid empty state).
  // Concretely: hide the skeleton as soon as we have any rows, OR when
  // we've completed at least one refresh while online.
  const [hasCompletedFirstFetch, setHasCompletedFirstFetch] = useState(false);
  useEffect(() => {
    if (!isReady) return;
    let cancelled = false;
    void refresh().then(() => {
      if (!cancelled) setHasCompletedFirstFetch(true);
    });
    return () => {
      cancelled = true;
    };
  }, [isReady, refresh]);
  const loading =
    !hasCompletedFirstFetch &&
    conversations.length === 0 &&
    archivedConversations.length === 0;

  // Delete confirmation state
  const [deleteTarget, setDeleteTarget] = useState<Conversation | null>(null);
  // Chain delete confirmation state. We fetch the full ChainView when a
  // user invokes "Delete chain" so the confirm dialog can show member count
  // + worktree count without forcing every list query to carry the chain
  // detail.
  const [deleteChainTarget, setDeleteChainTarget] = useState<ChainView | null>(null);

  // Rename state
  const [renameTarget, setRenameTarget] = useState<Conversation | null>(null);
  const [renameError, setRenameError] = useState<string | undefined>();

  const { credentialStatus } = useModels();
  const { showAuthPanel, setShowAuthPanel } = useAutoAuth(credentialStatus);


  // Listen for storage warnings
  useEffect(() => {
    const handleStorageWarning = (event: Event) => {
      const customEvent = event as CustomEvent;
      const { usageMB } = customEvent.detail;
      showWarning(`Storage usage is high: ${usageMB.toFixed(1)}MB. Consider clearing old data.`, 10000);
    };

    const handleQuotaExceeded = () => {
      showError('Storage quota exceeded! Old conversations are being cleaned up automatically.', 8000);
    };

    window.addEventListener('storage-warning', handleStorageWarning);
    window.addEventListener('storage-quota-exceeded', handleQuotaExceeded);
    return () => {
      window.removeEventListener('storage-warning', handleStorageWarning);
      window.removeEventListener('storage-quota-exceeded', handleQuotaExceeded);
    };
  }, [showWarning, showError]);

  // Removed: per-page loadConversations + periodic refresh. The shared
  // `useConversationsRefresh` (mounted in ConversationProvider) owns the
  // cache hydration, polling, online listener, and hard-delete cascade.
  // This page calls `refresh()` after mutations that need an immediate
  // resync, but never holds its own conversation arrays.

  // Restore scroll position after data loads
  useEffect(() => {
    if (!loading && !scrollRestoredRef.current && conversations.length > 0) {
      const savedPosition = sessionStorage.getItem('conversationListScrollPosition');
      if (savedPosition && mainRef.current) {
        const target = parseInt(savedPosition, 10);
        // Use rAF to ensure the list items are painted before scrolling
        requestAnimationFrame(() => {
          if (mainRef.current) {
            mainRef.current.scrollTop = target;
          }
        });
        sessionStorage.removeItem('conversationListScrollPosition');
      }
      scrollRestoredRef.current = true;
    }
  }, [loading, conversations]);

  // Save scroll position before navigating away
  const handleConversationClick = useCallback((conv: Conversation) => {
    if (mainRef.current) {
      sessionStorage.setItem('conversationListScrollPosition', mainRef.current.scrollTop.toString());
    }
    navigate(`/c/${conv.slug}`);
  }, [navigate]);

  const handleNewConversation = () => {
    navigate('/new');
  };

  const handleArchive = async (conv: Conversation) => {
    try {
      if (isOnline) {
        await api.archiveConversation(conv.id);
        await refresh();
      } else {
        await queueOperation({
          type: 'archive',
          conversationId: conv.id,
          payload: {},
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending'
        });
        // Offline optimistic: the operation is queued, but the row in
        // the store is still the pre-archive shape until the queue
        // drains and the next `refresh()` picks up the server-side
        // change. The UI will show the conversation as still-active
        // until then; the offline indicator already conveys that the
        // queue is pending. If we want eager-flip-on-queue we'd dispatch
        // a `local_conversation_update` against the atom — deferred
        // because the local mutation would then desync from anything
        // SSE eventually sends.
      }
    } catch (err) {
      console.error('Failed to archive:', err);
    }
  };

  const handleUnarchive = async (conv: Conversation) => {
    try {
      if (isOnline) {
        await api.unarchiveConversation(conv.id);
        await refresh();
      } else {
        await queueOperation({
          type: 'unarchive',
          conversationId: conv.id,
          payload: {},
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending'
        });
      }
    } catch (err) {
      console.error('Failed to unarchive:', err);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    try {
      await api.deleteConversation(deleteTarget.id);
      setDeleteTarget(null);
      await refresh();
    } catch (err) {
      console.error('Failed to delete:', err);
    }
  };

  /** Whether `conv` is part of the chain rooted at `rootId`, given the
   *  population of conversations to consider. Walks chain pointers via the
   *  shared `computeChainRoots` helper so the rule matches the sidebar's
   *  grouping. */
  const isMemberOfChain = (
    conv: Conversation,
    rootId: string,
    all: readonly Conversation[],
  ): boolean => {
    const roots = computeChainRoots(all);
    return roots.get(conv.id) === rootId;
  };

  const handleArchiveChain = async (rootId: string) => {
    try {
      if (isOnline) {
        await api.archiveChain(rootId);
        await refresh();
      } else {
        await queueOperation({
          type: 'archive_chain',
          conversationId: rootId,
          payload: {},
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending',
        });
        // Offline path: the queued op will fire on reconnect; refresh()
        // then reconciles. (Pre-08684 the page locally moved every chain
        // member between the two arrays — with the store-as-source-of-
        // truth, the same store-level mutation would need to be done as
        // a series of local_conversation_updates against each member's
        // atom. Deferred until we have a concrete need; the offline
        // indicator already signals the queue.)
        void isMemberOfChain;
      }
    } catch (err) {
      console.error('Failed to archive chain:', err);
      showError(err instanceof Error ? err.message : 'Failed to archive chain', 5000);
    }
  };

  const handleUnarchiveChain = async (rootId: string) => {
    try {
      if (isOnline) {
        await api.unarchiveChain(rootId);
        await refresh();
      } else {
        await queueOperation({
          type: 'unarchive_chain',
          conversationId: rootId,
          payload: {},
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending',
        });
      }
    } catch (err) {
      console.error('Failed to unarchive chain:', err);
      showError(err instanceof Error ? err.message : 'Failed to unarchive chain', 5000);
    }
  };

  const requestDeleteChain = async (rootId: string) => {
    try {
      const view = await api.getChain(rootId);
      setDeleteChainTarget(view);
    } catch (err) {
      console.error('Failed to load chain for delete:', err);
      showError(err instanceof Error ? err.message : 'Failed to load chain', 5000);
    }
  };

  const handleDeleteChain = async () => {
    if (!deleteChainTarget) return;
    try {
      await api.deleteChain(deleteChainTarget.root_conv_id);
      setDeleteChainTarget(null);
      await refresh();
    } catch (err) {
      console.error('Failed to delete chain:', err);
      showError(err instanceof Error ? err.message : 'Failed to delete chain', 5000);
    }
  };

  const handleRename = async (newName: string) => {
    if (!renameTarget) return;
    try {
      await api.renameConversation(renameTarget.id, newName);
      setRenameTarget(null);
      setRenameError(undefined);
      await refresh();
    } catch (err) {
      setRenameError(err instanceof Error ? err.message : 'Failed to rename');
    }
  };

  // Pull-to-refresh handlers
  const handleTouchStart = useCallback((e: React.TouchEvent) => {
    const touch = e.touches[0];
    if (window.scrollY === 0 && touch) {
      pullStartY.current = touch.clientY;
    }
  }, []);

  const handleTouchMove = useCallback((e: React.TouchEvent) => {
    if (pullStartY.current === null || refreshing) return;
    const touch = e.touches[0];
    if (!touch) return;
    const pullDistance = touch.clientY - pullStartY.current;
    if (pullDistance > 80 && window.scrollY === 0) {
      pullStartY.current = null;
      setRefreshing(true);
      void refresh().finally(() => setRefreshing(false));
    }
  }, [refreshing, refresh]);

  const handleTouchEnd = useCallback(() => {
    pullStartY.current = null;
  }, []);

  // Desktop media query listener
  useEffect(() => {
    const mq = window.matchMedia('(min-width: 1025px)');
    const handler = (e: MediaQueryListEvent) => setIsDesktop(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  // On desktop, the sidebar handles the conversation list.
  // Root route shows the new conversation form in main content.
  if (isDesktop) {
    return <NewConversationPage desktopMode />;
  }

  // Show error UI if IndexedDB init failed
  if (initError) {
    return (
      <div id="app" className="list-page">
        <div className="init-error">
          <h2><AlertTriangle />Storage Error</h2>
          <p>Failed to initialize local storage: {initError}</p>
          <p>Please try refreshing the page. If the problem persists, try clearing your browser data for this site.</p>
          <button onClick={() => window.location.reload()}>Refresh Page</button>
        </div>
      </div>
    );
  }

  const totalConversations = conversations.length + archivedConversations.length;

  const authChip = credentialStatus && credentialStatus !== 'not_configured' ? (
    <button
      className={`auth-chip ${
        credentialStatus === 'valid' ? 'valid' :
        credentialStatus === 'running' ? 'running' :
        'required'
      }`}
      onClick={credentialStatus === 'required' || credentialStatus === 'failed'
        ? () => setShowAuthPanel(true)
        : undefined}
      disabled={credentialStatus === 'valid' || credentialStatus === 'running'}
    >
      {credentialStatus === 'valid' ? 'AUTH \u2713' :
       credentialStatus === 'running' ? 'AUTH ...' :
       'AUTH \u2717'}
    </button>
  ) : undefined;

  return (
    <div id="app" className="list-page">
      <Toast messages={toasts} onDismiss={dismissToast} />
      {!isOnline && (
        <div className="offline-banner">
          Offline
          {pendingOpsCount > 0 && ` · ${pendingOpsCount} pending`}
        </div>
      )}
      {refreshing && (
        <div className="pull-refresh-indicator">Refreshing...</div>
      )}
      <main 
        id="main-area" 
        ref={mainRef}
        onTouchStart={handleTouchStart}
        onTouchMove={handleTouchMove}
        onTouchEnd={handleTouchEnd}
      >
        {loading ? (
          <section id="conversation-list" className="view active">
            <div className="view-header">
              <h2>Conversations</h2>
              <div className="view-header-actions">
                {authChip}
                <button className="btn-primary" disabled>+ New</button>
              </div>
            </div>
            <ConversationListSkeleton count={5} />
          </section>
        ) : (
          <>
            <ConversationList
              conversations={conversations}
              archivedConversations={archivedConversations}
              showArchived={showArchived}
              onToggleArchived={() => setShowArchived(!showArchived)}
              onNewConversation={handleNewConversation}
              onArchive={handleArchive}
              onUnarchive={handleUnarchive}
              onDelete={(conv) => setDeleteTarget(conv)}
              onRename={(conv) => {
                setRenameError(undefined);
                setRenameTarget(conv);
              }}
              onArchiveChain={handleArchiveChain}
              onUnarchiveChain={handleUnarchiveChain}
              onDeleteChain={requestDeleteChain}
              onConversationClick={handleConversationClick}
              authChip={authChip}
            />
            <StorageStatus conversationCount={totalConversations} />
          </>
        )}
      </main>
      <ConfirmDialog
        visible={deleteTarget !== null}
        title="Delete Conversation"
        message={`Are you sure you want to delete "${deleteTarget?.slug}"? This cannot be undone.`}
        confirmText="Delete"
        danger
        onConfirm={handleDelete}
        onCancel={() => setDeleteTarget(null)}
      />
      <ChainDeleteConfirm
        visible={deleteChainTarget !== null}
        chain={deleteChainTarget}
        onConfirm={handleDeleteChain}
        onCancel={() => setDeleteChainTarget(null)}
      />
      <RenameDialog
        visible={renameTarget !== null}
        currentName={renameTarget?.slug ?? ''}
        error={renameError ?? undefined}
        onRename={handleRename}
        onCancel={() => {
          setRenameTarget(null);
          setRenameError(undefined);
        }}
      />
      {showAuthPanel && credentialStatus && credentialStatus !== 'not_configured' && credentialStatus !== 'valid' && (
        <CredentialHelperPanel
          active={showAuthPanel}
          onDismiss={() => {
            setShowAuthPanel(false);
            void refreshModels().catch(() => {});
          }}
        />
      )}
    </div>
  );
}
