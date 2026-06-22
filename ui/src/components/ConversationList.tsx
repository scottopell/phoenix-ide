import { memo, useState, useEffect, useRef, useMemo, useCallback, useLayoutEffect } from 'react';
import type { ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { getConvDisplayState } from '../api';
import type { Conversation, CachedPrSummary } from '../api';
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
  onDelete: (conv: Conversation) => void;
  onRename: (conv: Conversation) => void;
  /** Chain-scope archive/delete. Triggered from the chain block
   *  header `⋮` menu. Per-member rows never invoke these — they hide the
   *  affordance entirely so the only path to a chain lifecycle op is the
   *  chain header. The rename callback is per-member rename and reuses
   *  `onRename` (slugs stay per-conversation). */
  onArchiveChain?: (rootId: string) => void;
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
  isSidebarMode: boolean;
  chainIndex: number | undefined;
  showArchived: boolean;
  onClick: (conv: Conversation) => void;
  onToggleMenu: (e: React.MouseEvent, convId: string) => void;
  onArchive: (conv: Conversation) => void;
  onDelete: (conv: Conversation) => void;
  onRename: (conv: Conversation) => void;
  onCloseMenu: () => void;
  /** Forwarded only when this row's menu is open; lets the parent install a
   *  click-outside listener scoped to the actual DOM node. */
  menuRef?: React.RefObject<HTMLDivElement> | undefined;
}

function sidebarPrBadgeLabel(pr: CachedPrSummary): string {
  const n = `#${pr.number}`;
  if (pr.display_state === 'draft') return `${n} draft`;
  if (pr.display_state === 'merged') return `${n} merged`;
  if (pr.display_state === 'closed') return `${n} closed`;
  return n;
}

function sidebarPrBadgeClass(pr: CachedPrSummary): string {
  if (pr.display_state === 'merged') return 'pr-badge pr-badge--merged sidebar-pr-badge';
  if (pr.display_state === 'closed') return 'pr-badge pr-badge--failing sidebar-pr-badge';
  if (pr.display_state === 'draft') return 'pr-badge pr-badge--pending sidebar-pr-badge';
  return 'pr-badge pr-badge--unknown sidebar-pr-badge';
}

function sidebarPrTooltip(pr: CachedPrSummary): string {
  const parts = [`PR #${pr.number}${pr.title ? ` — ${pr.title}` : ''}`];
  if (pr.head || pr.base) parts.push(`${pr.head || '?'} → ${pr.base || '?'}`);
  return parts.join('\n');
}

function SidebarPrBadge({ pr }: { pr: CachedPrSummary }) {
  return (
    <a
      className={sidebarPrBadgeClass(pr)}
      href={pr.url}
      target="_blank"
      rel="noreferrer"
      title={sidebarPrTooltip(pr)}
      onClick={(e) => e.stopPropagation()}
    >
      {sidebarPrBadgeLabel(pr)}
    </a>
  );
}

export const ConversationRow = memo(function ConversationRow({
  conv,
  isMenuOpen,
  isKeyboardSelected,
  isActive,
  isChainMember,
  isChainLatest,
  isSidebarMode,
  chainIndex,
  showArchived,
  onClick,
  onToggleMenu,
  onArchive,
  onDelete,
  onRename,
  onCloseMenu,
  menuRef,
}: ConversationRowProps) {
  const displayState = getConvDisplayState(conv);
  const isCompactCompletedChainMember = isSidebarMode
    && isChainMember
    && !isChainLatest
    && !isActive
    && displayState === 'terminal';
  const classes = [
    'conv-item',
    isMenuOpen ? 'expanded' : '',
    isKeyboardSelected ? 'keyboard-selected' : '',
    isActive ? 'active' : '',
    isChainMember ? 'conv-item-chain-member' : '',
    isChainLatest ? 'conv-item-chain-latest' : '',
    isCompactCompletedChainMember ? 'conv-item-chain-completed' : '',
  ]
    .filter(Boolean)
    .join(' ');

  const cachedPrForBadge = (!isChainMember || isChainLatest) ? conv.cached_pr : undefined;

  const stateTitle = (() => {
    if (conv.state?.type === 'context_exhausted') {
      return conv.presentation_mode === 'needs_action' ? 'Context full' : 'Continued';
    }
    switch (displayState) {
      case 'idle': return 'Ready';
      case 'working': return 'Working';
      case 'error': return 'Error';
      case 'terminal': return 'Completed';
      case 'awaiting-approval': return 'Awaiting approval';
    }
  })();

  return (
    <li className={classes} data-id={conv.id}>
      <div
        className="conv-item-main"
        onClick={() => onClick(conv)}
        title={conv.slug ? `Open conversation "${conv.slug}"` : 'Open conversation'}
      >
        <div className="conv-item-slug">
          <span
            className={`conv-state-dot ${displayState}`}
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
          {cachedPrForBadge && <SidebarPrBadge pr={cachedPrForBadge} />}
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
        <button
          className="conv-item-menu-btn"
          onClick={(e) => onToggleMenu(e, conv.id)}
          title="Actions"
          aria-label="Conversation actions"
          aria-haspopup="menu"
          aria-expanded={isMenuOpen}
        >
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
              title={conv.slug ? `Rename conversation "${conv.slug}"` : 'Rename conversation'}
            >
              Rename
            </button>
            {!isChainMember && !showArchived && (
              <button
                className="action-btn"
                onClick={(e) => {
                  e.stopPropagation();
                  onCloseMenu();
                  onArchive(conv);
                }}
                title={conv.slug ? `Archive conversation "${conv.slug}"` : 'Archive conversation'}
              >
                Archive
              </button>
            )}
            {!isChainMember && (
              <button
                className="action-btn danger"
                onClick={(e) => {
                  e.stopPropagation();
                  onCloseMenu();
                  onDelete(conv);
                }}
                title={conv.slug ? `Delete conversation "${conv.slug}" (can't be undone)` : "Delete conversation (can't be undone)"}
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
  sidebarMode: boolean;
  showArchived: boolean;
  onToggleCollapsed: (rootId: string) => void;
  onToggleChainMenu: (e: React.MouseEvent, rootId: string) => void;
  onCloseChainMenu: () => void;
  onArchiveChain?: ((rootId: string) => void) | undefined;
  onDeleteChain?: ((rootId: string) => void) | undefined;
  onRowClick: (conv: Conversation) => void;
  onRowToggleMenu: (e: React.MouseEvent, convId: string) => void;
  onArchive: (conv: Conversation) => void;
  onDelete: (conv: Conversation) => void;
  onRename: (conv: Conversation) => void;
  onCloseRowMenu: () => void;
  rowMenuRef?: React.RefObject<HTMLDivElement> | undefined;
  chainMenuRef?: React.RefObject<HTMLDivElement> | undefined;
}

export const ChainBlock = memo(function ChainBlock({
  item,
  collapsed,
  isMenuOpen,
  expandedRowId,
  keyboardSelectedId,
  activeSlug,
  sidebarMode,
  showArchived,
  onToggleCollapsed,
  onToggleChainMenu,
  onCloseChainMenu,
  onArchiveChain,
  onDeleteChain,
  onRowClick,
  onRowToggleMenu,
  onArchive,
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
                  // The chain name is edited on the ChainPage header (the single
                  // chain_name input). `?rename=1` opens that editor directly so
                  // the menu item lands on an actionable state instead of a bare
                  // page (a no-op when already on the chain page).
                  navigate(`/chains/${item.rootId}?rename=1`);
                }}
              title={`Open chain "${item.displayName}" to rename it`}
              >
                Rename chain…
              </button>
              {!showArchived && (
                <button
                  className="action-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    onCloseChainMenu();
                    onArchiveChain?.(item.rootId);
                  }}
                  title={`Archive chain "${item.displayName}"`}
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
                title={`Delete chain "${item.displayName}" (can't be undone)`}
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
              isSidebarMode={sidebarMode}
              chainIndex={idx}
              showArchived={showArchived}
              onClick={onRowClick}
              onToggleMenu={onRowToggleMenu}
              onArchive={onArchive}
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
  onDelete,
  onRename,
  onArchiveChain,
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
  const listRootRef = useRef<HTMLElement>(null);
  const lastRevealedActiveSlugRef = useRef<string | null>(null);

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

  useLayoutEffect(() => {
    if (!activeSlug) {
      lastRevealedActiveSlugRef.current = null;
      return;
    }
    if (lastRevealedActiveSlugRef.current === activeSlug) return;

    for (const item of groupedItems) {
      if (item.kind === 'single') {
        if (item.conversation.slug !== activeSlug) continue;
        listRootRef.current
          ?.querySelector<HTMLElement>('.conv-item.active')
          ?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        lastRevealedActiveSlugRef.current = activeSlug;
        return;
      }

      if (!item.members.some((m) => m.slug === activeSlug)) continue;

      const latestMember = item.members.find(m => m.id === item.latestMemberId);
      const isCompleted = getConvDisplayState(latestMember) === 'terminal';
      const collapsed = isCompleted ? !collapsedChains.has(item.rootId) : collapsedChains.has(item.rootId);
      if (collapsed) {
        setCollapsedChains((prev) => {
          const next = new Set(prev);
          if (isCompleted) next.add(item.rootId);
          else next.delete(item.rootId);
          return next;
        });
        return;
      }

      listRootRef.current
        ?.querySelector<HTMLElement>('.conv-item.active')
        ?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
      lastRevealedActiveSlugRef.current = activeSlug;
      return;
    }
  }, [activeSlug, groupedItems, collapsedChains]);

  const closeRowMenu = useCallback(() => setExpandedId(null), []);
  const closeChainMenu = useCallback(() => setExpandedChainId(null), []);

  const isEmpty = displayList.length === 0;

  return (
    <section ref={listRootRef} id="conversation-list" className={`view active ${sidebarMode ? 'sidebar-mode' : ''}`}>
      {!sidebarMode && (
        <div className="view-header">
          <h2>Conversations</h2>
          <div className="view-header-actions">
            {(archivedConversations.length > 0 || showArchived) && (
              <button
                className={`btn-secondary archive-toggle ${showArchived ? 'active' : ''}`}
                onClick={onToggleArchived}
                title={showArchived ? 'Show active conversations' : 'Show archived conversations'}
              >
                {showArchived ? 'Active' : `Archived (${archivedConversations.length})`}
              </button>
            )}
            {authChip}
            <button
              id="new-conv-btn"
              className="btn-primary"
              onClick={onNewConversation}
              title="Start new conversation"
            >
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
            title={showArchived ? 'Show active conversations' : 'Show archived conversations'}
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
                  isSidebarMode={!!sidebarMode}
                  chainIndex={undefined}
                  showArchived={showArchived}
                  onClick={handleClick}
                  onToggleMenu={toggleActions}
                  onArchive={onArchive}
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
                sidebarMode={!!sidebarMode}
                showArchived={showArchived}
                onToggleCollapsed={toggleChainCollapsed}
                onToggleChainMenu={toggleChainActions}
                onCloseChainMenu={closeChainMenu}
                onArchiveChain={onArchiveChain}
                onDeleteChain={onDeleteChain}
                onRowClick={handleClick}
                onRowToggleMenu={toggleActions}
                onArchive={onArchive}
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