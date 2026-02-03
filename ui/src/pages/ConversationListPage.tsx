import { useState, useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, Conversation } from '../api';
import { ConversationList } from '../components/ConversationList';
import { NewConversationModal } from '../components/NewConversationModal';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { RenameDialog } from '../components/RenameDialog';

export function ConversationListPage() {
  const navigate = useNavigate();
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [archivedConversations, setArchivedConversations] = useState<Conversation[]>([]);
  const [showArchived, setShowArchived] = useState(false);
  const [showModal, setShowModal] = useState(false);
  const [loading, setLoading] = useState(true);

  // Delete confirmation state
  const [deleteTarget, setDeleteTarget] = useState<Conversation | null>(null);

  // Rename state
  const [renameTarget, setRenameTarget] = useState<Conversation | null>(null);
  const [renameError, setRenameError] = useState<string | undefined>();

  const loadConversations = useCallback(async () => {
    try {
      const [convs, archived] = await Promise.all([
        api.listConversations(),
        api.listArchivedConversations(),
      ]);
      setConversations(convs);
      setArchivedConversations(archived);
    } catch (err) {
      console.error('Failed to load conversations:', err);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadConversations();
  }, [loadConversations]);

  const handleCreated = (conv: { id: string; slug: string }) => {
    setShowModal(false);
    loadConversations();
    navigate(`/c/${conv.slug}`);
  };

  const handleArchive = async (conv: Conversation) => {
    try {
      await api.archiveConversation(conv.id);
      await loadConversations();
    } catch (err) {
      console.error('Failed to archive:', err);
    }
  };

  const handleUnarchive = async (conv: Conversation) => {
    try {
      await api.unarchiveConversation(conv.id);
      await loadConversations();
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

  return (
    <div id="app" className="list-page">
      <main id="main-area">
        {loading ? (
          <div className="empty-state">
            <div className="spinner"></div>
            <p>Loading...</p>
          </div>
        ) : (
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
          />
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
