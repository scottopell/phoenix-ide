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
  listDensity?: 'full' | 'mobile' | 'sidebar';
  authChip?: ReactNode;
  utilityActions?: ReactNode;
}

interface ConversationRowProps {
  conv: Conversation;
  isMenuOpen: boolean;
  isKeyboardSelected: boolean;
  isActive: boolean;
  isChainMember: boolean;
  isChainLatest: boolean;
  listDensity: 'full' | 'mobile' | 'sidebar';
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

function PrBadge({ pr, interactive = true }: { pr: CachedPrSummary; interactive?: boolean }) {
  if (!interactive) {
    return (
      <span
        className={sidebarPrBadgeClass(pr)}
        title={sidebarPrTooltip(pr)}
        aria-label={`PR ${pr.number}`}
      >
        {sidebarPrBadgeLabel(pr)}
      </span>
    );
  }
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

function compactContextLabel(conv: Conversation): string | null {
  if (conv.project_name) return conv.project_name;
  const path = conv.worktree_path || conv.cwd;
  const leaf = path?.split('/').filter(Boolean).pop();
  return leaf || null;
}

function stateLabel(conv: Conversation, displayState: ReturnType<typeof getConvDisplayState>): string {
  if (conv.presentation_mode === 'needs_action' || displayState === 'awaiting-approval') return 'Needs approval';
  if (displayState === 'working') return 'Working';
  if (displayState === 'error') return 'Error';
  if (displayState === 'terminal') return 'Completed';
  return 'Ready';
}

export const SidebarPrBadge = PrBadge;

export const ConversationRow = memo(function ConversationRow({
  conv,
  isMenuOpen,
  isKeyboardSelected,
  isActive,
  isChainMember,
  isChainLatest,
  listDensity,
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
  const isCompactCompletedChainMember = (listDensity === 'sidebar' || listDensity === 'mobile')
    && isChainMember
    && !isChainLatest
    && !isActive
    && displayState === 'terminal';
  const isMobileList = listDensity === 'mobile';
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

  const contextLabel = compactContextLabel(conv);
  const visibleStateLabel = stateLabel(conv, displayState);

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
          {isMobileList && (displayState === 'working' || displayState === 'error' || displayState === 'awaiting-approval') && (
            <span className={`conv-state-chip ${displayState}`}>{visibleStateLabel}</span>
          )}
          {chainIndex !== undefined ? (
            <span className="conv-item-slug-pos" title={conv.slug ?? undefined}>
              #{chainIndex + 1}
            </span>
          ) : (
            <span className="conv-item-title">{conv.slug}</span>
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
          {cachedPrForBadge && <PrBadge pr={cachedPrForBadge} interactive={!isMobileList} />}
          {isMobileList && (
            <span
              className="conv-item-time conv-item-time-mobile"
              title={`Created: ${formatShortDateTime(conv.created_at)}\nLast activity: ${formatRelativeTime(conv.updated_at)}`}
            >
              {formatRelativeTime(conv.updated_at)}
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
          {contextLabel && (
            <span className="conv-project-label">{contextLabel}</span>
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
  /** Chain-scoped, not global. The parent passes the expanded/keyboard-selected
   *  row id only when it belongs to THIS chain, else null — so a global id change
   *  that lands on a different chain produces referentially-identical props here
   *  and the `memo` bails out. The member list still gets the precise id it needs
   *  to highlight one row. */
  expandedRowId: string | null;
  keyboardSelectedId: string | null;
  activeSlug: string | null;
  listDensity: 'full' | 'mobile' | 'sidebar';
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
  listDensity,
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
  const latestMember = item.members.find((m) => m.id === item.latestMemberId) ?? item.members[item.members.length - 1];
  const latestIndex = Math.max(0, item.members.findIndex((m) => m.id === latestMember?.id));
  const latestDisplayState = latestMember ? getConvDisplayState(latestMember) : 'idle';
  const latestContext = latestMember ? compactContextLabel(latestMember) : null;
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
          {listDensity !== 'mobile' && (
            <span className="conv-chain-count">{item.members.length} parts</span>
          )}
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
      {collapsed && listDensity === 'mobile' && latestMember && (
        <button
          className="conv-chain-latest-summary"
          onClick={() => onRowClick(latestMember)}
          title={latestMember.slug ? `Open latest conversation "${latestMember.slug}"` : 'Open latest conversation'}
        >
          <span className={`conv-state-dot ${latestDisplayState}`} title={stateLabel(latestMember, latestDisplayState)} />
          <span className="conv-chain-summary-main">
            <span className="conv-chain-summary-title">Latest #{latestIndex + 1}</span>
            {latestMember.conv_mode_label && <span className="conv-mode-badge">{latestMember.conv_mode_label}</span>}
            <span className="conv-item-time">{formatRelativeTime(latestMember.updated_at)}</span>
          </span>
          <span className="conv-chain-summary-meta">
            {latestContext && <span className="conv-project-label">{latestContext}</span>}
            {latestMember.cached_pr && <PrBadge pr={latestMember.cached_pr} interactive={false} />}
          </span>
        </button>
      )}
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
              listDensity={listDensity}
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

const TerminalGlyph = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="4 17 10 11 4 5" />
    <line x1="12" y1="19" x2="20" y2="19" />
  </svg>
);

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
  listDensity,
  authChip,
  utilityActions,
}: ConversationListProps) {
  const navigate = useNavigate();
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [expandedChainId, setExpandedChainId] = useState<string | null>(null);
  const [collapsedChains, setCollapsedChains] = useState<Set<string>>(new Set());
  const menuRef = useRef<HTMLDivElement>(null);
  const chainMenuRef = useRef<HTMLDivElement>(null);
  const listRootRef = useRef<HTMLElement>(null);
  const lastRevealedActiveSlugRef = useRef<string | null>(null);

  const effectiveListDensity = listDensity ?? (sidebarMode ? 'sidebar' : 'full');
  const isSidebarLayout = effectiveListDensity === 'sidebar';
  const isMobileList = effectiveListDensity === 'mobile';

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

  const groupedItems: SidebarItem[] = useMemo(() => {
    const roots = computeChainRoots(displayList);
    return groupConversationsForSidebar(displayList, roots);
  }, [displayList]);

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
    <section ref={listRootRef} id="conversation-list" className={`view active ${isSidebarLayout ? 'sidebar-mode' : ''} ${isMobileList ? 'mobile-list-density' : ''}`}>
      {!isSidebarLayout && (
        <div className="view-header conversation-list-header">
          <div className="conversation-list-title-group">
            <button
              type="button"
              className="conversation-list-brand"
              onClick={() => navigate('/')}
              title="Phoenix"
              aria-label="Phoenix home"
            >
              <img src="/phoenix.svg" alt="" className="conversation-list-brand-logo" />
              <span>Phoenix</span>
            </button>
            <div className="conversation-list-subnav">
              <button
                className={`archive-toggle-text ${!showArchived ? 'active' : ''}`}
                onClick={() => { if (showArchived) onToggleArchived(); }}
                disabled={!showArchived}
              >
                Active
              </button>
              <span aria-hidden="true">·</span>
              <button
                className={`archive-toggle-text ${showArchived ? 'active' : ''}`}
                onClick={() => { if (!showArchived) onToggleArchived(); }}
              >
                Archived {archivedConversations.length}
              </button>
              {authChip && <span className="conversation-list-auth-chip">{authChip}</span>}
            </div>
          </div>
          <div className="conversation-list-command-cluster">
            <button
              type="button"
              className="conversation-list-icon-btn"
              onClick={() => navigate('/terminal')}
              title="Terminal"
              aria-label="Open terminal"
            >
              <TerminalGlyph />
            </button>
            {utilityActions}
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
      {isSidebarLayout && (archivedConversations.length > 0 || showArchived) && (
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
                  listDensity={effectiveListDensity}
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
            const hasActiveMember = item.members.some((m) => m.slug === activeSlug);
            const collapsed = isMobileList
              ? hasActiveMember ? false : !collapsedChains.has(item.rootId)
              : isCompleted ? !collapsedChains.has(item.rootId) : collapsedChains.has(item.rootId);
            const chainExpandedRowId =
              expandedId !== null && item.members.some((m) => m.id === expandedId)
                ? expandedId
                : null;
            const chainKeyboardSelectedId =
              selectedId != null && item.members.some((m) => m.id === selectedId)
                ? selectedId
                : null;
            const chainActiveSlug =
              activeSlug != null && item.members.some((m) => m.slug === activeSlug)
                ? activeSlug
                : null;
            return (
              <ChainBlock
                key={`chain:${item.rootId}`}
                item={item}
                collapsed={collapsed}
                isMenuOpen={expandedChainId === item.rootId}
                expandedRowId={chainExpandedRowId}
                keyboardSelectedId={chainKeyboardSelectedId}
                activeSlug={chainActiveSlug}
                listDensity={effectiveListDensity}
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
