import { useState, useEffect, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api';
import type { Conversation } from '../api';
import { cacheDB } from '../cache';
import { ConversationList } from '../components/ConversationList';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { RenameDialog } from '../components/RenameDialog';
import { StorageStatus } from '../components/StorageStatus';
import { Toast } from '../components/Toast';
import { ConversationListSkeleton } from '../components/Skeleton';
import { useAppMachine } from '../hooks/useAppMachine';
import { useToast } from '../hooks/useToast';

export function ConversationListPage() {
  const navigate = useNavigate();
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [archivedConversations, setArchivedConversations] = useState<Conversation[]>([]);
  const [showArchived, setShowArchived] = useState(false);

  const [loading, setLoading] = useState(true);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);
  const scrollRestoredRef = useRef(false);

  // App state for offline/sync status
  const { isOnline, isReady, initError, pendingOpsCount, queueOperation } = useAppMachine();
  const { toasts, dismissToast, showWarning, showError } = useToast();

  // Delete confirmation state
  const [deleteTarget, setDeleteTarget] = useState<Conversation | null>(null);

  // Rename state
  const [renameTarget, setRenameTarget] = useState<Conversation | null>(null);
  const [renameError, setRenameError] = useState<string | undefined>();

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

  // Load conversations: cache first, then network
  const loadConversations = useCallback(async () => {
    try {
      // Step 1: Show cached data immediately
      const cached = await cacheDB.getAllConversations();
      const cachedActive = cached.filter(c => !c.archived);
      const cachedArchived = cached.filter(c => c.archived);
      if (cachedActive.length > 0 || cachedArchived.length > 0) {
        setConversations(cachedActive);
        setArchivedConversations(cachedArchived);
        setLoading(false);
      }

      // Step 2: Fetch fresh if online
      if (navigator.onLine) {
        try {
          const [freshActive, freshArchived] = await Promise.all([
            api.listConversations(),
            api.listArchivedConversations()
          ]);
          setConversations(freshActive);
          setArchivedConversations(freshArchived);
          setLastUpdated(new Date());
          
          // Update cache
          await cacheDB.putConversations([...freshActive, ...freshArchived]);
        } catch (err) {
          console.error('Failed to fetch fresh conversations:', err);
          // Network failed, cached data still showing (if any)
          if (cachedActive.length === 0 && cachedArchived.length === 0) {
            showError('Failed to load conversations. Please check your connection.', 5000);
          }
        }
      }
    } catch (err) {
      console.error('Failed to load conversations:', err);
      showError('Failed to load conversations.', 5000);
    } finally {
      setLoading(false);
    }
  }, [showError]);

  // Initial load when cache is ready
  useEffect(() => {
    if (isReady) {
      loadConversations();
    }
  }, [isReady, loadConversations]);

  // Restore scroll position after data loads
  useEffect(() => {
    if (!loading && !scrollRestoredRef.current && conversations.length > 0) {
      const savedPosition = sessionStorage.getItem('conversationListScrollPosition');
      if (savedPosition) {
        window.scrollTo(0, parseInt(savedPosition, 10));
        sessionStorage.removeItem('conversationListScrollPosition');
      }
      scrollRestoredRef.current = true;
    }
  }, [loading, conversations]);

  // Save scroll position before navigating away
  const handleConversationClick = useCallback((conv: Conversation) => {
    sessionStorage.setItem('conversationListScrollPosition', window.scrollY.toString());
    navigate(`/c/${conv.slug}`);
  }, [navigate]);

  const handleNewConversation = () => {
    navigate('/new');
  };

  const handleArchive = async (conv: Conversation) => {
    try {
      if (isOnline) {
        await api.archiveConversation(conv.id);
        await loadConversations();
      } else {
        await queueOperation({
          type: 'archive',
          conversationId: conv.id,
          payload: {},
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending'
        });
        // Optimistically update UI
        setConversations(prev => prev.filter(c => c.id !== conv.id));
        setArchivedConversations(prev => [...prev, { ...conv, archived: true }]);
      }
    } catch (err) {
      console.error('Failed to archive:', err);
    }
  };

  const handleUnarchive = async (conv: Conversation) => {
    try {
      if (isOnline) {
        await api.unarchiveConversation(conv.id);
        await loadConversations();
      } else {
        await queueOperation({
          type: 'unarchive',
          conversationId: conv.id,
          payload: {},
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending'
        });
        // Optimistically update UI
        setArchivedConversations(prev => prev.filter(c => c.id !== conv.id));
        setConversations(prev => [...prev, { ...conv, archived: false }]);
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
      await loadConversations();
    } catch (err) {
      console.error('Failed to delete:', err);
    }
  };

  const handleRename = async (newName: string) => {
    if (!renameTarget) return;
    try {
      await api.renameConversation(renameTarget.id, newName);
      setRenameTarget(null);
      setRenameError(undefined);
      await loadConversations();
    } catch (err) {
      setRenameError(err instanceof Error ? err.message : 'Failed to rename');
    }
  };

  // Format last updated time
  const getLastUpdatedText = () => {
    if (!lastUpdated) return null;
    const minutes = Math.floor((Date.now() - lastUpdated.getTime()) / 60000);
    if (minutes < 1) return 'Updated just now';
    if (minutes === 1) return 'Updated 1 minute ago';
    if (minutes < 60) return `Updated ${minutes} minutes ago`;
    const hours = Math.floor(minutes / 60);
    if (hours === 1) return 'Updated 1 hour ago';
    return `Updated ${hours} hours ago`;
  };

  // Show error UI if IndexedDB init failed
  if (initError) {
    return (
      <div id="app" className="list-page">
        <div className="init-error">
          <h2>⚠️ Storage Error</h2>
          <p>Failed to initialize local storage: {initError}</p>
          <p>Please try refreshing the page. If the problem persists, try clearing your browser data for this site.</p>
          <button onClick={() => window.location.reload()}>Refresh Page</button>
        </div>
      </div>
    );
  }

  return (
    <div id="app" className="list-page">
      <Toast messages={toasts} onDismiss={dismissToast} />
      <header className="status-header">
        <div className="header-left">
          {!isOnline && (
            <div className="offline-banner">
              <span className="offline-icon">⚡</span>
              Offline Mode
              {pendingOpsCount > 0 && ` (${pendingOpsCount} pending)`}
            </div>
          )}
        </div>
        <div className="header-right">
          <StorageStatus />
        </div>
      </header>
      <main id="main-area">
        {loading ? (
          <section id="conversation-list" className="view active">
            <div className="view-header">
              <h2>Conversations</h2>
              <div className="view-header-actions">
                <button className="btn-primary" disabled>+ New</button>
              </div>
            </div>
            <ConversationListSkeleton count={5} />
          </section>
        ) : (
          <>
            {lastUpdated && (
              <div className="last-updated">
                {getLastUpdatedText()}
                <button 
                  className="refresh-btn"
                  onClick={() => loadConversations()}
                  disabled={!isOnline}
                >
                  ↻
                </button>
              </div>
            )}
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
              onConversationClick={handleConversationClick}
            />
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
      <RenameDialog
        visible={renameTarget !== null}
        currentName={renameTarget?.slug ?? ''}
        error={renameError}
        onRename={handleRename}
        onCancel={() => {
          setRenameTarget(null);
          setRenameError(undefined);
        }}
      />
    </div>
  );
}
