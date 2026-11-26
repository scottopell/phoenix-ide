import './Sidebar.css';
import { useState, useCallback, useEffect, useRef, useSyncExternalStore } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import { api } from '../api';
import type { Conversation, ProductConversationListRow } from '../api';
import { ConversationList } from './ConversationList';
import { coordinatorUnread } from '../conversation/readState';
import {
  refreshProductConversationList,
  updateProductConversationRows,
  useProductConversationList,
} from '../conversation/productConversationList';
import { EmberEntry } from './EmberEntry';
import { productConversationPresentationIndicator } from './ConversationList.presentation';
import { ProductConversationActionDialogs } from './ProductConversationActionDialogs';
import {
  projectProductConversationHistory,
  useProductConversationActions,
} from '../hooks/useProductConversationActions';
import { SettingsDropdown } from './SettingsDropdown';
import { LocalServicesPanel } from './LocalServicesPanel';
import { useTheme } from '../hooks';
import { useCodexPreflight } from '../hooks/useCodexPreflight';
import {
  getProductConversationListRevision,
  subscribeProductConversationDeleted,
  subscribeProductConversationClosed,
  subscribeProductConversationListRevision,
} from '../notifications';

const COLLAPSED_DOT_LIMIT = 9;
const PRODUCT_REFRESH_COALESCE_MS = 50;

function productRowMatchesRoute(row: ProductConversationListRow, routeIdentity: string | null): boolean {
  return !!routeIdentity && (
    row.product_conversation_id === routeIdentity
    || row.latest_transcript_row_id === routeIdentity
    || row.canonical_root.transcript_row_id === routeIdentity
    || row.canonical_root.slug === routeIdentity
  );
}

function collapsedDotProductConversations(
  rows: readonly ProductConversationListRow[],
  routeIdentity: string | null,
): readonly ProductConversationListRow[] {
  if (rows.length <= COLLAPSED_DOT_LIMIT) return rows;
  const visible = rows.slice(0, COLLAPSED_DOT_LIMIT);
  if (!routeIdentity || visible.some((row) => productRowMatchesRoute(row, routeIdentity))) return visible;

  const active = rows.find((row) => productRowMatchesRoute(row, routeIdentity));
  if (!active) return visible;
  return [...rows.slice(0, COLLAPSED_DOT_LIMIT - 1), active];
}

const ChevronLeft = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="15 18 9 12 15 6" />
  </svg>
);
const ChevronRight = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="9 18 15 12 9 6" />
  </svg>
);
const TerminalGlyph = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="4 17 10 11 4 5" />
    <line x1="12" y1="19" x2="20" y2="19" />
  </svg>
);

const matchesRouteSegment = (conv: Pick<Conversation, 'id' | 'slug'>, segment: string | null | undefined) =>
  !!segment && (conv.slug === segment || conv.id === segment);

interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
  conversations: readonly Conversation[];
  archivedConversations: readonly Conversation[];
  activeSlug: string | null;
  onConversationCreated: () => void;
  /** Width in px when expanded — driven by useResizablePane */
  width?: number | undefined;
}

export function Sidebar({
  collapsed,
  onToggle,
  conversations,
  archivedConversations,
  activeSlug,
  onConversationCreated,
  width,
}: SidebarProps) {
  const navigate = useNavigate();
  const location = useLocation();
  const { theme, toggleTheme } = useTheme();
  const { codexPreflight, refetchCodexPreflight } = useCodexPreflight();
  const [showArchived, setShowArchived] = useState(false);
  const productList = useProductConversationList();
  const productConversations = productList.rows;
  const productConversationsError = productList.error;
  const productListAvailable = productList.fresh || productList.fromCache;
  const listAvailability = productListAvailable
    ? 'available'
    : productList.error
      ? 'error'
      : 'loading';
  const coordinatorUnreadState = coordinatorUnread(productList);
  const productConversationListRevision = useSyncExternalStore(
    subscribeProductConversationListRevision,
    getProductConversationListRevision,
    getProductConversationListRevision,
  );
  const refreshScheduledRef = useRef<number | null>(null);
  const [activeProductSnapshot, setActiveProductSnapshot] = useState<Awaited<ReturnType<typeof api.getProductConversationSnapshot>> | null>(null);

  const shouldNavigateHomeOnDelete = useCallback((row: ProductConversationListRow) => {
    const productId = row.product_conversation_id;
    const activeSnapshotMatches = activeProductSnapshot?.product_conversation_id === productId
      && activeProductSnapshot.segments.some((segment) => (
        segment.transcript_row_id === activeSlug || segment.slug === activeSlug
      ));
    return productRowMatchesRoute(row, activeSlug) || activeSnapshotMatches;
  }, [activeProductSnapshot, activeSlug]);

  const productActions = useProductConversationActions({
    onAfterMutate: onConversationCreated,
    shouldNavigateHomeOnDelete,
  });

  const scheduleProductRefresh = useCallback(() => {
    if (refreshScheduledRef.current !== null) return;
    refreshScheduledRef.current = window.setTimeout(() => {
      refreshScheduledRef.current = null;
      void refreshProductConversationList();
    }, PRODUCT_REFRESH_COALESCE_MS);
  }, []);

  useEffect(() => () => {
    if (refreshScheduledRef.current !== null) window.clearTimeout(refreshScheduledRef.current);
  }, []);

  useEffect(() => {
    const handleVisibility = () => {
      if (document.visibilityState === 'visible') scheduleProductRefresh();
    };
    const handleFocus = () => scheduleProductRefresh();
    const handleOnline = () => scheduleProductRefresh();
    document.addEventListener('visibilitychange', handleVisibility);
    window.addEventListener('focus', handleFocus);
    window.addEventListener('online', handleOnline);
    return () => {
      document.removeEventListener('visibilitychange', handleVisibility);
      window.removeEventListener('focus', handleFocus);
      window.removeEventListener('online', handleOnline);
    };
  }, [scheduleProductRefresh]);
  const lastArchiveRevealKeyRef = useRef<string | null>(null);
  const openProductConversations = productConversations.filter((row) => row.lifecycle.state === 'open');
  const archivedProductConversations = productConversations.filter((row) => row.lifecycle.state === 'history');
  useEffect(() => {
    const unsubscribes = productConversations.map((row) => {
      const productId = row.product_conversation_id;
      return subscribeProductConversationDeleted(new Set([
        productId,
        row.canonical_root.transcript_row_id,
        row.latest_transcript_row_id,
      ]), () => {
        updateProductConversationRows((current) => current.filter((candidate) => (
          candidate.product_conversation_id !== productId
        )));
      });
    });
    return () => { unsubscribes.forEach((unsubscribe) => unsubscribe()); };
  }, [productConversations]);
  useEffect(() => subscribeProductConversationClosed((productConversationId) => {
    updateProductConversationRows((rows) => projectProductConversationHistory(rows, productConversationId));
  }), []);
  useEffect(() => {
    if (!activeSlug || !(location.pathname.startsWith('/product-conversations/') || location.pathname.startsWith('/c/'))) {
      setActiveProductSnapshot(null);
      return;
    }
    let cancelled = false;
    setActiveProductSnapshot(null);
    api.getProductConversationSnapshot(activeSlug, { message_limit: 1 })
      .then((snapshot) => { if (!cancelled) setActiveProductSnapshot(snapshot); })
      .catch(() => { if (!cancelled) setActiveProductSnapshot(null); });
    return () => { cancelled = true; };
  }, [activeSlug, location.pathname, productConversationListRevision]);

  const scopedActiveCount = openProductConversations.length;
  const scopedArchivedCount = archivedProductConversations.length;
  const activeProductRouteIdentity = activeProductSnapshot?.product_conversation_id ?? activeSlug;

  useEffect(() => {
    if (!activeSlug) {
      lastArchiveRevealKeyRef.current = null;
      return;
    }
    const inOpenProductList = openProductConversations.some(
      (row) => productRowMatchesRoute(row, activeProductRouteIdentity),
    );
    const inHistoryProductList = archivedProductConversations.some(
      (row) => productRowMatchesRoute(row, activeProductRouteIdentity),
    );
    const productRouteResolved = inOpenProductList || inHistoryProductList;
    const inActiveList = inOpenProductList
      || (!productRouteResolved && conversations.some((c) => matchesRouteSegment(c, activeSlug)));
    const inArchivedList = inHistoryProductList
      || (!productRouteResolved && archivedConversations.some((c) => matchesRouteSegment(c, activeSlug)));
    if (!inActiveList && !inArchivedList) return;
    const revealKey = `${activeSlug}:${inActiveList ? 'open' : 'history'}`;
    if (lastArchiveRevealKeyRef.current === revealKey) return;

    if (inArchivedList && !inActiveList && !showArchived) {
      setShowArchived(true);
    } else if (inActiveList && showArchived) {
      setShowArchived(false);
    }
    lastArchiveRevealKeyRef.current = revealKey;
  }, [activeSlug, activeProductRouteIdentity, archivedConversations, archivedProductConversations, conversations, openProductConversations, showArchived]);

  const handleNewClick = useCallback(() => {
    navigate('/new');
  }, [navigate]);

  const handleTerminalClick = useCallback(() => {
    navigate('/terminal');
  }, [navigate]);

  const handleProductConversationClick = useCallback(
    (row: ProductConversationListRow) => navigate(row.canonical_route),
    [navigate],
  );

  const handleToggleArchived = useCallback(() => {
    setShowArchived((prev) => !prev);
  }, []);

  const isOnNewPage = location.pathname === '/' || location.pathname === '/new';
  const isOnTerminalPage = location.pathname === '/terminal';
  const isOnGlobalPage = location.pathname === '/global' || location.pathname.startsWith('/global/');
  const collapsedProductConversations = collapsedDotProductConversations(openProductConversations, activeProductRouteIdentity);
  const collapsedOverflowCount = Math.max(0, openProductConversations.length - collapsedProductConversations.length);

  if (collapsed) {
    return (
      <aside className="sidebar sidebar-collapsed">
        <button className="sidebar-icon-btn sidebar-toggle" onClick={onToggle} title="Expand sidebar">
          <ChevronRight />
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
        <button
          className={`sidebar-icon-btn sidebar-terminal-btn ${isOnTerminalPage ? 'active' : ''}`}
          onClick={handleTerminalClick}
          title="Home terminal"
        >
          <TerminalGlyph />
        </button>
        <EmberEntry compact unread={coordinatorUnreadState} active={isOnGlobalPage} onClick={() => navigate('/global')} />
        <SettingsDropdown
          theme={theme}
          onToggleTheme={toggleTheme}
          codexPreflight={codexPreflight}
          onPreflightInvalidated={refetchCodexPreflight}
          compact
        />
        <div className="sidebar-collapsed-dots">
          {collapsedProductConversations.map((row) => {
            const indicator = productConversationPresentationIndicator(row);
            return (
              <button
                key={row.product_conversation_id}
                className={`sidebar-dot-btn ${productRowMatchesRoute(row, activeProductRouteIdentity) ? 'active' : ''}`}
                onClick={() => navigate(row.canonical_route)}
                title={row.presentation.display_name}
                aria-label={`Open ${row.presentation.display_name}`}
              >
                <span
                  className={`conv-state-dot ${indicator.dotClass}`}
                  role="img"
                  aria-label={indicator.ariaLabel}
                />
              </button>
            );
          })}
          {collapsedOverflowCount > 0 && (
            <button
              className="sidebar-dot-overflow"
              onClick={onToggle}
              title={`${collapsedOverflowCount} more conversations — expand sidebar`}
              aria-label={`${collapsedOverflowCount} more conversations — expand sidebar`}
            >
              +{collapsedOverflowCount}
            </button>
          )}
        </div>
      </aside>
    );
  }

  return (
    <aside
      className="sidebar sidebar-expanded"
      // `--sidebar-pane-width` (set on `.desktop-layout` by the divider's
      // live-drag channel) wins over the committed `width` prop during a drag,
      // so resizing does not re-render this list per frame; the prop is the
      // fallback for hosts that don't drive the variable.
      style={width !== undefined ? { width: `var(--sidebar-pane-width, ${width}px)`, minWidth: `var(--sidebar-pane-width, ${width}px)` } : undefined}
    >
      <div className="sidebar-header">
        <button className="sidebar-toggle-expanded" onClick={onToggle} title="Collapse sidebar">
          <ChevronLeft />
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
        <button
          className={`sidebar-icon-btn sidebar-terminal-btn ${isOnTerminalPage ? 'active' : ''}`}
          onClick={handleTerminalClick}
          title="Home terminal"
          aria-label="Home terminal"
        >
          <TerminalGlyph />
        </button>

        <SettingsDropdown
          theme={theme}
          onToggleTheme={toggleTheme}
          codexPreflight={codexPreflight}
          onPreflightInvalidated={refetchCodexPreflight}
        />
      </div>
      <EmberEntry unread={coordinatorUnreadState} active={isOnGlobalPage} onClick={() => navigate('/global')} />
      <div className="sidebar-lifecycle-tabs" aria-label="Conversation lifecycle">
        <button
          type="button"
          className={`sidebar-lifecycle-tab ${!showArchived ? 'active' : ''}`}
          onClick={() => { if (showArchived) handleToggleArchived(); }}
          aria-pressed={!showArchived}
        >
          <span>Open</span>
          <span className="sidebar-lifecycle-count">{productListAvailable ? scopedActiveCount : '…'}</span>
        </button>
        <button
          type="button"
          className={`sidebar-lifecycle-tab ${showArchived ? 'active' : ''}`}
          onClick={() => { if (!showArchived) handleToggleArchived(); }}
          aria-pressed={showArchived}
        >
          <span>History</span>
          <span className="sidebar-lifecycle-count">{productListAvailable ? scopedArchivedCount : '…'}</span>
        </button>
      </div>
      {productConversationsError && (
        <div className="sidebar-refresh-error" role="status">
          <span>{productList.fromCache
            ? 'Showing cached conversations'
            : productList.fresh
              ? 'Showing last loaded conversations'
              : 'Unable to load conversations'}</span>
          <button type="button" onClick={() => void refreshProductConversationList()}>Retry</button>
        </div>
      )}
      <LocalServicesPanel />
      <div className="sidebar-list">
        <ConversationList
          coordinatorUnread={coordinatorUnreadState}
          productConversations={openProductConversations}
          archivedProductConversations={archivedProductConversations}
          showArchived={showArchived}
          onToggleArchived={handleToggleArchived}
          onNewConversation={handleNewClick}
          onProductConversationClick={handleProductConversationClick}
          onProductConversationRename={productActions.openRename}
          onProductConversationClose={productActions.openClose}

          activeSlug={activeSlug}
          sidebarMode
          listAvailability={listAvailability}
        />
      </div>
      <ProductConversationActionDialogs actions={productActions} />
    </aside>
  );
}
