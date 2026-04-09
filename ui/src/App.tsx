import { useState, useEffect, useCallback } from 'react';
import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { ConversationListPage } from './pages/ConversationListPage';
import { ConversationPage } from './pages/ConversationPage';
import { NewConversationPage } from './pages/NewConversationPage';
import { LoginPage } from './pages/LoginPage';
import { SharePage } from './pages/SharePage';
import { DesktopLayout } from './components/DesktopLayout';
import { ShortcutHelpPanel } from './components/ShortcutHelpPanel';
import { useGlobalKeyboardShortcuts, FocusScopeProvider } from './hooks';
import { ConversationProvider } from './conversation';
import { api } from './api';
import './index.css';

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
    return null;
  }

  if (authState.status === 'login_required') {
    return <LoginPage onSuccess={handleLoginSuccess} />;
  }

  return (
    <BrowserRouter>
      <FocusScopeProvider>
        <ConversationProvider>
          <AppRoutes />
        </ConversationProvider>
      </FocusScopeProvider>
    </BrowserRouter>
  );
}

export default App;
