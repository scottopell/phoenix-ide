import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import type { Conversation, ConversationState } from '../api';
import { formatRelativeTime, formatShortDateTime } from '../utils';

import { useKeyboardNav } from '../hooks';

function getConvDisplayState(state?: ConversationState): 'idle' | 'working' | 'error' {
  if (!state || !state.type) return 'idle';
  const t = state.type;
  if (t === 'idle' || t === 'context_exhausted' || t === 'completed' || t === 'failed') return 'idle';
  if (t === 'error') return 'error';
  return 'working';
}

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
  activeSlug?: string | null;
  sidebarMode?: boolean;
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
  activeSlug,
  sidebarMode,
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
    <section id="conversation-list" className={`view active ${sidebarMode ? 'sidebar-mode' : ''}`}>
      {!sidebarMode && (
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
      )}
      {sidebarMode && archivedConversations.length > 0 && (
        <div className="sidebar-archive-toggle">
          <button
            className={`btn-secondary archive-toggle ${showArchived ? 'active' : ''}`}
            onClick={onToggleArchived}
          >
            {showArchived ? 'Active' : `Archived (${archivedConversations.length})`}
          </button>
        </div>
      )}

      <ul id="conv-list">
        {displayList.length === 0 ? (
          <li className="empty-state">
            <p>{showArchived ? 'No archived conversations' : 'No conversations yet'}</p>
          </li>
        ) : (
          displayList.map((conv) => (
            <li
              key={conv.id}
              className={`conv-item ${expandedId === conv.id ? 'expanded' : ''} ${selectedId === conv.id ? 'keyboard-selected' : ''} ${activeSlug && conv.slug === activeSlug ? 'active' : ''}`}
              data-id={conv.id}
            >
              <div className="conv-item-main" onClick={() => handleClick(conv)}>
                <div className="conv-item-slug">
                  <span className={`conv-state-dot ${getConvDisplayState(conv.state)}`} />
                  {conv.slug}
                </div>
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
