import { useState, useEffect, useMemo } from 'react';
import { useLocation } from 'react-router-dom';
import {
  useConversationsList,
  useConversationsRefresh,
  useConversationByActiveSlug,
} from '../conversation';
import { useResizablePane } from '../hooks';
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
  const location = useLocation();
  const { toasts, dismissToast, showSuccess } = useToast();

  // Task 08684: ConversationStore is the single source of truth.
  // The store-owned `useConversationsRefresh` (mounted in
  // ConversationProvider) handles the 5s poll + cache + online +
  // hard-delete cascade. This layout reads the derived list and the
  // per-slug active row directly off the store — no parallel
  // `Conversation[]` state, no per-field bridge hooks.
  const { refresh: refreshConversations } = useConversationsRefresh();
  const { active: conversations, archived: archivedConversations } = useConversationsList();

  // Media query listener
  useEffect(() => {
    const mq = window.matchMedia('(min-width: 1025px)');
    const handler = (e: MediaQueryListEvent) => setIsDesktop(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  // Extract active slug and find active conversation. Reading
  // `useConversationByActiveSlug(activeSlug)` subscribes only to that
  // slug's atom — token streaming on a non-active conversation does
  // not re-render this layout. Returns null until the SSE init or a
  // poll has populated the row.
  const slugMatch = location.pathname.match(/^\/c\/(.+)$/);
  const activeSlug = slugMatch?.[1] ?? null;
  const activeConversationFromAtom = useConversationByActiveSlug(activeSlug);
  // Fallback to scanning the derived list while the per-slug atom is
  // still empty (e.g. first paint after navigation, before SSE init
  // has landed). Both paths read the same store so they cannot diverge.
  const activeConversation = useMemo(
    () => activeConversationFromAtom ?? conversations.find((c) => c.slug === activeSlug) ?? null,
    [activeConversationFromAtom, conversations, activeSlug],
  );

  const effectiveCwd = activeConversation?.cwd ?? '/';

  // Always render a single stable tree so children never unmounts across the
  // desktop/mobile breakpoint. Conditionally show sidebar and file-explorer
  // panel via isDesktop — children stays in the same tree position throughout.
  // See task 08664: previously the early-return on !isDesktop produced a
  // different React tree, unmounting ConversationPage and resetting its state.
  return (
    <FileExplorerProvider scopeKey={activeSlug ?? undefined}>
      <div className={isDesktop ? 'desktop-layout' : undefined}>
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
