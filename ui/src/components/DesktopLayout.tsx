import { useState, useEffect, useCallback, useRef } from 'react';
import { useLocation } from 'react-router-dom';
import { api } from '../api';
import type { Conversation } from '../api';
import { cacheDB } from '../cache';
import { useLocalStorage } from '../hooks';
import { Sidebar } from './Sidebar';
import { Toast } from './Toast';
import { useToast } from '../hooks/useToast';

interface DesktopLayoutProps {
  children: React.ReactNode;
}

export function DesktopLayout({ children }: DesktopLayoutProps) {
  const [isDesktop, setIsDesktop] = useState(() => window.matchMedia('(min-width: 1025px)').matches);
  const [collapsed, setCollapsed] = useLocalStorage('sidebar-collapsed', false);
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
      // Cache first
      const cached = await cacheDB.getAllConversations();
      if (cached.length > 0) {
        setConversations(cached.filter(c => !c.archived));
        setArchivedConversations(cached.filter(c => c.archived));
      }
      // Then fresh from network
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

  // Initial load + periodic refresh for state indicators
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

  // Refresh after navigating (e.g. new conversation created)
  useEffect(() => {
    if (isDesktop) loadConversations(true);
  }, [location.pathname, isDesktop, loadConversations]);

  if (!isDesktop) {
    return <>{children}</>;
  }

  // Extract active slug from URL
  const slugMatch = location.pathname.match(/^\/c\/(.+)$/);
  const activeSlug = slugMatch?.[1] ?? null;

  return (
    <div className="desktop-layout">
      <Sidebar
        collapsed={collapsed}
        onToggle={() => setCollapsed(!collapsed)}
        conversations={conversations}
        archivedConversations={archivedConversations}
        activeSlug={activeSlug}
        onConversationCreated={() => loadConversations(true)}
        showToast={showSuccess}
      />
      <div className="desktop-main">
        {children}
      </div>
      <Toast messages={toasts} onDismiss={dismissToast} />
    </div>
  );
}
