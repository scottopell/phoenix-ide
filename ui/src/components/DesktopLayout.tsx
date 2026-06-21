import { useLocation } from 'react-router-dom';
import { lazy, Suspense, useEffect, useLayoutEffect, useCallback, useRef } from 'react';
import {
  useConversationsList,
  useConversationsRefresh,
  useConversationSnapshot,
  useWorkScope,
} from '../conversation';
import { useResizablePane, useIsDesktop } from '../hooks';
import { Sidebar } from './Sidebar';
import { FileExplorerPanel, FileExplorerProvider } from './FileExplorer';
import { ViewerSlotProvider } from '../contexts/ViewerSlotContext';
import { SubAgentViewerProvider, useSubAgentViewer } from '../contexts/SubAgentViewerContext';
// Code-split: the panel pulls MessageComponents (markdown + syntax highlighting),
// which must not land in the non-lazy app-shell bundle for routes that never
// open a sub-agent. Loaded on first open.
const SubAgentViewerPanel = lazy(() =>
  import('./SubAgentViewerPanel').then((m) => ({ default: m.SubAgentViewerPanel })),
);
import { CommandPalette } from './CommandPalette';
import { Toast } from './Toast';
import { PaneDivider } from './PaneDivider';
import { useToast } from '../hooks/useToast';
import {
  closeNotificationsForConversation,
  consumeNotificationPermissionCue,
  loadNotificationSettingsAndCatchUp,
  notifyCatchUp,
  useNotificationClickNavigationBridge,
} from '../notifications';

const subAgentViewerPaneMax = () => Math.max(360, Math.round(window.innerWidth * 0.6));

interface DesktopLayoutProps {
  children: React.ReactNode;
}

/**
 * Right-docked sub-agent viewer. Owns its own resizable width and renders
 * nothing until a sub-agent is opened. A descendant of SubAgentViewerProvider,
 * mounted only on desktop so the panel never appears where there's no room.
 */
function SubAgentViewerDock() {
  const viewer = useSubAgentViewer();
  const pane = useResizablePane({
    key: 'subagent-viewer-width',
    min: 320,
    max: subAgentViewerPaneMax,
    defaultSize: 460,
  });
  if (!viewer?.opened) return null;
  return (
    <>
      <PaneDivider
        orientation="vertical"
        title="Drag to resize • Double-click to close"
        onPointerDown={(e) => pane.startDrag(e, 'x', true)}
        onDoubleClick={viewer.close}
      />
      <Suspense fallback={null}>
        {/* Key by agentId so switching sub-agents remounts with a fresh stream
            instead of showing the prior agent's transcript under the new title. */}
        <SubAgentViewerPanel
          key={viewer.opened.agentId}
          opened={viewer.opened}
          onClose={viewer.close}
          width={pane.size}
        />
      </Suspense>
    </>
  );
}

export function DesktopLayout({ children }: DesktopLayoutProps) {
  const isDesktop = useIsDesktop();
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

  // Live-drag channels for the sidebar and file-explorer dividers. Both panes'
  // state lives here, so committing it on every pointer move would re-render the
  // whole layout — including the (heavy) sidebar conversation list and file
  // tree — at pointer frequency. Instead each panel's width is read from a CSS
  // variable on `.desktop-layout` that two non-concurrent writers own: a layout
  // effect synced to committed state, and the divider's `onLiveResize`. The
  // variable lives on the ancestor (not the panel's React `style` prop), so an
  // unrelated mid-drag re-render (the 5s conversation poll, SSE) cannot clobber
  // the live width — the effect's deps are frozen until the drag commits on
  // pointer-up. Collapse still commits on pointer-up (the collapsed rail is a
  // different render); the width simply tracks to its clamped minimum first.
  const layoutRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    layoutRef.current?.style.setProperty('--sidebar-pane-width', `${sidebarPane.size}px`);
  }, [sidebarPane.size]);
  useLayoutEffect(() => {
    layoutRef.current?.style.setProperty('--file-explorer-pane-width', `${fileExplorerPane.size}px`);
  }, [fileExplorerPane.size]);
  const handleSidebarLiveResize = useCallback((size: number) => {
    layoutRef.current?.style.setProperty('--sidebar-pane-width', `${size}px`);
  }, []);
  const handleFileExplorerLiveResize = useCallback((size: number) => {
    layoutRef.current?.style.setProperty('--file-explorer-pane-width', `${size}px`);
  }, []);

  const location = useLocation();
  const { toasts, dismissToast, showSuccess, showError, showInfo } = useToast();
  useNotificationClickNavigationBridge();

  // Task 08684: ConversationStore is the single source of truth.
  // The store-owned `useConversationsRefresh` (mounted in
  // ConversationProvider) handles the 5s poll + cache + online +
  // hard-delete cascade. This layout reads the derived list and the
  // per-slug active row directly off the store — no parallel
  // `Conversation[]` state, no per-field bridge hooks.
  const { refresh: refreshConversations } = useConversationsRefresh();
  const { active: conversations, archived: archivedConversations } = useConversationsList();

  useEffect(() => {
    void loadNotificationSettingsAndCatchUp(conversations).catch(() => {});
  }, [conversations]);

  useEffect(() => {
    notifyCatchUp(conversations);
  }, [conversations]);

  useEffect(() => {
    const handler = () => {
      if (document.visibilityState === 'visible' && consumeNotificationPermissionCue()) {
        showInfo('Enable desktop notifications in notification settings to be pulled back when the agent needs you.', 8000);
      }
    };
    document.addEventListener('visibilitychange', handler);
    window.addEventListener('focus', handler);
    handler();
    return () => {
      document.removeEventListener('visibilitychange', handler);
      window.removeEventListener('focus', handler);
    };
  }, [showInfo]);

  // Extract active slug. `useConversationSnapshot` reads the row directly
  // from the store — polling and SSE both write through the same atom,
  // so this is the single source of truth for the active conversation.
  // Returns null until polling or SSE init has populated the row
  // (typically one tick after navigation; `if (!conversation)` callers
  // downstream paint a skeleton during that window).
  const slugMatch = location.pathname.match(/^\/c\/(.+)$/);
  const activeSlug = slugMatch?.[1] ?? null;
  const activeConversation = useConversationSnapshot(activeSlug);
  const activeConversationId = activeConversation?.id;
  // Live work-scope inventory (SSE-fed) for the active conversation, threaded
  // into FileExplorerPanel's Work scope section + collapsed-rail badge
  // (REQ-WSUI-010). Single-writer atom contract: only the SSE reducer writes
  // `workScope`; the section's initial fetch seeds local state, not the atom.
  const liveWorkScope = useWorkScope(activeSlug);

  useEffect(() => {
    if (activeConversationId) {
      closeNotificationsForConversation(activeConversationId);
    }
  }, [activeConversationId]);

  const effectiveCwd = activeConversation?.worktree_path ?? activeConversation?.cwd ?? '/';

  // Always render a single stable tree so children never unmounts across the
  // desktop/mobile breakpoint. Conditionally show sidebar and file-explorer
  // panel via isDesktop — children stays in the same tree position throughout.
  // See task 08664: previously the early-return on !isDesktop produced a
  // different React tree, unmounting ConversationPage and resetting its state.
  return (
    <SubAgentViewerProvider>
    <ViewerSlotProvider
      scopeKey={activeSlug ?? undefined}
      browserSessionActive={activeConversation?.browser_session_active ?? false}
    >
     <FileExplorerProvider>
      <div ref={layoutRef} className={isDesktop ? 'desktop-layout' : undefined}>
        {isDesktop && (
          <Sidebar
            collapsed={sidebarPane.collapsed}
            onToggle={() => sidebarPane.setCollapsed(!sidebarPane.collapsed)}
            conversations={conversations}
            archivedConversations={archivedConversations}
            activeSlug={activeSlug}
            onConversationCreated={() => refreshConversations()}
            width={sidebarPane.collapsed ? undefined : sidebarPane.size}
          />
        )}
        {isDesktop && (
          <PaneDivider
            orientation="vertical"
            title="Drag to resize • Drag past edge to collapse"
            onPointerDown={(e) => sidebarPane.startDrag(e, 'x', false, handleSidebarLiveResize)}
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
            showError={showError}
            branchName={activeConversation?.branch_name}
            activeSlug={activeSlug}
            width={fileExplorerPane.collapsed ? undefined : fileExplorerPane.size}
            workScopeKey={activeConversation?.work_scope_key}
            liveWorkScope={liveWorkScope}
          />
        )}
        {isDesktop && activeSlug && (
          <PaneDivider
            orientation="vertical"
            title="Drag to resize • Drag past edge to collapse"
            onPointerDown={(e) => fileExplorerPane.startDrag(e, 'x', false, handleFileExplorerLiveResize)}
            onDoubleClick={() => fileExplorerPane.setCollapsed(!fileExplorerPane.collapsed)}
          />
        )}
        {/* children is always at this position — never remounts on breakpoint crossing */}
        <div className={isDesktop ? 'desktop-main' : undefined}>
          {children}
        </div>
        {isDesktop && <SubAgentViewerDock />}
        {isDesktop && <CommandPalette conversations={conversations} activeConversation={activeConversation} />}
        <Toast messages={toasts} onDismiss={dismissToast} />
      </div>
     </FileExplorerProvider>
    </ViewerSlotProvider>
    </SubAgentViewerProvider>
  );
}
