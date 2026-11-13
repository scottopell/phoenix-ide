import { memo, useState, useEffect, useRef, useMemo, useCallback, useLayoutEffect } from 'react';
import type { ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import type { CachedPrSummary, ProductConversationListRow } from '../api';
import { prReviewState } from './prReviewState';
import { useKeyboardNav } from '../hooks';
import { useFocusScope } from '../hooks/useFocusScope';
import {
  productConversationDisplayTitle,
  productConversationPresentationIndicator,
} from './ConversationList.presentation';
import './ConversationList.css';
import { EmberEntry } from './EmberEntry';
import { markProductConversationRead, markProductConversationUnread } from '../conversation/readState';


interface ConversationListProps {
  productConversations?: readonly ProductConversationListRow[];
  archivedProductConversations?: readonly ProductConversationListRow[];
  showArchived: boolean;
  onToggleArchived: () => void;
  onNewConversation: () => void;
  onProductConversationClick?: (productConversation: ProductConversationListRow) => void;
  onProductConversationRename?: (productConversation: ProductConversationListRow) => void;
  onProductConversationClose?: (productConversation: ProductConversationListRow) => void;
  onProductConversationDelete?: (productConversation: ProductConversationListRow) => void;
  activeSlug?: string | null;
  sidebarMode?: boolean;
  listDensity?: 'full' | 'mobile' | 'sidebar';
  authChip?: ReactNode;
  utilityActions?: ReactNode;
  emptyScopeLabel?: string | null;
  footer?: ReactNode;
  coordinatorUnread?: 'marked' | 'new_activity' | null;
}

function sidebarFeedbackStatusSuffix(pr: CachedPrSummary): string {
  const reviewState = prReviewState(pr.feedback_status);
  return reviewState ? ` ${reviewState.symbol}` : '';
}

function sidebarFeedbackStatusText(pr: CachedPrSummary): string | null {
  const reviewState = prReviewState(pr.feedback_status);
  return reviewState?.label.replace('feedback ', '') ?? null;
}

function sidebarPrBadgeLabel(pr: CachedPrSummary): string {
  const n = `#${pr.number}`;
  const suffix = sidebarFeedbackStatusSuffix(pr);
  if (pr.display_state === 'draft') return `${n} draft${suffix}`;
  if (pr.display_state === 'merged') return `${n} merged${suffix}`;
  if (pr.display_state === 'closed') return `${n} closed${suffix}`;
  return `${n}${suffix}`;
}

function sidebarPrBadgeClass(pr: CachedPrSummary): string {
  const statusClass = pr.feedback_status === 'approved'
    ? ' sidebar-pr-badge--feedback-approved'
    : pr.feedback_status === 'in_progress'
      ? ' sidebar-pr-badge--feedback-in-progress'
      : '';
  if (pr.display_state === 'merged') return `pr-badge pr-badge--merged sidebar-pr-badge${statusClass}`;
  if (pr.display_state === 'closed') return `pr-badge pr-badge--failing sidebar-pr-badge${statusClass}`;
  if (pr.display_state === 'draft') return `pr-badge pr-badge--pending sidebar-pr-badge${statusClass}`;
  return `pr-badge pr-badge--unknown sidebar-pr-badge${statusClass}`;
}

function sidebarPrTooltip(pr: CachedPrSummary): string {
  const parts = [`PR #${pr.number}${pr.title ? ` — ${pr.title}` : ''}`];
  if (pr.head || pr.base) parts.push(`${pr.head || '?'} → ${pr.base || '?'}`);
  const feedbackStatus = sidebarFeedbackStatusText(pr);
  if (feedbackStatus) parts.push(`Feedback status: ${feedbackStatus}`);
  return parts.join('\n');
}

function sidebarPrAriaLabel(pr: CachedPrSummary): string {
  const parts = [`PR ${pr.number}`];
  if (pr.display_state !== 'open') parts.push(pr.display_state);
  const feedbackStatus = sidebarFeedbackStatusText(pr);
  if (feedbackStatus) parts.push(`feedback ${feedbackStatus}`);
  return parts.join(', ');
}

function PrBadge({ pr, interactive = true }: { pr: CachedPrSummary; interactive?: boolean }) {
  if (!interactive) {
    return (
      <span
        className={sidebarPrBadgeClass(pr)}
        title={sidebarPrTooltip(pr)}
        aria-label={sidebarPrAriaLabel(pr)}
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
      aria-label={sidebarPrAriaLabel(pr)}
    >
      {sidebarPrBadgeLabel(pr)}
    </a>
  );
}


const ProductConversationListRowView = memo(function ProductConversationListRowView({
  row,
  isActive,
  isKeyboardSelected,
  onClick,
  onProductConversationClose,
  onProductConversationDelete,
  onProductConversationRename,
  isMenuOpen,
  onToggleMenu,
  onCloseMenu,
  menuRef,
  isMobileList = false,
}: {
  isMobileList?: boolean;
  isMenuOpen: boolean;
  onToggleMenu: (event: React.MouseEvent, id: string) => void;
  onCloseMenu: () => void;
  menuRef?: React.RefObject<HTMLDivElement> | undefined;
  row: ProductConversationListRow;
  isActive: boolean;
  isKeyboardSelected: boolean;
  onClick: (row: ProductConversationListRow) => void;
  onProductConversationClose?: (row: ProductConversationListRow) => void;
  onProductConversationDelete?: (row: ProductConversationListRow) => void;
  onProductConversationRename?: (row: ProductConversationListRow) => void;
}) {
  const unread = row.read_state.state === 'unread';
  const unreadLabel = row.read_state.state === 'unread'
    ? (row.read_state.reason === 'marked' ? 'Marked unread' : 'New activity')
    : null;
  const classes = [
    'conv-item',
    isActive ? 'active' : '',
    isKeyboardSelected ? 'keyboard-selected' : '',
    unread ? 'unread' : '',
    'product-conversation-list-row',
  ].filter(Boolean).join(' ');
  const displayTitle = productConversationDisplayTitle(row);
  const indicator = productConversationPresentationIndicator(row);
  const statusTitle = indicator.label;
  const closeAction = row.lifecycle.state === 'open' ? row.lifecycle.close_action : null;
  const closeUnavailableReason = closeAction?.availability === 'unavailable'
    ? ({
        history: 'This conversation is already in History',
        active_close_attempt: 'Close is already in progress',
        awaiting_task_approval: 'Resolve the pending task approval before closing',
        awaiting_continuation: 'Resolve the pending continuation before closing',
        handed_off_without_continuation: 'Complete the continuation handoff before closing',
      } as const)[closeAction.reason]
    : undefined;
  const canRename = !!onProductConversationRename && row.lifecycle.state === 'open'
    && !(closeAction?.availability === 'unavailable' && closeAction.reason === 'active_close_attempt');
  const canDelete = !!onProductConversationDelete && row.lifecycle.state === 'history';
  return (
    <li
      className={classes}
      data-id={row.product_conversation_id}
      data-product-conversation-id={row.product_conversation_id}
    >
      <button
        type="button"
        className="conv-item-main"
        onClick={() => onClick(row)}
        title={`Open conversation "${displayTitle}" — ${statusTitle}`}
      >
        <span
          className={`conv-state-dot ${indicator.dotClass}`}
          title={statusTitle}
          role="img"
          aria-label={indicator.ariaLabel}
        />
        <span className="conv-item-title">{displayTitle}</span>
        {unreadLabel && (
          <span
            className={`conv-item-unread-dot${row.read_state.state === 'unread' && row.read_state.reason === 'marked' ? ' conv-item-unread-dot--marked' : ''}`}
            title={unreadLabel}
            role="img"
            aria-label={unreadLabel}
          />
        )}
      </button>
      {row.cached_pr && (
        <span className="product-conversation-list-row__prs">
          <PrBadge pr={row.cached_pr} interactive={!isMobileList} />
        </span>
      )}
      <div ref={menuRef} className="conv-item-menu-container product-conversation-actions">
          <button type="button" className="conv-item-menu-btn conv-action-btn"
            aria-label={`Actions for conversation ${displayTitle}`} aria-expanded={isMenuOpen}
            onClick={(event) => onToggleMenu(event, row.product_conversation_id)}>⋮</button>
          {isMenuOpen && <div className="conv-item-actions" aria-label={`Actions for ${displayTitle}`}>
          <button
            type="button"
            className="conv-action-btn"
            onClick={(event) => {
              event.stopPropagation();
              onCloseMenu();
              const action = unread ? markProductConversationRead : markProductConversationUnread;
              void action(row.product_conversation_id).catch(() => { /* list refresh shows the authoritative state */ });
            }}
            aria-label={`${unread ? 'Mark as read' : 'Mark as unread'}: ${displayTitle}`}
            title={unread ? 'Mark as read' : 'Mark as unread to revisit later'}
          >
            {unread ? 'Mark as read' : 'Mark as unread'}
          </button>
          {canRename && onProductConversationRename && (
            <button
              type="button"
              className="conv-action-btn"
              onClick={(event) => { event.stopPropagation(); onCloseMenu(); onProductConversationRename(row); }}
              aria-label={`Rename conversation ${displayTitle}`}
              title="Rename"
            >
              Rename
            </button>
          )}
          {canDelete && onProductConversationDelete && (
            <button
              type="button"
              className="conv-action-btn danger"
              onClick={(event) => { event.stopPropagation(); onCloseMenu(); onProductConversationDelete(row); }}
              aria-label={`Delete conversation ${displayTitle}`}
              title="Delete permanently"
            >
              Delete permanently
            </button>
          )}
          {onProductConversationClose && closeAction?.availability === 'available' && (
            <button
              type="button"
              className="conv-action-btn danger"
              onClick={(event) => { event.stopPropagation(); onCloseMenu(); onProductConversationClose(row); }}
              aria-label={`Close conversation ${displayTitle}`}
              title="Close"
            >
              Close conversation
            </button>
          )}
          {onProductConversationClose && closeAction?.availability === 'unavailable' && (
            <button
              type="button"
              className="conv-action-btn danger"
              disabled
              aria-label={`Close conversation ${displayTitle}. ${closeUnavailableReason}`}
              title={closeUnavailableReason}
            >
              Close conversation — {closeUnavailableReason}
            </button>
          )}
          </div>}
      </div>
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
  productConversations = [],
  archivedProductConversations = [],
  showArchived,
  onToggleArchived,
  onNewConversation,
  onProductConversationClick,
  onProductConversationRename,
  onProductConversationClose,
  activeSlug,
  onProductConversationDelete,
  sidebarMode,
  listDensity,
  authChip,
  utilityActions,
  emptyScopeLabel,
  footer,
  coordinatorUnread = null,
}: ConversationListProps) {
  const navigate = useNavigate();
  const { hasActiveScope } = useFocusScope();
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const listRootRef = useRef<HTMLElement>(null);
  const effectiveListDensity = listDensity ?? (sidebarMode ? 'sidebar' : 'full');
  const isSidebarLayout = effectiveListDensity === 'sidebar';
  const isMobileList = effectiveListDensity === 'mobile';

  useLayoutEffect(() => {
    const root = listRootRef.current;
    const header = root?.querySelector<HTMLElement>('.conversation-list-pinned-header');
    const scrollOwner = root?.closest<HTMLElement>('[data-app-scroll-owner]');
    if (!root || !header || !scrollOwner) return;
    const previousScrollPaddingTop = scrollOwner.style.scrollPaddingTop;
    const updateScrollOffset = () => {
      scrollOwner.style.scrollPaddingTop = `${header.getBoundingClientRect().height}px`;
    };
    updateScrollOffset();
    const observer = new ResizeObserver(updateScrollOffset);
    observer.observe(header);
    return () => {
      observer.disconnect();
      scrollOwner.style.scrollPaddingTop = previousScrollPaddingTop;
    };
  }, [effectiveListDensity, sidebarMode]);
  const lastRevealedActiveSlugRef = useRef<string | null>(null);

  useEffect(() => {
    if (!expandedId) return;
    const handleMouseDown = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setExpandedId(null);
      }
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || hasActiveScope) return;
      event.preventDefault();
      event.stopPropagation();
      const trigger = menuRef.current?.querySelector<HTMLButtonElement>('.conv-item-menu-btn');
      setExpandedId(null);
      trigger?.focus();
    };
    document.addEventListener('mousedown', handleMouseDown);
    document.addEventListener('keydown', handleEscape, true);
    return () => {
      document.removeEventListener('mousedown', handleMouseDown);
      document.removeEventListener('keydown', handleEscape, true);
    };
  }, [expandedId, hasActiveScope]);

  const displayProductList = showArchived ? archivedProductConversations : productConversations;

  useLayoutEffect(() => {
    if (expandedId && !menuRef.current) setExpandedId(null);
  }, [expandedId, displayProductList, onProductConversationRename, onProductConversationClose, onProductConversationDelete]);

  const keyboardItems = useMemo(
    () => displayProductList.map((row) => ({ id: row.product_conversation_id })),
    [displayProductList],
  );

  const { selectedId } = useKeyboardNav({
    enabled: expandedId === null,
    items: keyboardItems,
    onSelect: (item: { id: string }) => {
      const row = displayProductList.find((candidate) => candidate.product_conversation_id === item.id);
      if (!row) return;
      if (onProductConversationClick) onProductConversationClick(row);
      else navigate(row.canonical_route);
    },
    onNew: onNewConversation,
  });

  const toggleActions = useCallback((e: React.MouseEvent, convId: string) => {
    e.stopPropagation();
    setExpandedId((prev) => (prev === convId ? null : convId));
  }, []);

  useLayoutEffect(() => {
    if (!activeSlug) {
      lastRevealedActiveSlugRef.current = null;
      return;
    }
    if (lastRevealedActiveSlugRef.current === activeSlug) return;
    const activeProductRow = displayProductList.find((row) => (
      row.product_conversation_id === activeSlug
      || row.latest_transcript_row_id === activeSlug
      || row.canonical_root.transcript_row_id === activeSlug
      || row.canonical_root.slug === activeSlug
    ));
    if (!activeProductRow) return;
    listRootRef.current
      ?.querySelector<HTMLElement>(`[data-product-conversation-id="${activeProductRow.product_conversation_id}"]`)
      ?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    lastRevealedActiveSlugRef.current = activeSlug;
  }, [activeSlug, displayProductList]);

  const closeRowMenu = useCallback(() => {
    menuRef.current?.querySelector<HTMLButtonElement>('.conv-item-menu-btn')?.focus();
    setExpandedId(null);
  }, []);

  const isEmpty = displayProductList.length === 0;

  return (
    <section ref={listRootRef} id="conversation-list" className={`view active ${isSidebarLayout ? 'sidebar-mode' : ''} ${isMobileList ? 'mobile-list-density' : ''}`}>
      {!isSidebarLayout && (
        <div className="conversation-list-pinned-header">
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
          <EmberEntry unread={coordinatorUnread} onClick={() => navigate('/global')} />
          <div className="conversation-list-subnav">
            <button
              className={`archive-toggle-text ${!showArchived ? 'active' : ''}`}
              onClick={() => { if (showArchived) onToggleArchived(); }}
              disabled={!showArchived}
            >
              Open
            </button>
            <span aria-hidden="true">·</span>
            <button
              className={`archive-toggle-text ${showArchived ? 'active' : ''}`}
              onClick={() => { if (!showArchived) onToggleArchived(); }}
            >
              History {archivedProductConversations.length}
            </button>
            {authChip && <span className="conversation-list-auth-chip">{authChip}</span>}
          </div>
        </div>
      )}

      <ul id="conv-list">
        {isEmpty ? (
          <li className="empty-state">
            <p>{`No ${showArchived ? 'History' : 'Open'} conversations${emptyScopeLabel ? ` in ${emptyScopeLabel}` : ''}`}</p>
          </li>
        ) : (
          displayProductList.map((row) => (
            <ProductConversationListRowView
              key={row.product_conversation_id}
              row={row}
              isMobileList={isMobileList}
              isMenuOpen={expandedId === row.product_conversation_id}
              onToggleMenu={toggleActions}
              onCloseMenu={closeRowMenu}
              menuRef={expandedId === row.product_conversation_id ? menuRef : undefined}
              isActive={activeSlug === row.product_conversation_id || activeSlug === row.canonical_root.slug || activeSlug === row.canonical_root.transcript_row_id}
              isKeyboardSelected={selectedId === row.product_conversation_id}
              onClick={(productRow) => {
                if (onProductConversationClick) onProductConversationClick(productRow);
                else navigate(productRow.canonical_route);
              }}
              {...(onProductConversationClose ? { onProductConversationClose } : {})}
              {...(onProductConversationDelete ? { onProductConversationDelete } : {})}
              {...(onProductConversationRename ? { onProductConversationRename } : {})}
            />
          ))
        )}
      </ul>
      {footer && <div className="conversation-list-footer">{footer}</div>}
    </section>
  );
}
