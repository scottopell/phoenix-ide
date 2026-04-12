import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { useLocation } from 'react-router-dom';
import { api } from '../api';
import type { Conversation } from '../api';
import { cacheDB } from '../cache';
import { useLocalStorage } from '../hooks';
import { Sidebar } from './Sidebar';
import { FileExplorerPanel, FileExplorerProvider } from './FileExplorer';
import { CommandPalette } from './CommandPalette';
import { Toast } from './Toast';
import { useToast } from '../hooks/useToast';

interface DesktopLayoutProps {
  children: React.ReactNode;
}

export function DesktopLayout({ children }: DesktopLayoutProps) {
  const [isDesktop, setIsDesktop] = useState(() => window.matchMedia('(min-width: 1025px)').matches);
  const [sidebarCollapsed, setSidebarCollapsed] = useLocalStorage('sidebar-collapsed', false);
  const [fileExplorerCollapsed, setFileExplorerCollapsed] = useLocalStorage('file-explorer-collapsed', false);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [archivedConversations, setArchivedConversations] = useState<Conversation[]>([]);
  const location = useLocation();
  const { toasts, dismissToast, showSuccess, showError, showWarning, showInfo } = useToast();
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
            collapsed={sidebarCollapsed}
            onToggle={() => setSidebarCollapsed(!sidebarCollapsed)}
            conversations={conversations}
            archivedConversations={archivedConversations}
            activeSlug={activeSlug}
            onConversationCreated={() => loadConversations(true)}
          />
        )}
        {isDesktop && activeSlug && (
          <FileExplorerPanel
            collapsed={fileExplorerCollapsed}
            onToggle={() => setFileExplorerCollapsed(!fileExplorerCollapsed)}
            rootPath={activeConversation?.cwd || '/'}
            conversationId={activeConversation?.id}
            showToast={showSuccess}
            branchName={activeConversation?.branch_name}
          />
        )}
        {/* children is always at this position — never remounts on breakpoint crossing */}
        <div className={isDesktop ? 'desktop-main' : undefined}>
          {children}
        </div>
        {isDesktop && <CommandPalette conversations={conversations} />}
        {/* Debug: toast test triggers */}
        {isDesktop && (
          <div className="toast-debug">
            <button onClick={() => showSuccess('Operation completed', 3000)} title="Test success toast">ok</button>
            <button onClick={() => showError('Something went wrong', 3000)} title="Test error toast">err</button>
            <button onClick={() => showWarning('Approaching limit', 3000)} title="Test warning toast">warn</button>
            <button onClick={() => showInfo('Processing...', 3000)} title="Test info toast">info</button>
          </div>
        )}
        <Toast messages={toasts} onDismiss={dismissToast} />
      </div>
    </FileExplorerProvider>
  );
}
