import { useState, useEffect, useCallback, useLayoutEffect, useRef } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { refreshModels } from '../modelsPoller';
import type { ProductConversationListRow } from '../api';
import { useModels, useAutoAuth, useIsDesktop, useTheme } from '../hooks';
import { useCodexPreflight } from '../hooks/useCodexPreflight';
import {
  useConversationsList,
  useConversationsRefresh,
} from '../conversation';
import { NewConversationPage } from './NewConversationPage';
import { ConversationList } from '../components/ConversationList';
import { coordinatorUnread } from '../conversation/readState';
import {
  refreshProductConversationList,
  useProductConversationList,
} from '../conversation/productConversationList';
import { EmberEntry } from '../components/EmberEntry';
import { ProductConversationActionDialogs } from '../components/ProductConversationActionDialogs';
import { useProductConversationActions } from '../hooks/useProductConversationActions';
import { StorageStatus } from '../components/StorageStatus';

const AlertTriangle = () => (
  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" style={{ verticalAlign: '-4px', marginRight: '8px' }}>
    <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
    <line x1="12" y1="9" x2="12" y2="13" />
    <line x1="12" y1="17" x2="12.01" y2="17" />
  </svg>
);
import { Toast } from '../components/Toast';
import { ConversationListSkeleton } from '../components/Skeleton';
import { useAppMachine } from '../hooks/useAppMachine';
import { useToast } from '../hooks/useToast';
import { CredentialHelperPanel } from '../components/CredentialHelperPanel';
import { SettingsDropdown } from '../components/SettingsDropdown';
import { effectiveVisibleConversationCount } from './conversationListCount';

const MOBILE_LIST_SCROLL_KEY = 'phoenix:mobile-conversation-list-scroll:v1';
const MOBILE_ARCHIVED_LIST_SCROLL_KEY = 'phoenix:mobile-archived-list-scroll:v1';

export function ConversationListPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const isDesktop = useIsDesktop();
  const { theme, toggleTheme } = useTheme();

  // Task 08684: ConversationStore is the single source of truth. The
  // shared `useConversationsRefresh` (mounted in ConversationProvider)
  // owns the cache hydration, the 5s poll, and the cache writeback. This
  // page reads the derived list straight off the store — no parallel
  // useState arrays, no per-page polling timer.
  const { refresh } = useConversationsRefresh();
  const { active: conversations, archived: archivedConversations } = useConversationsList();
  const [showArchived, setShowArchived] = useState(false);
  const productList = useProductConversationList();
  const productConversations = productList.rows;
  const productListError = productList.error;
  const productListAvailable = productList.fresh || productList.fromCache;
  const listAvailability = productListAvailable
    ? 'available'
    : productListError
      ? 'error'
      : 'loading';
  const coordinatorUnreadState = coordinatorUnread(productList);
  const mainRef = useRef<HTMLElement | null>(null);
  const didRestoreScrollRef = useRef(false);
  const currentScrollKey = showArchived ? MOBILE_ARCHIVED_LIST_SCROLL_KEY : MOBILE_LIST_SCROLL_KEY;
  const openProductConversations = productConversations.filter((row) => row.lifecycle.state === 'open');
  const archivedProductConversations = productConversations.filter((row) => row.lifecycle.state === 'history');

  const visibleConversationCount = effectiveVisibleConversationCount({
    showArchived,
    productListError,
    productCount: productConversations.length,
    openProductCount: openProductConversations.length,
    archivedProductCount: archivedProductConversations.length,
    activeMemberCount: conversations.length,
    archivedMemberCount: archivedConversations.length,
  });
  const currentScrollKeyRef = useRef(currentScrollKey);
  currentScrollKeyRef.current = currentScrollKey;

  useEffect(() => {
    if (isDesktop) didRestoreScrollRef.current = false;
  }, [isDesktop]);

  // App state for offline/sync status
  const { isOnline, isReady, initError, pendingOpsCount } = useAppMachine();
  const { toasts, dismissToast, showWarning, showError } = useToast();

  const [hasCompletedFirstFetch, setHasCompletedFirstFetch] = useState(false);
  useEffect(() => {
    if (!isReady) return;
    let cancelled = false;
    void refresh().then(() => {
      if (!cancelled) setHasCompletedFirstFetch(true);
    });
    return () => {
      cancelled = true;
    };
  }, [isReady, refresh]);
  const loading = !productListAvailable && productListError === null;
  const productActions = useProductConversationActions();

  const { credentialStatus } = useModels();
  const { showAuthPanel, setShowAuthPanel } = useAutoAuth(credentialStatus);
  const { codexPreflight, refetchCodexPreflight } = useCodexPreflight(true);

  // Listen for storage warnings
  useEffect(() => {
    const handleStorageWarning = (event: Event) => {
      const customEvent = event as CustomEvent;
      const { usageMB } = customEvent.detail;
      showWarning(`Storage usage is high: ${usageMB.toFixed(1)}MB. Consider clearing old data.`, 10000);
    };

    const handleQuotaExceeded = () => {
      showError('Storage quota exceeded! Old conversations are being cleaned up automatically.', 8000);
    };

    window.addEventListener('storage-warning', handleStorageWarning);
    window.addEventListener('storage-quota-exceeded', handleQuotaExceeded);
    return () => {
      window.removeEventListener('storage-warning', handleStorageWarning);
      window.removeEventListener('storage-quota-exceeded', handleQuotaExceeded);
    };
  }, [showWarning, showError]);

  // Removed: per-page loadConversations + periodic refresh. The shared
  // `useConversationsRefresh` (mounted in ConversationProvider) owns the
  // cache hydration, polling, online listener, and hard-delete cascade.
  // This page calls `refresh()` after mutations that need an immediate
  // resync, but never holds its own conversation arrays.

  const saveScrollPositionForKey = useCallback((scrollKey: string, scrollOwner = mainRef.current) => {
    if (!scrollOwner) return;
    try {
      localStorage.setItem(scrollKey, String(scrollOwner.scrollTop));
    } catch (error) {
      console.warn('Unable to save the mobile conversation-list scroll position', error);
    }
  }, []);

  const saveScrollPosition = useCallback(() => {
    saveScrollPositionForKey(currentScrollKey);
  }, [currentScrollKey, saveScrollPositionForKey]);

  const setMainScrollOwner = useCallback((node: HTMLElement | null) => {
    if (mainRef.current && !node) {
      saveScrollPositionForKey(currentScrollKeyRef.current, mainRef.current);
    }
    mainRef.current = node;
  }, [saveScrollPositionForKey]);

  useLayoutEffect(() => {
    didRestoreScrollRef.current = false;
  }, [currentScrollKey]);

  useLayoutEffect(() => {
    if (isDesktop || loading || !hasCompletedFirstFetch || didRestoreScrollRef.current || visibleConversationCount === 0) return;
    const scrollOwner = mainRef.current;
    if (!scrollOwner) return;
    try {
      const saved = Number(localStorage.getItem(currentScrollKey));
      if (Number.isFinite(saved)) {
        didRestoreScrollRef.current = true;
        const maxScrollTop = Math.max(0, scrollOwner.scrollHeight - scrollOwner.clientHeight);
        scrollOwner.scrollTop = Math.min(Math.max(0, saved), maxScrollTop);
      }
    } catch (error) {
      console.warn('Unable to restore the mobile conversation-list scroll position', error);
    }
  }, [currentScrollKey, hasCompletedFirstFetch, isDesktop, loading, visibleConversationCount]);

  useLayoutEffect(() => {
    const handleVisibilityChange = () => {
      if (document.visibilityState === 'hidden') saveScrollPosition();
    };
    document.addEventListener('visibilitychange', handleVisibilityChange);
    return () => document.removeEventListener('visibilitychange', handleVisibilityChange);
  }, [isDesktop, saveScrollPosition]);

  const handleProductConversationClick = useCallback((row: ProductConversationListRow) => {
    saveScrollPosition();
    navigate(row.canonical_route);
  }, [navigate, saveScrollPosition]);

  const handleNewConversation = () => {
    navigate('/new');
  };

  const handleToggleArchived = useCallback(() => {
    saveScrollPosition();
    setShowArchived((prev) => !prev);
  }, [saveScrollPosition]);



  // On desktop, the sidebar handles the conversation list.
  // Root route shows the new conversation form in main content.
  if (isDesktop) {
    return <NewConversationPage desktopMode />;
  }

  // Show error UI if IndexedDB init failed
  if (initError) {
    return (
      <div id="app" className="list-page">
        <div className="conversation-list-pinned-header">
          <EmberEntry unread={coordinatorUnreadState} onClick={() => navigate('/global')} />
        </div>
        <main className="init-error" data-app-scroll-owner>
          <h2><AlertTriangle />Storage Error</h2>
          <p>Failed to initialize local storage: {initError}</p>
          <p>Please try refreshing the page. If the problem persists, try clearing your browser data for this site.</p>
          <button onClick={() => window.location.reload()} title="Reload this page">Refresh Page</button>
        </main>
      </div>
    );
  }

  const totalConversations = productConversations.length;

  const authChip = credentialStatus && credentialStatus !== 'not_configured' ? (
    <button
      className={`auth-chip ${
        credentialStatus === 'valid' ? 'valid' :
        credentialStatus === 'running' ? 'running' :
        'required'
      }`}
      onClick={credentialStatus === 'required' || credentialStatus === 'failed'
        ? () => setShowAuthPanel(true)
        : undefined}
      disabled={credentialStatus === 'valid' || credentialStatus === 'running'}
      title={credentialStatus === 'valid'
        ? 'Authentication is valid'
        : credentialStatus === 'running'
          ? 'Authentication check is running'
          : 'Open authentication helper'}
      aria-label={credentialStatus === 'valid'
        ? 'Authentication is valid'
        : credentialStatus === 'running'
          ? 'Authentication check is running'
          : 'Open authentication helper'}
    >
      {credentialStatus === 'valid' ? 'AUTH \u2713' :
       credentialStatus === 'running' ? 'AUTH ...' :
       'AUTH \u2717'}
    </button>
  ) : undefined;

  return (
    <div id="app" className="list-page">
      <Toast messages={toasts} onDismiss={dismissToast} />
      {!isOnline && (
        <div className="offline-banner">
          Offline
          {pendingOpsCount > 0 && ` · ${pendingOpsCount} pending`}
        </div>
      )}
      <main id="main-area" ref={setMainScrollOwner} data-app-scroll-owner>
        {loading ? (
          <section id="conversation-list" className="view active">
            <div className="conversation-list-pinned-header">
            <div className="view-header conversation-list-header">
              <h2>Conversations</h2>
              <div className="view-header-actions">
                {authChip}
                <button className="btn-primary" disabled title="Loading conversations">+ New</button>
              </div>
            </div>
              <EmberEntry unread={coordinatorUnreadState} active={location.pathname === '/global'} onClick={() => navigate('/global')} />
            </div>
            <ConversationListSkeleton count={5} />
          </section>
        ) : (
          <>
            {productListError && (
              <div role="status" className="coordinator-error">
                <span>{productList.fromCache
                  ? `Showing cached conversations — ${productListError}`
                  : productList.fresh
                    ? `Showing last loaded conversations — ${productListError}`
                    : `Unable to load conversations — ${productListError}`}</span>
                <button type="button" onClick={() => void refreshProductConversationList()}>Retry</button>
              </div>
            )}
            <ConversationList
              coordinatorUnread={coordinatorUnreadState}
              listAvailability={listAvailability}
              productConversations={openProductConversations}
              archivedProductConversations={archivedProductConversations}
              showArchived={showArchived}
              onToggleArchived={handleToggleArchived}
              onNewConversation={handleNewConversation}
              onProductConversationClick={handleProductConversationClick}
              onProductConversationRename={productActions.openRename}
              {...(isOnline ? { onProductConversationClose: productActions.openClose } : {})}
              {...(isOnline ? { onProductConversationDelete: productActions.openDelete } : {})}
              listDensity={isDesktop ? 'full' : 'mobile'}
              authChip={authChip}
              utilityActions={(
                <SettingsDropdown
                  theme={theme}
                  onToggleTheme={toggleTheme}
                  codexPreflight={codexPreflight}
                  onPreflightInvalidated={() => {
                    refetchCodexPreflight();
                    void refreshModels();
                  }}
                  compact
                />
              )}
              footer={<StorageStatus conversationCount={totalConversations} />}
            />
          </>
        )}
      </main>
      <ProductConversationActionDialogs actions={productActions} />

      {showAuthPanel && credentialStatus && credentialStatus !== 'not_configured' && credentialStatus !== 'valid' && (
        <CredentialHelperPanel
          active={showAuthPanel}
          onDismiss={() => {
            setShowAuthPanel(false);
            void refreshModels().catch(() => {});
          }}
        />
      )}
    </div>
  );
}
