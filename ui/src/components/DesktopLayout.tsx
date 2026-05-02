import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { useLocation } from 'react-router-dom';
import { api } from '../api';
import type { Conversation } from '../api';
import { cacheDB } from '../cache';
import { useResizablePane } from '../hooks';
import { useConversationCwd } from '../conversation';
import { Sidebar } from './Sidebar';
import { FileExplorerPanel, FileExplorerProvider } from './FileExplorer';
import { CommandPalette } from './CommandPalette';
import { Toast } from './Toast';
import { PaneDivider } from './PaneDivider';
import { useToast } from '../hooks/useToast';

interface DesktopLayoutProps {
  children: React.ReactNode;
}

export function DesktopLayout({ children }: DesktopLayoutProps) {
  const [isDesktop, setIsDesktop] = useState(() => window.matchMedia('(min-width: 1025px)').matches);
  const sidebarPane = useResizablePane({
    key: 'sidebar-width',
    min: 160,
    max: 500,
    defaultSize: 280,
    collapseThreshold: 120,
  });
  const fileExplorerPane = useResizablePane({
    key: 'file-explorer-width',
    min: 160,
    max: 450,
    defaultSize: 220,
    collapseThreshold: 120,
  });
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [archivedConversations, setArchivedConversations] = useState<Conversation[]>([]);
  const location = useLocation();
  const { toasts, dismissToast, showSuccess } = useToast();
  const loadingRef = useRef(false);

  // Media query listener
  useEffect(() => {
    const mq = window.matchMedia('(min-width: 1025px)');
    const handler = (e: MediaQueryListEvent) => setIsDesktop(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  // Load conversations for sidebar
  const loadConversations = useCallback(async (silent = false) => {
    if (loadingRef.current && silent) return;
    loadingRef.current = true;
    try {
      const cached = await cacheDB.getAllConversations();
      if (cached.length > 0) {
        setConversations(cached.filter(c => !c.archived));
        setArchivedConversations(cached.filter(c => c.archived));
      }
      if (navigator.onLine) {
        const [freshActive, freshArchived] = await Promise.all([
          api.listConversations(),
          api.listArchivedConversations(),
        ]);
        setConversations(freshActive);
        setArchivedConversations(freshArchived);
        if (!silent) {
          await cacheDB.syncConversations([...freshActive, ...freshArchived]);
        }
      }
    } catch {
      // silent
    } finally {
      loadingRef.current = false;
    }
  }, []);

  // Initial load + periodic refresh
  useEffect(() => {
    if (!isDesktop) return;
    loadConversations();
    const interval = setInterval(() => {
      if (document.visibilityState === 'visible' && navigator.onLine) {
        loadConversations(true);
      }
    }, 5000);
    return () => clearInterval(interval);
  }, [isDesktop, loadConversations]);

  // Refresh after navigating
  useEffect(() => {
    if (isDesktop) loadConversations(true);
  }, [location.pathname, isDesktop, loadConversations]);

  // Extract active slug and find active conversation
  const slugMatch = location.pathname.match(/^\/c\/(.+)$/);
  const activeSlug = slugMatch?.[1] ?? null;
  const activeConversation = useMemo(
    () => conversations.find(c => c.slug === activeSlug) ?? null,
    [conversations, activeSlug],
  );

  // Active-conversation cwd reactivity: the periodic poll
  // (`loadConversations` every 5s) eventually picks up cwd transitions
  // (Explore → Work after task approval, complete/abandon revert) but
  // with up to ~5s of lag, leaving the file explorer stuck on the old
  // root. Subscribe to the conversation atom for the active slug —
  // that's updated immediately by `sse_conversation_update` events the
  // backend emits when it mutates `cwd`. Atom value wins; poll is the
  // fallback on first render before the SSE init lands. Task 08612.
  //
  // `useConversationCwd` is a selector hook: it only re-renders when
  // the cwd string actually changes. Subscribing to the full atom would
  // re-render on every `sse_token` because tokens churn the atom's
  // `streamingBuffer` field, even though cwd never moves during streaming.
  const liveCwd = useConversationCwd(activeSlug);
  const effectiveCwd = liveCwd ?? activeConversation?.cwd ?? '/';

  // Always render a single stable tree so children never unmounts across the
  // desktop/mobile breakpoint. Conditionally show sidebar and file-explorer
  // panel via isDesktop — children stays in the same tree position throughout.
  // See task 08664: previously the early-return on !isDesktop produced a
  // different React tree, unmounting ConversationPage and resetting its state.
  return (
    <FileExplorerProvider>
      <div className={isDesktop ? 'desktop-layout' : undefined}>
        {isDesktop && (
          <Sidebar
            collapsed={sidebarPane.collapsed}
            onToggle={() => sidebarPane.setCollapsed(!sidebarPane.collapsed)}
            conversations={conversations}
            archivedConversations={archivedConversations}
            activeSlug={activeSlug}
            onConversationCreated={() => loadConversations(true)}
            width={sidebarPane.collapsed ? undefined : sidebarPane.size}
          />
        )}
        {isDesktop && (
          <PaneDivider
            orientation="vertical"
            title="Drag to resize • Drag past edge to collapse"
            onPointerDown={(e) => sidebarPane.startDrag(e, 'x')}
            onDoubleClick={() => sidebarPane.setCollapsed(!sidebarPane.collapsed)}
          />
        )}
        {isDesktop && activeSlug && (
          <FileExplorerPanel
            collapsed={fileExplorerPane.collapsed}
            onToggle={() => fileExplorerPane.setCollapsed(!fileExplorerPane.collapsed)}
            rootPath={effectiveCwd}
            conversationId={activeConversation?.id}
            showToast={showSuccess}
            branchName={activeConversation?.branch_name}
            parentConversation={activeConversation}
            width={fileExplorerPane.collapsed ? undefined : fileExplorerPane.size}
          />
        )}
        {isDesktop && activeSlug && (
          <PaneDivider
            orientation="vertical"
            title="Drag to resize • Drag past edge to collapse"
            onPointerDown={(e) => fileExplorerPane.startDrag(e, 'x')}
            onDoubleClick={() => fileExplorerPane.setCollapsed(!fileExplorerPane.collapsed)}
          />
        )}
        {/* children is always at this position — never remounts on breakpoint crossing */}
        <div className={isDesktop ? 'desktop-main' : undefined}>
          {children}
        </div>
        {isDesktop && <CommandPalette conversations={conversations} />}
        <Toast messages={toasts} onDismiss={dismissToast} />
      </div>
    </FileExplorerProvider>
  );
}
