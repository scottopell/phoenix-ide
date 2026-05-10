import { memo, useState, useEffect, useRef, useMemo, useCallback } from 'react';
import type { ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { getConvDisplayState } from '../api';
import type { Conversation } from '../api';
import { formatRelativeTime, formatShortDateTime } from '../utils';
import {
  computeChainRoots,
  groupConversationsForSidebar,
  type SidebarItem,
} from '../utils/chains';

import { useKeyboardNav } from '../hooks';


interface ConversationListProps {
  conversations: readonly Conversation[];
  archivedConversations: readonly Conversation[];
  showArchived: boolean;
  onToggleArchived: () => void;
  onNewConversation: () => void;
  onArchive: (conv: Conversation) => void;
  onUnarchive: (conv: Conversation) => void;
  onDelete: (conv: Conversation) => void;
  onRename: (conv: Conversation) => void;
  /** Chain-scope archive/unarchive/delete. Triggered from the chain block
   *  header `⋮` menu. Per-member rows never invoke these — they hide the
   *  affordance entirely so the only path to a chain lifecycle op is the
   *  chain header. The rename callback is per-member rename and reuses
   *  `onRename` (slugs stay per-conversation). */
  onArchiveChain?: (rootId: string) => void;
  onUnarchiveChain?: (rootId: string) => void;
  onDeleteChain?: (rootId: string) => void;
  onConversationClick?: (conv: Conversation) => void;
  activeSlug?: string | null;
  sidebarMode?: boolean;
  authChip?: ReactNode;
}

interface ConversationRowProps {
  conv: Conversation;
  isMenuOpen: boolean;
  isKeyboardSelected: boolean;
  isActive: boolean;
  isChainMember: boolean;
  isChainLatest: boolean;
  chainIndex: number | undefined;
  showArchived: boolean;
  onClick: (conv: Conversation) => void;
  onToggleMenu: (e: React.MouseEvent, convId: string) => void;
  onArchive: (conv: Conversation) => void;
  onUnarchive: (conv: Conversation) => void;
  onDelete: (conv: Conversation) => void;
  onRename: (conv: Conversation) => void;
  onCloseMenu: () => void;
  /** Forwarded only when this row's menu is open; lets the parent install a
   *  click-outside listener scoped to the actual DOM node. */
  menuRef?: React.RefObject<HTMLDivElement> | undefined;
}

const ConversationRow = memo(function ConversationRow({
  conv,
  isMenuOpen,
  isKeyboardSelected,
  isActive,
  isChainMember,
  isChainLatest,
  chainIndex,
  showArchived,
  onClick,
  onToggleMenu,
  onArchive,
  onUnarchive,
  onDelete,
  onRename,
  onCloseMenu,
  menuRef,
}: ConversationRowProps) {
  const classes = [
    'conv-item',
    isMenuOpen ? 'expanded' : '',
    isKeyboardSelected ? 'keyboard-selected' : '',
    isActive ? 'active' : '',
    isChainMember ? 'conv-item-chain-member' : '',
    isChainLatest ? 'conv-item-chain-latest' : '',
  ]
    .filter(Boolean)
    .join(' ');

  const stateTitle = (() => {
    if (conv.state?.type === 'context_exhausted') {
      return conv.presentation_mode === 'needs_action' ? 'Context full' : 'Continued';
    }
    switch (getConvDisplayState(conv)) {
      case 'idle': return 'Ready';
      case 'working': return 'Working';
      case 'error': return 'Error';
      case 'terminal': return 'Completed';
      case 'awaiting_approval': return 'Awaiting approval';
    }
  })();

  return (
    <li className={classes} data-id={conv.id}>
      <div className="conv-item-main" onClick={() => onClick(conv)}>
        <div className="conv-item-slug">
          <span
            className={`conv-state-dot ${getConvDisplayState(conv)}`}
            title={stateTitle}
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
      <div ref={menuRef} className="conv-item-menu-container">
        <button className="conv-item-menu-btn" onClick={(e) => onToggleMenu(e, conv.id)} title="Actions">
          ⋮
        </button>
        {isMenuOpen && (
          <div className="conv-item-actions">
            <button
              className="action-btn"
              onClick={(e) => {
                e.stopPropagation();
                onCloseMenu();
                onRename(conv);
              }}
            >
              Rename
            </button>
            {!isChainMember && (
              showArchived ? (
                <button
                  className="action-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    onCloseMenu();
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
                    onCloseMenu();
                    onArchive(conv);
                  }}
                >
                  Archive
                </button>
              )
            )}
            {!isChainMember && (
              <button
                className="action-btn danger"
                onClick={(e) => {
                  e.stopPropagation();
                  onCloseMenu();
                  onDelete(conv);
                }}
              >
                Delete
              </button>
            )}
          </div>
        )}
      </div>
    </li>
  );
});

interface ChainBlockProps {
  item: Extract<SidebarItem, { kind: 'chain' }>;
  collapsed: boolean;
  isMenuOpen: boolean;
  expandedRowId: string | null;
  keyboardSelectedId: string | null | undefined;
  activeSlug: string | null | undefined;
  showArchived: boolean;
  onToggleCollapsed: (rootId: string) => void;
  onToggleChainMenu: (e: React.MouseEvent, rootId: string) => void;
  onCloseChainMenu: () => void;
  onArchiveChain?: ((rootId: string) => void) | undefined;
  onUnarchiveChain?: ((rootId: string) => void) | undefined;
  onDeleteChain?: ((rootId: string) => void) | undefined;
  onRowClick: (conv: Conversation) => void;
  onRowToggleMenu: (e: React.MouseEvent, convId: string) => void;
  onArchive: (conv: Conversation) => void;
  onUnarchive: (conv: Conversation) => void;
  onDelete: (conv: Conversation) => void;
  onRename: (conv: Conversation) => void;
  onCloseRowMenu: () => void;
  rowMenuRef?: React.RefObject<HTMLDivElement> | undefined;
  chainMenuRef?: React.RefObject<HTMLDivElement> | undefined;
}

const ChainBlock = memo(function ChainBlock({
  item,
  collapsed,
  isMenuOpen,
  expandedRowId,
  keyboardSelectedId,
  activeSlug,
  showArchived,
  onToggleCollapsed,
  onToggleChainMenu,
  onCloseChainMenu,
  onArchiveChain,
  onUnarchiveChain,
  onDeleteChain,
  onRowClick,
  onRowToggleMenu,
  onArchive,
  onUnarchive,
  onDelete,
  onRename,
  onCloseRowMenu,
  rowMenuRef,
  chainMenuRef,
}: ChainBlockProps) {
  const navigate = useNavigate();
  return (
    <li
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
            onToggleCollapsed(item.rootId);
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
        <div ref={chainMenuRef} className="conv-chain-menu-container">
          <button
            className="conv-chain-menu-btn"
            onClick={(e) => onToggleChainMenu(e, item.rootId)}
            title="Chain actions"
            aria-label="Chain actions"
          >
            ⋮
          </button>
          {isMenuOpen && (
            <div className="conv-item-actions conv-chain-actions">
              <button
                className="action-btn"
                onClick={(e) => {
                  e.stopPropagation();
                  onCloseChainMenu();
                  // Per-conversation rename on the root: chain "rename" =
                  // setting the chain_name override on the root via
                  // /api/chains/:rootId/name. Chains are renamed from the
                  // ChainPage header today; surfacing it here would mean
                  // duplicating that input. Open the chain page instead.
                  navigate(`/chains/${item.rootId}`);
                }}
              >
                Rename chain…
              </button>
              {showArchived ? (
                <button
                  className="action-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    onCloseChainMenu();
                    onUnarchiveChain?.(item.rootId);
                  }}
                >
                  Unarchive chain
                </button>
              ) : (
                <button
                  className="action-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    onCloseChainMenu();
                    onArchiveChain?.(item.rootId);
                  }}
                >
                  Archive chain
                </button>
              )}
              <button
                className="action-btn danger"
                onClick={(e) => {
                  e.stopPropagation();
                  onCloseChainMenu();
                  onDeleteChain?.(item.rootId);
                }}
              >
                Delete chain
              </button>
            </div>
          )}
        </div>
      </div>
      {!collapsed && (
        <ul className="conv-chain-members">
          {item.members.map((m, idx) => (
            <ConversationRow
              key={m.id}
              conv={m}
              isMenuOpen={expandedRowId === m.id}
              isKeyboardSelected={keyboardSelectedId === m.id}
              isActive={!!activeSlug && m.slug === activeSlug}
              isChainMember
              isChainLatest={m.id === item.latestMemberId}
              chainIndex={idx}
              showArchived={showArchived}
              onClick={onRowClick}
              onToggleMenu={onRowToggleMenu}
              onArchive={onArchive}
              onUnarchive={onUnarchive}
              onDelete={onDelete}
              onRename={onRename}
              onCloseMenu={onCloseRowMenu}
              menuRef={expandedRowId === m.id ? rowMenuRef : undefined}
            />
          ))}
        </ul>
      )}
    </li>
  );
});

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
  onArchiveChain,
  onUnarchiveChain,
  onDeleteChain,
  onConversationClick,
  activeSlug,
  sidebarMode,
  authChip,
}: ConversationListProps) {
  const navigate = useNavigate();
  const [expandedId, setExpandedId] = useState<string | null>(null);
  // Chain-block menu open state. Tracked separately from `expandedId` so a
  // member row's dropdown and a chain header dropdown can never both be open
  // at once but also can't share a key with a conversation row of the same
  // id.
  const [expandedChainId, setExpandedChainId] = useState<string | null>(null);
  // Per-chain collapse state. NOT persisted across navigations
  // (specs/chains/design.md "Sidebar Grouping"). A chain absent from the
  // map is considered expanded (the default).
  const [collapsedChains, setCollapsedChains] = useState<Set<string>>(new Set());
  const menuRef = useRef<HTMLDivElement>(null);
  const chainMenuRef = useRef<HTMLDivElement>(null);

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

  useEffect(() => {
    if (!expandedChainId) return;
    const handleMouseDown = (e: MouseEvent) => {
      if (chainMenuRef.current && !chainMenuRef.current.contains(e.target as Node)) {
        setExpandedChainId(null);
      }
    };
    document.addEventListener('mousedown', handleMouseDown);
    return () => document.removeEventListener('mousedown', handleMouseDown);
  }, [expandedChainId]);

  const displayList = showArchived ? archivedConversations : conversations;

  // Chain grouping applies to both active and archived lists. Archive is a
  // chain-level op now (chains/lifecycle), so the same chain block renders
  // in either view.
  const groupedItems: SidebarItem[] = useMemo(() => {
    const roots = computeChainRoots(displayList);
    return groupConversationsForSidebar(displayList, roots);
  }, [displayList]);

  // Keyboard navigation traverses the flat list of conversations as
  // displayed. For chain blocks the order is members-in-chain-order
  // interleaved with standalones at the chain block's recency rank, so a
  // user pressing j/k walks through the same items they see.
  const keyboardItems = useMemo(() => {
    const out: Conversation[] = [];
    for (const item of groupedItems) {
      if (item.kind === 'single') out.push(item.conversation);
      else out.push(...item.members);
    }
    return out;
  }, [groupedItems]);

  const { selectedId } = useKeyboardNav({
    items: keyboardItems,
    onNew: onNewConversation,
  });

  const handleClick = useCallback((conv: Conversation) => {
    if (onConversationClick) {
      onConversationClick(conv);
    } else {
      navigate(`/c/${conv.slug}`);
    }
  }, [onConversationClick, navigate]);

  const toggleActions = useCallback((e: React.MouseEvent, convId: string) => {
    e.stopPropagation();
    setExpandedChainId(null);
    setExpandedId((prev) => (prev === convId ? null : convId));
  }, []);

  const toggleChainActions = useCallback((e: React.MouseEvent, rootId: string) => {
    e.stopPropagation();
    setExpandedId(null);
    setExpandedChainId((prev) => (prev === rootId ? null : rootId));
  }, []);

  const toggleChainCollapsed = useCallback((rootId: string) => {
    setCollapsedChains((prev) => {
      const next = new Set(prev);
      if (next.has(rootId)) next.delete(rootId);
      else next.add(rootId);
      return next;
    });
  }, []);

  const closeRowMenu = useCallback(() => setExpandedId(null), []);
  const closeChainMenu = useCallback(() => setExpandedChainId(null), []);

  const isEmpty = displayList.length === 0;

  return (
    <section id="conversation-list" className={`view active ${sidebarMode ? 'sidebar-mode' : ''}`}>
      {!sidebarMode && (
        <div className="view-header">
          <h2>Conversations</h2>
          <div className="view-header-actions">
            {(archivedConversations.length > 0 || showArchived) && (
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
      {sidebarMode && (archivedConversations.length > 0 || showArchived) && (
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
        ) : (
          groupedItems.map((item) => {
            if (item.kind === 'single') {
              const conv = item.conversation;
              return (
                <ConversationRow
                  key={conv.id}
                  conv={conv}
                  isMenuOpen={expandedId === conv.id}
                  isKeyboardSelected={selectedId === conv.id}
                  isActive={!!activeSlug && conv.slug === activeSlug}
                  isChainMember={false}
                  isChainLatest={false}
                  chainIndex={undefined}
                  showArchived={showArchived}
                  onClick={handleClick}
                  onToggleMenu={toggleActions}
                  onArchive={onArchive}
                  onUnarchive={onUnarchive}
                  onDelete={onDelete}
                  onRename={onRename}
                  onCloseMenu={closeRowMenu}
                  menuRef={expandedId === conv.id ? menuRef : undefined}
                />
              );
            }
            const latestMember = item.members.find(m => m.id === item.latestMemberId);
            const isCompleted = getConvDisplayState(latestMember) === 'terminal';
            // Completed chains default collapsed; the toggle set tracks non-default state.
            const collapsed = isCompleted ? !collapsedChains.has(item.rootId) : collapsedChains.has(item.rootId);
            return (
              <ChainBlock
                key={`chain:${item.rootId}`}
                item={item}
                collapsed={collapsed}
                isMenuOpen={expandedChainId === item.rootId}
                expandedRowId={expandedId}
                keyboardSelectedId={selectedId}
                activeSlug={activeSlug}
                showArchived={showArchived}
                onToggleCollapsed={toggleChainCollapsed}
                onToggleChainMenu={toggleChainActions}
                onCloseChainMenu={closeChainMenu}
                onArchiveChain={onArchiveChain}
                onUnarchiveChain={onUnarchiveChain}
                onDeleteChain={onDeleteChain}
                onRowClick={handleClick}
                onRowToggleMenu={toggleActions}
                onArchive={onArchive}
                onUnarchive={onUnarchive}
                onDelete={onDelete}
                onRename={onRename}
                onCloseRowMenu={closeRowMenu}
                rowMenuRef={menuRef}
                chainMenuRef={expandedChainId === item.rootId ? chainMenuRef : undefined}
              />
            );
          })
        )}
      </ul>
    </section>
  );
}
