import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import type { Conversation } from '../api';
import { formatRelativeTime, formatShortDateTime } from '../utils';

import { useKeyboardNav } from '../hooks';

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
  onConversationClick?: (conv: Conversation) => void;
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
  onConversationClick,
}: ConversationListProps) {
  const navigate = useNavigate();
  const [expandedId, setExpandedId] = useState<string | null>(null);

  const displayList = showArchived ? archivedConversations : conversations;

  // Keyboard navigation
  const { selectedId } = useKeyboardNav({
    items: displayList,
    onNew: onNewConversation,
  });

  const handleClick = (conv: Conversation) => {
    if (onConversationClick) {
      onConversationClick(conv);
    } else {
      navigate(`/c/${conv.slug}`);
    }
  };

  const toggleActions = (e: React.MouseEvent, convId: string) => {
    e.stopPropagation();
    setExpandedId(expandedId === convId ? null : convId);
  };

  return (
    <section id="conversation-list" className="view active">
      <div className="view-header">
        <h2>Conversations</h2>
        <div className="view-header-actions">
          {archivedConversations.length > 0 && (
            <button
              className={`btn-secondary archive-toggle ${showArchived ? 'active' : ''}`}
              onClick={onToggleArchived}
            >
              {showArchived ? 'Active' : `Archived (${archivedConversations.length})`}
            </button>
          )}
          <button id="new-conv-btn" className="btn-primary" onClick={onNewConversation}>
            + New
          </button>
        </div>
      </div>

      <ul id="conv-list">
        {displayList.length === 0 ? (
          <li className="empty-state">
            <p>{showArchived ? 'No archived conversations' : 'No conversations yet'}</p>
          </li>
        ) : (
          displayList.map((conv) => (
            <li
              key={conv.id}
              className={`conv-item ${expandedId === conv.id ? 'expanded' : ''} ${selectedId === conv.id ? 'keyboard-selected' : ''}`}
              data-id={conv.id}
            >
              <div className="conv-item-main" onClick={() => handleClick(conv)}>
                <div className="conv-item-slug">{conv.slug}</div>
                <div className="conv-item-meta">
                  <span className="conv-item-time" title={`Created: ${formatShortDateTime(conv.created_at)}\nLast activity: ${formatRelativeTime(conv.updated_at)}`}>
                    {formatShortDateTime(conv.created_at)} → {formatRelativeTime(conv.updated_at)}
                  </span>
                  <span className="conv-item-messages">{conv.message_count} {conv.message_count === 1 ? 'msg' : 'msgs'}</span>
                </div>
                <div className="conv-item-meta secondary">
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
                    Rename
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
                      Restore
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
                      Archive
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
                    Delete
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
