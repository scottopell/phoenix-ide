import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import type { Conversation } from '../api';
import { formatRelativeTime } from '../utils';
import { ThemeToggle } from './ThemeToggle';
import { useTheme } from '../hooks/useTheme';

interface ConversationListProps {
  conversations: Conversation[];
  archivedConversations: Conversation[];
  showArchived: boolean;
  onToggleArchived: () => void;
  onNewConversation: () => void;
  onArchive: (conv: Conversation) => void;
  onUnarchive: (conv: Conversation) => void;
  onDelete: (conv: Conversation) => void;
  onRename: (conv: Conversation) => void;
}

export function ConversationList({
  conversations,
  archivedConversations,
  showArchived,
  onToggleArchived,
  onNewConversation,
  onArchive,
  onUnarchive,
  onDelete,
  onRename,
}: ConversationListProps) {
  const navigate = useNavigate();
  const { theme, toggleTheme } = useTheme();
  const [expandedId, setExpandedId] = useState<string | null>(null);

  const handleClick = (conv: Conversation) => {
    navigate(`/c/${conv.slug}`);
  };

  const toggleActions = (e: React.MouseEvent, convId: string) => {
    e.stopPropagation();
    setExpandedId(expandedId === convId ? null : convId);
  };

  const displayList = showArchived ? archivedConversations : conversations;

  return (
    <section id="conversation-list" className="view active">
      <div className="view-header">
        <h2>Conversations</h2>
        <div className="view-header-actions">
          <ThemeToggle theme={theme} onToggle={toggleTheme} />
          <button
            className={`btn-icon archive-toggle ${showArchived ? 'active' : ''}`}
            onClick={onToggleArchived}
            title={showArchived ? 'Show active' : 'Show archived'}
          >
            📦
            {archivedConversations.length > 0 && (
              <span className="badge">{archivedConversations.length}</span>
            )}
          </button>
          <button id="new-conv-btn" className="btn-primary" onClick={onNewConversation}>
            + New
          </button>
        </div>
      </div>
      {showArchived && (
        <div className="archive-banner">
          <span>📦 Archived Conversations</span>
          <button className="btn-link" onClick={onToggleArchived}>← Back to active</button>
        </div>
      )}
      <ul id="conv-list">
        {displayList.length === 0 ? (
          <li className="empty-state">
            <div className="empty-state-icon">{showArchived ? '📦' : '💬'}</div>
            <p>{showArchived ? 'No archived conversations' : 'No conversations yet'}</p>
          </li>
        ) : (
          displayList.map((conv) => (
            <li
              key={conv.id}
              className={`conv-item ${expandedId === conv.id ? 'expanded' : ''}`}
              data-id={conv.id}
            >
              <div className="conv-item-main" onClick={() => handleClick(conv)}>
                <div className="conv-item-slug">{conv.slug}</div>
                <div className="conv-item-meta">
                  <span>{formatRelativeTime(conv.updated_at)}</span>
                  <span className="conv-item-model">{conv.model}</span>
                  <span className="conv-item-cwd">{conv.cwd}</span>
                </div>
              </div>
              <button
                className="conv-item-menu-btn"
                onClick={(e) => toggleActions(e, conv.id)}
                title="Actions"
              >
                ⋮
              </button>
              {expandedId === conv.id && (
                <div className="conv-item-actions">
                  <button
                    className="action-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      setExpandedId(null);
                      onRename(conv);
                    }}
                  >
                    ✏️ Rename
                  </button>
                  {showArchived ? (
                    <button
                      className="action-btn"
                      onClick={(e) => {
                        e.stopPropagation();
                        setExpandedId(null);
                        onUnarchive(conv);
                      }}
                    >
                      📤 Unarchive
                    </button>
                  ) : (
                    <button
                      className="action-btn"
                      onClick={(e) => {
                        e.stopPropagation();
                        setExpandedId(null);
                        onArchive(conv);
                      }}
                    >
                      📦 Archive
                    </button>
                  )}
                  <button
                    className="action-btn danger"
                    onClick={(e) => {
                      e.stopPropagation();
                      setExpandedId(null);
                      onDelete(conv);
                    }}
                  >
                    🗑️ Delete
                  </button>
                </div>
              )}
            </li>
          ))
        )}
      </ul>
    </section>
  );
}
