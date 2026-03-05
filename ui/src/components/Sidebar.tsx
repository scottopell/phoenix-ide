import { useState, useCallback, useEffect } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import type { Conversation } from '../api';
import { ConversationList } from './ConversationList';
import { SidebarNewForm } from './SidebarNewForm';
import { ConfirmDialog } from './ConfirmDialog';
import { RenameDialog } from './RenameDialog';
import { api } from '../api';

interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
  conversations: Conversation[];
  archivedConversations: Conversation[];
  activeSlug: string | null;
  onConversationCreated: () => void;
  showToast: (message: string, duration?: number) => void;
}

export function Sidebar({
  collapsed,
  onToggle,
  conversations,
  archivedConversations,
  activeSlug,
  onConversationCreated,
  showToast,
}: SidebarProps) {
  const navigate = useNavigate();
  const location = useLocation();
  const [showNewForm, setShowNewForm] = useState(false);
  const [showArchived, setShowArchived] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Conversation | null>(null);
  const [renameTarget, setRenameTarget] = useState<Conversation | null>(null);
  const [renameError, setRenameError] = useState<string | undefined>();

  // Close inline form when navigating to root (where full form is visible)
  useEffect(() => {
    if (location.pathname === '/' || location.pathname === '/new') {
      setShowNewForm(false);
    }
  }, [location.pathname]);

  const handleNewClick = useCallback(() => {
    if (location.pathname === '/' || location.pathname === '/new') {
      return; // No-op, already on new conversation
    }
    setShowNewForm(true);
  }, [location.pathname]);

  const handleConversationClick = useCallback((conv: Conversation) => {
    setShowNewForm(false);
    navigate(`/c/${conv.slug}`);
  }, [navigate]);

  const handleFormCreated = useCallback(() => {
    setShowNewForm(false);
    onConversationCreated();
  }, [onConversationCreated]);

  const handleArchive = useCallback(async (conv: Conversation) => {
    try {
      await api.archiveConversation(conv.id);
      onConversationCreated();
    } catch (err) {
      console.error('Failed to archive:', err);
    }
  }, [onConversationCreated]);

  const handleUnarchive = useCallback(async (conv: Conversation) => {
    try {
      await api.unarchiveConversation(conv.id);
      onConversationCreated();
    } catch (err) {
      console.error('Failed to unarchive:', err);
    }
  }, [onConversationCreated]);

  const handleDelete = useCallback(async () => {
    if (!deleteTarget) return;
    try {
      await api.deleteConversation(deleteTarget.id);
      setDeleteTarget(null);
      onConversationCreated();
    } catch (err) {
      console.error('Failed to delete:', err);
    }
  }, [deleteTarget, onConversationCreated]);

  const handleRename = useCallback(async (newName: string) => {
    if (!renameTarget) return;
    try {
      await api.renameConversation(renameTarget.id, newName);
      setRenameTarget(null);
      setRenameError(undefined);
      onConversationCreated();
    } catch (err) {
      setRenameError(err instanceof Error ? err.message : 'Failed to rename');
    }
  }, [renameTarget, onConversationCreated]);

  const isOnNewPage = location.pathname === '/' || location.pathname === '/new';

  if (collapsed) {
    return (
      <aside className="sidebar sidebar-collapsed">
        <button className="sidebar-icon-btn sidebar-toggle" onClick={onToggle} title="Expand sidebar">
          ▶
        </button>
        <button className="sidebar-icon-btn" onClick={() => navigate('/')} title="Phoenix">
          <img src="/phoenix.svg" alt="Phoenix" className="sidebar-logo-icon" />
        </button>
        <button
          className={`sidebar-icon-btn sidebar-new-btn ${isOnNewPage ? 'disabled' : ''}`}
          onClick={handleNewClick}
          title="New conversation"
        >
          +
        </button>
        <div className="sidebar-collapsed-dots">
          {conversations.slice(0, 15).map(conv => {
            const displayState = conv.display_state || 'idle';
            const isActive = conv.slug === activeSlug;
            return (
              <button
                key={conv.id}
                className={`sidebar-dot-btn ${isActive ? 'active' : ''}`}
                onClick={() => handleConversationClick(conv)}
                title={conv.slug}
              >
                <span className={`conv-state-dot ${displayState}`} />
              </button>
            );
          })}
        </div>
      </aside>
    );
  }

  return (
    <aside className="sidebar sidebar-expanded">
      <div className="sidebar-header">
        <button className="sidebar-toggle-expanded" onClick={onToggle} title="Collapse sidebar">
          ◀
        </button>
        <button className="sidebar-brand" onClick={() => navigate('/')}>
          <img src="/phoenix.svg" alt="Phoenix" className="sidebar-logo" />
          <span className="sidebar-brand-text">Phoenix</span>
        </button>
        <button
          className={`btn-primary sidebar-new-btn ${isOnNewPage ? 'disabled' : ''}`}
          onClick={handleNewClick}
        >
          + New
        </button>
      </div>
      {showNewForm && (
        <SidebarNewForm
          onClose={() => setShowNewForm(false)}
          onCreated={handleFormCreated}
          showToast={showToast}
        />
      )}
      <div className="sidebar-list">
        <ConversationList
          conversations={conversations}
          archivedConversations={archivedConversations}
          showArchived={showArchived}
          onToggleArchived={() => setShowArchived(!showArchived)}
          onNewConversation={handleNewClick}
          onArchive={handleArchive}
          onUnarchive={handleUnarchive}
          onDelete={(conv) => setDeleteTarget(conv)}
          onRename={(conv) => { setRenameError(undefined); setRenameTarget(conv); }}
          onConversationClick={handleConversationClick}
          activeSlug={activeSlug}
          sidebarMode
        />
      </div>
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
        error={renameError ?? undefined}
        onRename={handleRename}
        onCancel={() => { setRenameTarget(null); setRenameError(undefined); }}
      />
    </aside>
  );
}
