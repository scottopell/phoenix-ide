import { useState, useEffect, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { enhancedApi } from '../enhancedApi';
import type { Conversation } from '../api';
import { ConversationList } from '../components/ConversationList';
import { NewConversationModal } from '../components/NewConversationModal';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { RenameDialog } from '../components/RenameDialog';
import { useAppMachine } from '../hooks/useAppMachine';

export function ConversationListPage() {
  const navigate = useNavigate();
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [archivedConversations, setArchivedConversations] = useState<Conversation[]>([]);
  const [showArchived, setShowArchived] = useState(false);
  const [showModal, setShowModal] = useState(false);
  const [loading, setLoading] = useState(true);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);
  const scrollRestoredRef = useRef(false);

  // App state for offline/sync status
  const { isOnline, isReady, showSyncStatus, syncProgress, pendingOpsCount } = useAppMachine();

  // Delete confirmation state
  const [deleteTarget, setDeleteTarget] = useState<Conversation | null>(null);

  // Rename state
  const [renameTarget, setRenameTarget] = useState<Conversation | null>(null);
  const [renameError, setRenameError] = useState<string | undefined>();

  const loadConversations = useCallback(async (forceFresh = false) => {
    try {
      const [activeResult, archivedResult] = await Promise.all([
        enhancedApi.listConversations({ forceFresh }),
        enhancedApi.listArchivedConversations({ forceFresh }),
      ]);
      
      setConversations(activeResult.data);
      setArchivedConversations(archivedResult.data);
      
      // Update last updated time based on freshest data
      if (activeResult.source === 'network' || archivedResult.source === 'network') {
        setLastUpdated(new Date());
      }
      
      // Log cache hit/miss for debugging
      console.log(`Loaded conversations - Active: ${activeResult.source} (stale: ${activeResult.stale}), Archived: ${archivedResult.source} (stale: ${archivedResult.stale})`);
    } catch (err) {
      console.error('Failed to load conversations:', err);
      // Even on error, try to show cached data
      const [activeCached, archivedCached] = await Promise.all([
        enhancedApi.listConversations({ forceFresh: false }),
        enhancedApi.listArchivedConversations({ forceFresh: false }),
      ]);
      if (activeCached.data.length > 0 || archivedCached.data.length > 0) {
        setConversations(activeCached.data);
        setArchivedConversations(archivedCached.data);
      }
    } finally {
      setLoading(false);
    }
  }, []);

  // Initial load - use cached data for instant display
  useEffect(() => {
    if (isReady) {
      loadConversations(false);
    }
  }, [loadConversations, isReady]);

  // Restore scroll position after data loads
  useEffect(() => {
    if (!loading && !scrollRestoredRef.current && conversations.length > 0) {
      // Get scroll position from cache
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

  const handleCreated = (conv: { id: string; slug: string }) => {
    setShowModal(false);
    loadConversations(true); // Force fresh to include new conversation
    navigate(`/c/${conv.slug}`);
  };

  const handleArchive = async (conv: Conversation) => {
    try {
      if (isOnline) {
        await enhancedApi.archiveConversation(conv.id);
      } else {
        // Queue for later
        const { queueOperation } = useAppMachine();
        await queueOperation({
          type: 'archive',
          conversationId: conv.id,
          payload: {},
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending'
        });
      }
      await loadConversations(true);
    } catch (err) {
      console.error('Failed to archive:', err);
    }
  };

  const handleUnarchive = async (conv: Conversation) => {
    try {
      if (isOnline) {
        await enhancedApi.unarchiveConversation(conv.id);
      } else {
        // Queue for later
        const { queueOperation } = useAppMachine();
        await queueOperation({
          type: 'unarchive',
          conversationId: conv.id,
          payload: {},
          createdAt: new Date(),
          retryCount: 0,
          status: 'pending'
        });
      }
      await loadConversations(true);
    } catch (err) {
      console.error('Failed to unarchive:', err);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    try {
      await enhancedApi.deleteConversation(deleteTarget.id);
      setDeleteTarget(null);
      await loadConversations(true);
    } catch (err) {
      console.error('Failed to delete:', err);
    }
  };

  const handleRename = async (newName: string) => {
    if (!renameTarget) return;
    try {
      await enhancedApi.renameConversation(renameTarget.id, newName);
      setRenameTarget(null);
      setRenameError(undefined);
      await loadConversations(true);
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

  return (
    <div id="app" className="list-page">
      <header className="status-header">
        {!isOnline && (
          <div className="offline-banner">
            <span className="offline-icon">⚡</span>
            Offline Mode
            {pendingOpsCount > 0 && ` (${pendingOpsCount} pending)`}
          </div>
        )}
        {showSyncStatus && syncProgress !== null && (
          <div className="sync-banner">
            <span className="sync-icon">🔄</span>
            Syncing... {syncProgress}%
          </div>
        )}
      </header>
      <main id="main-area">
        {loading ? (
          <div className="empty-state">
            <div className="spinner"></div>
            <p>Loading...</p>
          </div>
        ) : (
          <>
            {lastUpdated && (
              <div className="last-updated">
                {getLastUpdatedText()}
                <button 
                  className="refresh-btn"
                  onClick={() => loadConversations(true)}
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
              onNewConversation={() => setShowModal(true)}
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
      <NewConversationModal
        visible={showModal}
        onClose={() => setShowModal(false)}
        onCreated={handleCreated}
      />
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
