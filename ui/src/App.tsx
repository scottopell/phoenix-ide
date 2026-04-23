import { lazy, Suspense, useState, useEffect, useCallback } from 'react';
import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { DesktopLayout } from './components/DesktopLayout';
import { ShortcutHelpPanel } from './components/ShortcutHelpPanel';
import { useGlobalKeyboardShortcuts, FocusScopeProvider, ThemeProvider } from './hooks';
import { ConversationProvider } from './conversation';
import { api } from './api';
import './index.css';

// Routes are code-split so the initial bundle only contains what the user
// actually needs to view the current page. Heavy dependencies that live in
// specific routes (react-syntax-highlighter, xterm, react-markdown) stay out
// of the main chunk until that route mounts.
const ConversationListPage = lazy(() =>
  import('./pages/ConversationListPage').then((m) => ({ default: m.ConversationListPage })),
);
const ConversationPage = lazy(() =>
  import('./pages/ConversationPage').then((m) => ({ default: m.ConversationPage })),
);
const NewConversationPage = lazy(() =>
  import('./pages/NewConversationPage').then((m) => ({ default: m.NewConversationPage })),
);
const LoginPage = lazy(() =>
  import('./pages/LoginPage').then((m) => ({ default: m.LoginPage })),
);
const SharePage = lazy(() =>
  import('./pages/SharePage').then((m) => ({ default: m.SharePage })),
);

/** Route loading fallback — blank div sized to the viewport to avoid CLS. */
function RouteFallback() {
  return <div style={{ minHeight: '100vh' }} />;
}

type AuthState =
  | { status: 'checking' }
  | { status: 'authenticated' }
  | { status: 'login_required' };

// Wrapper component to use hooks inside router context
function AppRoutes() {
  useGlobalKeyboardShortcuts();
  const [showHelp, setShowHelp] = useState(false);

  useEffect(() => {
    const handler = () => setShowHelp((prev) => !prev);
    window.addEventListener('toggle-shortcut-help', handler);
    return () => window.removeEventListener('toggle-shortcut-help', handler);
  }, []);

  return (
    <>
      <Suspense fallback={<RouteFallback />}>
        <Routes>
          {/* Share view: minimal layout, no sidebar, no auth required */}
          <Route path="/s/:token" element={<SharePage />} />
          {/* Main app routes: full layout with sidebar */}
          <Route path="*" element={
            <DesktopLayout>
              <Routes>
                <Route path="/" element={<ConversationListPage />} />
                <Route path="/new" element={<NewConversationPage />} />
                <Route path="/c/:slug" element={<ConversationPage />} />
              </Routes>
            </DesktopLayout>
          } />
        </Routes>
      </Suspense>
      <ShortcutHelpPanel visible={showHelp} onClose={() => setShowHelp(false)} />
    </>
  );
}

function App() {
  const [authState, setAuthState] = useState<AuthState>({ status: 'checking' });

  useEffect(() => {
    // Share pages are auth-exempt -- skip the check entirely so we don't
    // flash a login screen while the /api/auth/status round-trip resolves.
    if (window.location.pathname.startsWith('/s/')) {
      setAuthState({ status: 'authenticated' });
      return;
    }

    let cancelled = false;
    api.authStatus().then((result) => {
      if (cancelled) return;
      if (result.auth_required && !result.authenticated) {
        setAuthState({ status: 'login_required' });
      } else {
        setAuthState({ status: 'authenticated' });
      }
    }).catch(() => {
      // If we can't reach the server, show the app and let normal error
      // handling surface the connection issue
      if (!cancelled) setAuthState({ status: 'authenticated' });
    });
    return () => { cancelled = true; };
  }, []);

  const handleLoginSuccess = useCallback(() => {
    setAuthState({ status: 'authenticated' });
  }, []);

  if (authState.status === 'checking') {
    return <ThemeProvider>{null}</ThemeProvider>;
  }

  if (authState.status === 'login_required') {
    return (
      <ThemeProvider>
        <Suspense fallback={<RouteFallback />}>
          <LoginPage onSuccess={handleLoginSuccess} />
        </Suspense>
      </ThemeProvider>
    );
  }

  return (
    <ThemeProvider>
      <BrowserRouter>
        <FocusScopeProvider>
          <ConversationProvider>
            <AppRoutes />
          </ConversationProvider>
        </FocusScopeProvider>
      </BrowserRouter>
    </ThemeProvider>
  );
}

export default App;
