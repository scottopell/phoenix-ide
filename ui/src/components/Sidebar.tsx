import { useState, useCallback, useContext, useEffect, useMemo, useRef } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import { api, getConvDisplayState } from '../api';
import type { ChainView, Conversation, Project } from '../api';
import { ConversationList } from './ConversationList';
import { ConfirmDialog } from './ConfirmDialog';
import { ChainDeleteConfirm } from './ChainDeleteConfirm';
import { RenameDialog } from './RenameDialog';
import { SettingsDropdown } from './SettingsDropdown';
import { LocalServicesPanel } from './LocalServicesPanel';
import { useTheme } from '../hooks';
import type { CodexLoginPreflight } from '../api';
import { subscribeModels } from '../modelsPoller';
import { ConversationContext } from '../conversation/ConversationContext';

const PROJECT_FILTER_KEY = 'phoenix:sidebar-project-filter';
const COLLAPSED_DOT_LIMIT = 9;

function projectLabel(project: Project): string {
  return project.canonical_path.split('/').filter(Boolean).pop() || project.canonical_path;
}

function countForProject(conversations: readonly Conversation[], projectId: string | null): number {
  if (projectId === null) return conversations.length;
  return conversations.filter((c) => c.project_id === projectId).length;
}

function collapsedDotConversations(conversations: readonly Conversation[], activeSlug: string | null): readonly Conversation[] {
  if (conversations.length <= COLLAPSED_DOT_LIMIT) return conversations;
  const visible = conversations.slice(0, COLLAPSED_DOT_LIMIT);
  if (!activeSlug || visible.some((c) => c.slug === activeSlug)) return visible;

  const active = conversations.find((c) => c.slug === activeSlug);
  if (!active) return visible;
  return [...conversations.slice(0, COLLAPSED_DOT_LIMIT - 1), active];
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

  const [showArchived, setShowArchived] = useState(false);
  const lastProjectFilterRevealSlugRef = useRef<string | null>(null);
  const lastArchiveRevealSlugRef = useRef<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Conversation | null>(null);
  const [deleteChainTarget, setDeleteChainTarget] = useState<ChainView | null>(null);
  const [renameTarget, setRenameTarget] = useState<Conversation | null>(null);
  const [renameError, setRenameError] = useState<string | undefined>();
  const [projects, setProjects] = useState<Project[]>([]);
  // Tracks whether `api.getProjects()` has successfully resolved at least
  // once. The stale-filter cleanup below gates on this so it doesn't
  // clear during the initial empty-state render, but DOES clear once
  // we've confirmed (via a successful fetch) that the persisted project
  // no longer exists -- including the case where the API legitimately
  // returns an empty list.
  const [projectsLoaded, setProjectsLoaded] = useState(false);
  const [activeProjectId, setActiveProjectIdState] = useState<string | null>(() => {
    try {
      return localStorage.getItem(PROJECT_FILTER_KEY);
    } catch {
      return null;
    }
  });
  const setActiveProjectId = useCallback((id: string | null) => {
    setActiveProjectIdState(id);
    try {
      if (id === null) localStorage.removeItem(PROJECT_FILTER_KEY);
      else localStorage.setItem(PROJECT_FILTER_KEY, id);
    } catch {
      // storage full / disabled — degrade gracefully
    }
  }, []);

  // Fetch projects on mount
  useEffect(() => {
    api.getProjects().then((rows) => {
      setProjects(rows);
      setProjectsLoaded(true);
    }).catch(() => {
      // Transient failure: leave projectsLoaded false so the cleanup
      // effect below doesn't clear the persisted filter on a network
      // blip. A subsequent conversations-count tick will retry.
    });
  }, [conversations.length]); // re-fetch when conversation count changes

  // Clear the persisted filter if the project no longer exists (e.g.,
  // deleted server-side while the user was offline). Gated on
  // projectsLoaded so we don't clear during the initial unloaded state,
  // but DO clear once a successful fetch has confirmed the project is
  // gone -- including the case where the API returns []. Without this
  // gate-via-flag (vs. gating on `projects.length > 0`), a stale
  // filter could survive the deletion of all projects.
  useEffect(() => {
    if (
      projectsLoaded &&
      activeProjectId &&
      !projects.some((p) => p.id === activeProjectId)
    ) {
      setActiveProjectId(null);
    }
  }, [activeProjectId, projects, projectsLoaded, setActiveProjectId]);

  // Filter conversations by selected project
  const filteredConversations = useMemo(() => {
    if (!activeProjectId) return conversations;
    return conversations.filter(c => c.project_id === activeProjectId);
  }, [conversations, activeProjectId]);

  const filteredArchivedConversations = useMemo(() => {
    if (!activeProjectId) return archivedConversations;
    return archivedConversations.filter(c => c.project_id === activeProjectId);
  }, [archivedConversations, activeProjectId]);

  const activeProject = activeProjectId ? projects.find((p) => p.id === activeProjectId) ?? null : null;
  const activeProjectLabel = activeProject ? projectLabel(activeProject) : null;
  const scopedActiveCount = filteredConversations.length;
  const scopedArchivedCount = filteredArchivedConversations.length;

  useEffect(() => {
    if (!activeSlug) {
      lastProjectFilterRevealSlugRef.current = null;
      return;
    }
    if (lastProjectFilterRevealSlugRef.current === activeSlug) return;

    const activeConversation = [...conversations, ...archivedConversations]
      .find((c) => c.slug === activeSlug);
    if (!activeConversation) return;

    if (activeProjectId && activeConversation.project_id !== activeProjectId) {
      setActiveProjectId(null);
    }
    lastProjectFilterRevealSlugRef.current = activeSlug;
  }, [activeProjectId, activeSlug, conversations, archivedConversations, setActiveProjectId]);

  useEffect(() => {
    if (!activeSlug) {
      lastArchiveRevealSlugRef.current = null;
      return;
    }
    if (lastArchiveRevealSlugRef.current === activeSlug) return;

    const inActiveList = conversations.some((c) => c.slug === activeSlug);
    const inArchivedList = archivedConversations.some((c) => c.slug === activeSlug);
    if (!inActiveList && !inArchivedList) return;

    if (inArchivedList && !inActiveList && !showArchived) {
      setShowArchived(true);
    } else if (inActiveList && showArchived) {
      setShowArchived(false);
    }
    lastArchiveRevealSlugRef.current = activeSlug;
  }, [activeSlug, conversations, archivedConversations, showArchived]);

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

  const handleArchiveChain = useCallback(async (rootId: string) => {
    try {
      await api.archiveChain(rootId);
      onConversationCreated();
    } catch (err) {
      console.error('Failed to archive chain:', err);
    }
  }, [onConversationCreated]);

  const requestDeleteChain = useCallback(async (rootId: string) => {
    try {
      const view = await api.getChain(rootId);
      setDeleteChainTarget(view);
    } catch (err) {
      console.error('Failed to load chain for delete:', err);
    }
  }, []);

  const handleDeleteChain = useCallback(async () => {
    if (!deleteChainTarget) return;
    try {
      await api.deleteChain(deleteChainTarget.root_conv_id);
      setDeleteChainTarget(null);
      onConversationCreated();
    } catch (err) {
      console.error('Failed to delete chain:', err);
    }
  }, [deleteChainTarget, onConversationCreated]);

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
  const collapsedConversations = collapsedDotConversations(conversations, activeSlug);
  const collapsedOverflowCount = Math.max(0, conversations.length - collapsedConversations.length);

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
        <SettingsDropdown
          theme={theme}
          onToggleTheme={toggleTheme}
          codexPreflight={codexPreflight}
          onPreflightInvalidated={refetchCodexPreflight}
          compact
        />
        <div className="sidebar-collapsed-dots">
          {collapsedConversations.map(conv => {
            const displayState = getConvDisplayState(conv);
            const isActive = conv.slug === activeSlug;
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
        <SettingsDropdown
          theme={theme}
          onToggleTheme={toggleTheme}
          codexPreflight={codexPreflight}
          onPreflightInvalidated={refetchCodexPreflight}
        />
      </div>
      {projects.length > 0 && (
        <div className="sidebar-project-scope" aria-label="Project scope">
          <div className="sidebar-section-label">Projects</div>
          <div className="project-tabs">
            <button
              className={`project-tab ${activeProjectId === null ? 'active' : ''}`}
              onClick={() => setActiveProjectId(null)}
              aria-pressed={activeProjectId === null}
            >
              <span>All</span>
              <span className="project-tab-count">{conversations.length}</span>
            </button>
            {projects.map(p => {
              const label = projectLabel(p);
              return (
                <button
                  key={p.id}
                  className={`project-tab ${activeProjectId === p.id ? 'active' : ''}`}
                  onClick={() => setActiveProjectId(p.id)}
                  title={p.canonical_path}
                  aria-pressed={activeProjectId === p.id}
                >
                  <span>{label}</span>
                  <span className="project-tab-count">{countForProject(conversations, p.id)}</span>
                </button>
              );
            })}
          </div>
        </div>
      )}
      <div className="sidebar-lifecycle-tabs" aria-label="Conversation lifecycle">
        <button
          type="button"
          className={`sidebar-lifecycle-tab ${!showArchived ? 'active' : ''}`}
          onClick={() => { if (showArchived) handleToggleArchived(); }}
          aria-pressed={!showArchived}
        >
          <span>Active</span>
          <span className="sidebar-lifecycle-count">{scopedActiveCount}</span>
        </button>
        <button
          type="button"
          className={`sidebar-lifecycle-tab ${showArchived ? 'active' : ''}`}
          onClick={() => { if (!showArchived) handleToggleArchived(); }}
          aria-pressed={showArchived}
        >
          <span>Archived</span>
          <span className="sidebar-lifecycle-count">{scopedArchivedCount}</span>
        </button>
      </div>
      <LocalServicesPanel />
      <div className="sidebar-list">
        <ConversationList
          conversations={filteredConversations}
          archivedConversations={filteredArchivedConversations}
          showArchived={showArchived}
          onToggleArchived={handleToggleArchived}
          onNewConversation={handleNewClick}
          onArchive={handleArchive}
          onDelete={handleSetDeleteTarget}
          onRename={handleSetRenameTarget}
          onArchiveChain={handleArchiveChain}
          onDeleteChain={requestDeleteChain}
          onConversationClick={handleConversationClick}
          activeSlug={activeSlug}
          sidebarMode
          emptyScopeLabel={activeProjectLabel}
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
      <ChainDeleteConfirm
        visible={deleteChainTarget !== null}
        chain={deleteChainTarget}
        onConfirm={handleDeleteChain}
        onCancel={() => setDeleteChainTarget(null)}
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
