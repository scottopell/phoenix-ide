import { useState, useEffect, useRef, useMemo } from 'react';
import type { ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { getDisplayState } from '../api';
import type { Conversation } from '../api';
import { formatRelativeTime, formatShortDateTime } from '../utils';
import {
  computeChainRoots,
  groupConversationsForSidebar,
  type SidebarItem,
} from '../utils/chains';

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
  activeSlug?: string | null;
  sidebarMode?: boolean;
  authChip?: ReactNode;
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
  authChip,
}: ConversationListProps) {
  const navigate = useNavigate();
  const [expandedId, setExpandedId] = useState<string | null>(null);
  // Per-chain collapse state. NOT persisted across navigations
  // (specs/chains/design.md "Sidebar Grouping"). A chain absent from the
  // map is considered expanded (the default).
  const [collapsedChains, setCollapsedChains] = useState<Set<string>>(new Set());
  const menuRef = useRef<HTMLDivElement>(null);

  // Close context menu on click-outside
  useEffect(() => {
    if (!expandedId) return;
    const handleMouseDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setExpandedId(null);
      }
    };
    document.addEventListener('mousedown', handleMouseDown);
    return () => document.removeEventListener('mousedown', handleMouseDown);
  }, [expandedId]);

  const displayList = showArchived ? archivedConversations : conversations;

  // Chain grouping applies to the active list in both sidebar and full-page
  // mode. Archived list stays flat — REQ-CHN-002 scopes chain navigation to
  // active conversations (the server already filters `archived = 0`).
  const groupedItems: SidebarItem[] | null = useMemo(() => {
    if (showArchived) return null;
    const roots = computeChainRoots(displayList);
    return groupConversationsForSidebar(displayList, roots);
  }, [showArchived, displayList]);

  // Keyboard navigation traverses the flat list of conversations as
  // displayed. For chain blocks the order is members-in-chain-order
  // interleaved with standalones at the chain block's recency rank, so a
  // user pressing j/k walks through the same items they see.
  const keyboardItems = useMemo(() => {
    if (!groupedItems) return displayList;
    const out: Conversation[] = [];
    for (const item of groupedItems) {
      if (item.kind === 'single') out.push(item.conversation);
      else out.push(...item.members);
    }
    return out;
  }, [groupedItems, displayList]);

  const { selectedId } = useKeyboardNav({
    items: keyboardItems,
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

  const toggleChainCollapsed = (rootId: string) => {
    setCollapsedChains(prev => {
      const next = new Set(prev);
      if (next.has(rootId)) next.delete(rootId);
      else next.add(rootId);
      return next;
    });
  };

  // Render a single conversation row. Used both for standalone items in
  // sidebar mode and for every row in non-sidebar mode.
  const renderConvRow = (
    conv: Conversation,
    options: { isChainMember?: boolean; isChainLatest?: boolean; chainIndex?: number } = {},
  ) => {
    const { isChainMember, isChainLatest, chainIndex } = options;
    const classes = [
      'conv-item',
      expandedId === conv.id ? 'expanded' : '',
      selectedId === conv.id ? 'keyboard-selected' : '',
      activeSlug && conv.slug === activeSlug ? 'active' : '',
      isChainMember ? 'conv-item-chain-member' : '',
      isChainLatest ? 'conv-item-chain-latest' : '',
    ]
      .filter(Boolean)
      .join(' ');
    return (
      <li key={conv.id} className={classes} data-id={conv.id}>
        <div className="conv-item-main" onClick={() => handleClick(conv)}>
          <div className="conv-item-slug">
            <span
              className={`conv-state-dot ${getDisplayState(conv.state?.type)}`}
              title={(() => {
                const s = getDisplayState(conv.state?.type);
                switch (s) {
                  case 'idle':
                    return 'Ready';
                  case 'working':
                    return 'Working';
                  case 'error':
                    return 'Error';
                  case 'terminal':
                    return 'Completed';
                  case 'awaiting_approval':
                    return 'Awaiting approval';
                  default:
                    return s;
                }
              })()}
            />
            {chainIndex !== undefined ? (
              <span className="conv-item-slug-pos" title={conv.slug ?? undefined}>
                #{chainIndex + 1}
              </span>
            ) : (
              conv.slug
            )}
            {isChainLatest && (
              <span className="conv-chain-latest-badge" title="Latest in chain — click to continue">
                latest
              </span>
            )}
            {conv.conv_mode_label && (
              <span
                className="conv-mode-badge"
                title={
                  conv.conv_mode_label.toLowerCase() === 'explore'
                    ? 'Managed mode (read-only exploration)'
                    : conv.conv_mode_label.toLowerCase() === 'work'
                      ? 'Managed mode (task branch)'
                      : conv.conv_mode_label.toLowerCase() === 'direct'
                        ? 'Full access (Direct mode)'
                        : conv.conv_mode_label.toLowerCase() === 'branch'
                          ? 'Branch mode (existing branch)'
                          : conv.conv_mode_label
                }
              >
                {conv.conv_mode_label}
              </span>
            )}
          </div>
          <div className="conv-item-meta">
            <span
              className="conv-item-time"
              title={`Created: ${formatShortDateTime(conv.created_at)}\nLast activity: ${formatRelativeTime(conv.updated_at)}`}
            >
              {formatShortDateTime(conv.created_at)} → {formatRelativeTime(conv.updated_at)}
            </span>
            <span className="conv-item-messages">
              {conv.message_count} {conv.message_count === 1 ? 'msg' : 'msgs'}
            </span>
          </div>
          <div className="conv-item-meta secondary">
            {conv.project_id && conv.cwd && (
              <span className="conv-project-label">{conv.cwd.split('/').filter(Boolean).pop()}</span>
            )}
            <span className="conv-item-model">{conv.model}</span>
            <span className="conv-item-cwd">{conv.cwd}</span>
          </div>
        </div>
        <div ref={expandedId === conv.id ? menuRef : undefined} className="conv-item-menu-container">
          <button className="conv-item-menu-btn" onClick={(e) => toggleActions(e, conv.id)} title="Actions">
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
        </div>
      </li>
    );
  };

  // Render a chain block: collapsible header + (when expanded) nested
  // member rows. Header click navigates to the chain page; the caret-only
  // affordance toggles collapse without navigating.
  const renderChainBlock = (item: Extract<SidebarItem, { kind: 'chain' }>) => {
    const collapsed = collapsedChains.has(item.rootId);
    return (
      <li
        key={`chain:${item.rootId}`}
        className={`conv-chain-block ${collapsed ? 'collapsed' : 'expanded'}`}
        data-chain-root={item.rootId}
      >
        <div className="conv-chain-header">
          <button
            className="conv-chain-caret"
            aria-label={collapsed ? 'Expand chain' : 'Collapse chain'}
            aria-expanded={!collapsed}
            onClick={(e) => {
              e.stopPropagation();
              toggleChainCollapsed(item.rootId);
            }}
            title={collapsed ? 'Expand chain' : 'Collapse chain'}
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
              className={`conv-chain-caret-icon ${collapsed ? 'collapsed' : ''}`}
            >
              <polyline points="6 9 12 15 18 9" />
            </svg>
          </button>
          <button
            className="conv-chain-name"
            onClick={() => navigate(`/chains/${item.rootId}`)}
            title={`Open chain "${item.displayName}"`}
          >
            <span className="conv-chain-name-label">{item.displayName}</span>
            <span className="conv-chain-count">{item.members.length}</span>
          </button>
        </div>
        {!collapsed && (
          <ul className="conv-chain-members">
            {item.members.map((m, idx) =>
              renderConvRow(m, {
                isChainMember: true,
                isChainLatest: m.id === item.latestMemberId,
                chainIndex: idx,
              }),
            )}
          </ul>
        )}
      </li>
    );
  };

  const isEmpty = displayList.length === 0;

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
            {authChip}
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
        {isEmpty ? (
          <li className="empty-state">
            <p>{showArchived ? 'No archived conversations' : 'No conversations yet'}</p>
          </li>
        ) : groupedItems ? (
          groupedItems.map(item =>
            item.kind === 'single' ? renderConvRow(item.conversation) : renderChainBlock(item),
          )
        ) : (
          displayList.map(conv => renderConvRow(conv))
        )}
      </ul>
    </section>
  );
}
