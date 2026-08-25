import { useState, useCallback, useContext, useEffect, useRef } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import { api, getConvDisplayState } from '../api';
import type { Conversation, ProductConversationListRow } from '../api';
import { ConversationList } from './ConversationList';
import { ConfirmDialog } from './ConfirmDialog';
import { RenameDialog } from './RenameDialog';
import { SettingsDropdown } from './SettingsDropdown';
import { LocalServicesPanel } from './LocalServicesPanel';
import { useTheme } from '../hooks';
import type { CodexLoginPreflight } from '../api';
import { subscribeModels } from '../modelsPoller';
import { ConversationContext } from '../conversation/ConversationContext';

const COLLAPSED_DOT_LIMIT = 9;

function collapsedDotConversations(conversations: readonly Conversation[], activeSlug: string | null): readonly Conversation[] {
  if (conversations.length <= COLLAPSED_DOT_LIMIT) return conversations;
  const visible = conversations.slice(0, COLLAPSED_DOT_LIMIT);
  if (!activeSlug || visible.some((c) => c.slug === activeSlug)) return visible;

  const active = conversations.find((c) => c.slug === activeSlug);
  if (!active) return visible;
  return [...conversations.slice(0, COLLAPSED_DOT_LIMIT - 1), active];
}

function productRowMatchesRoute(row: ProductConversationListRow, routeIdentity: string | null): boolean {
  return !!routeIdentity && (
    row.product_conversation_id === routeIdentity
    || row.latest_transcript_row_id === routeIdentity
    || row.canonical_root.transcript_row_id === routeIdentity
    || row.canonical_root.slug === routeIdentity
  );
}

function productRowDotClass(row: ProductConversationListRow): string {
  if (row.presentation.kind === 'needs_action') return 'awaiting-approval';
  switch (row.presentation.presentation_mode) {
    case 'working': return 'working';
    case 'error': return 'error';
    case 'done': return 'terminal';
    default: return row.ordinary_lifecycle === 'history' ? 'terminal' : 'idle';
  }
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
  const conversationStore = useContext(ConversationContext);
  const { theme, toggleTheme } = useTheme();
  const [codexPreflight, setCodexPreflight] = useState<CodexLoginPreflight | null>(null);
  const refetchCodexPreflight = useCallback(() => {
    api.codexLoginPreflight()
      .then((p) => setCodexPreflight(p))
      .catch(() => { /* chip just hides — non-fatal */ });
  }, []);
  const [showArchived, setShowArchived] = useState(false);
  const [productConversations, setProductConversations] = useState<ProductConversationListRow[]>([]);
  const [productConversationsError, setProductConversationsError] = useState<string | null>(null);
  const [productConversationsRetry, setProductConversationsRetry] = useState(0);
  const [deleteTarget, setDeleteTarget] = useState<Conversation | null>(null);
  const [renameTarget, setRenameTarget] = useState<Conversation | null>(null);
  const [renameError, setRenameError] = useState<string | undefined>();

  // Fetch once at mount, and refetch whenever the credential health flips.
  // The shared models poller fires on credential transitions (login completes,
  // token expires, sign-out wipes), so subscribing keeps the chip in sync
  // without a second polling loop.
  useEffect(() => {
    refetchCodexPreflight();
    let lastConfigured: boolean | null = null;
    const unsub = subscribeModels((m) => {
      if (lastConfigured !== null && lastConfigured !== m.llm_configured) {
        refetchCodexPreflight();
      }
      lastConfigured = m.llm_configured;
    });
    return () => { unsub(); };
  }, [refetchCodexPreflight]);

  useEffect(() => {
    let cancelled = false;
    api.listProductConversations()
      .then((response) => {
        if (!cancelled) {
          setProductConversations(response.product_conversations);
          setProductConversationsError(null);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setProductConversationsError(error instanceof Error ? error.message : 'Failed to refresh conversations');
        }
      });
    return () => {
      cancelled = true;
    };
  }, [productConversationsRetry]);
  const lastArchiveRevealSlugRef = useRef<string | null>(null);
  const openProductConversations = productConversations.filter((row) => row.ordinary_lifecycle !== 'history');
  const archivedProductConversations = productConversations.filter((row) => row.ordinary_lifecycle === 'history');
  const scopedActiveCount = openProductConversations.length;
  const scopedArchivedCount = archivedProductConversations.length;

  useEffect(() => {
    if (!activeSlug) {
      lastArchiveRevealSlugRef.current = null;
      return;
    }
    if (lastArchiveRevealSlugRef.current === activeSlug) return;

    const inActiveList = conversations.some((c) => matchesRouteSegment(c, activeSlug))
      || openProductConversations.some((row) => productRowMatchesRoute(row, activeSlug));
    const inArchivedList = archivedConversations.some((c) => matchesRouteSegment(c, activeSlug))
      || archivedProductConversations.some((row) => productRowMatchesRoute(row, activeSlug));
    if (!inActiveList && !inArchivedList) return;

    if (inArchivedList && !inActiveList && !showArchived) {
      setShowArchived(true);
    } else if (inActiveList && showArchived) {
      setShowArchived(false);
    }
    lastArchiveRevealSlugRef.current = activeSlug;
  }, [activeSlug, archivedConversations, archivedProductConversations, conversations, openProductConversations, showArchived]);

  const handleNewClick = useCallback(() => {
    navigate('/new');
  }, [navigate]);

  const handleTerminalClick = useCallback(() => {
    navigate('/terminal');
  }, [navigate]);

  const handleConversationClick = useCallback((conv: Conversation) => {
    navigate(`/c/${conv.slug}`);
  }, [navigate]);

  const handleArchive = useCallback(async (conv: Conversation) => {
    try {
      await api.archiveConversation(conv.id);
      onConversationCreated();
    } catch (err) {
      console.error('Failed to archive:', err);
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


  const applyRenameSnapshot = useCallback((oldSlug: string | null | undefined, conversation: Conversation) => {
    if (oldSlug) {
      conversationStore?.replaceSlugSnapshot(oldSlug, conversation);
    }
    if (oldSlug && conversation.slug && oldSlug === activeSlug) {
      navigate(`/c/${conversation.slug}`, { replace: true });
    }
  }, [activeSlug, conversationStore, navigate]);

  const handleRename = useCallback(async (newName: string) => {
    if (!renameTarget) return;
    try {
      const res = await api.renameConversation(renameTarget.id, newName);
      applyRenameSnapshot(renameTarget.slug, res.conversation);
      setRenameTarget(null);
      setRenameError(undefined);
      onConversationCreated();
    } catch (err) {
      setRenameError(err instanceof Error ? err.message : 'Failed to rename');
    }
  }, [renameTarget, onConversationCreated, applyRenameSnapshot]);

  const handleGenerateRename = useCallback(async () => {
    if (!renameTarget) return;
    try {
      const res = await api.regenerateConversationName(renameTarget.id);
      applyRenameSnapshot(renameTarget.slug, res.conversation);
      setRenameTarget(null);
      setRenameError(undefined);
      onConversationCreated();
    } catch (err) {
      setRenameError(err instanceof Error ? err.message : 'Failed to generate name');
      throw err;
    }
  }, [renameTarget, onConversationCreated, applyRenameSnapshot]);

  const handleSetDeleteTarget = useCallback((conv: Conversation) => {
    setDeleteTarget(conv);
  }, []);

  const handleSetRenameTarget = useCallback((conv: Conversation) => {
    setRenameError(undefined);
    setRenameTarget(conv);
  }, []);

  const handleToggleArchived = useCallback(() => {
    setShowArchived((prev) => !prev);
  }, []);

  const isOnNewPage = location.pathname === '/' || location.pathname === '/new';
  const isOnTerminalPage = location.pathname === '/terminal';
  const isOnGlobalPage = location.pathname === '/global';
  const collapsedProductConversations = collapsedDotProductConversations(openProductConversations, activeSlug);
  const collapsedConversations = productConversationsError && productConversations.length === 0
    ? collapsedDotConversations(conversations, activeSlug)
    : [];
  const collapsedVisibleCount = collapsedProductConversations.length || collapsedConversations.length;
  const collapsedTotalCount = collapsedProductConversations.length > 0
    ? openProductConversations.length
    : conversations.length;
  const collapsedOverflowCount = Math.max(0, collapsedTotalCount - collapsedVisibleCount);

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
        <button
          className={`sidebar-icon-btn ${isOnGlobalPage ? 'active' : ''}`}
          onClick={() => navigate('/global')}
          title="Coordinator"
          aria-label="Coordinator"
        >
          ◎
        </button>
        <SettingsDropdown
          theme={theme}
          onToggleTheme={toggleTheme}
          codexPreflight={codexPreflight}
          onPreflightInvalidated={refetchCodexPreflight}
          compact
        />
        <div className="sidebar-collapsed-dots">
          {collapsedProductConversations.map((row) => (
            <button
              key={row.product_conversation_id}
              className={`sidebar-dot-btn ${productRowMatchesRoute(row, activeSlug) ? 'active' : ''}`}
              onClick={() => navigate(row.canonical_route)}
              title={row.presentation.display_name}
              aria-label={`Open ${row.presentation.display_name}`}
            >
              <span className={`conv-state-dot ${productRowDotClass(row)}`} />
            </button>
          ))}
          {collapsedConversations.map(conv => {
            const displayState = getConvDisplayState(conv);
            const isActive = matchesRouteSegment(conv, activeSlug);
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
        <button
          className={`sidebar-icon-btn ${isOnGlobalPage ? 'active' : ''}`}
          onClick={() => navigate('/global')}
          title="Coordinator"
          aria-label="Coordinator"
        >
          ◎
        </button>
        <SettingsDropdown
          theme={theme}
          onToggleTheme={toggleTheme}
          codexPreflight={codexPreflight}
          onPreflightInvalidated={refetchCodexPreflight}
        />
      </div>
      <div className="sidebar-lifecycle-tabs" aria-label="Conversation lifecycle">
        <button
          type="button"
          className={`sidebar-lifecycle-tab ${!showArchived ? 'active' : ''}`}
          onClick={() => { if (showArchived) handleToggleArchived(); }}
          aria-pressed={!showArchived}
        >
          <span>Open</span>
          <span className="sidebar-lifecycle-count">{scopedActiveCount}</span>
        </button>
        <button
          type="button"
          className={`sidebar-lifecycle-tab ${showArchived ? 'active' : ''}`}
          onClick={() => { if (!showArchived) handleToggleArchived(); }}
          aria-pressed={showArchived}
        >
          <span>History</span>
          <span className="sidebar-lifecycle-count">{scopedArchivedCount}</span>
        </button>
      </div>
      {productConversationsError && (
        <div className="sidebar-refresh-error" role="status">
          <span>Showing cached conversations</span>
          <button type="button" onClick={() => setProductConversationsRetry((revision) => revision + 1)}>Retry</button>
        </div>
      )}
      <LocalServicesPanel />
      <div className="sidebar-list">
        <ConversationList
          conversations={productConversationsError && productConversations.length === 0 ? conversations : []}
          archivedConversations={productConversationsError && productConversations.length === 0 ? archivedConversations : []}
          productConversations={openProductConversations}
          archivedProductConversations={archivedProductConversations}
          showArchived={showArchived}
          onToggleArchived={handleToggleArchived}
          onNewConversation={handleNewClick}
          onArchive={handleArchive}
          onDelete={handleSetDeleteTarget}
          onRename={handleSetRenameTarget}
          onConversationClick={handleConversationClick}
          onProductConversationClick={(row) => navigate(row.canonical_route)}
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
        onGenerate={handleGenerateRename}
        onCancel={() => { setRenameTarget(null); setRenameError(undefined); }}
      />
    </aside>
  );
}
