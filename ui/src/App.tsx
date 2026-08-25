import { lazy, Suspense, useState, useEffect, useCallback } from 'react';
import { BrowserRouter, Routes, Route, useNavigate, useParams, useLocation } from 'react-router-dom';
import { DesktopLayout } from './components/DesktopLayout';
import { ShortcutHelpPanel } from './components/ShortcutHelpPanel';
import { useGlobalKeyboardShortcuts, FocusScopeProvider } from './hooks';
import { ThemeProvider } from './components/ThemeProvider';
import { DensityProvider } from './components/DensityProvider';
import { ConversationProvider } from './conversation';
import { api } from './api';
import { ConversationReadinessProvider } from './contexts/ConversationReadinessContext';
import './index.css';

// Routes are code-split so the initial bundle only contains what the user
// actually needs to view the current page. Heavy dependencies that live in
// specific routes (react-syntax-highlighter, xterm, react-markdown) stay out
// of the main chunk until that route mounts.
const ConversationListPage = lazy(() =>
  import('./pages/ConversationListPage').then((m) => ({ default: m.ConversationListPage })),
);
const ProductConversationPage = lazy(() =>
  import('./pages/ProductConversationPage').then((m) => ({ default: m.ProductConversationPage })),
);
const NewConversationPage = lazy(() =>
  import('./pages/NewConversationPage').then((m) => ({ default: m.NewConversationPage })),
);
const LoginPage = lazy(() =>
  import('./pages/LoginPage').then((m) => ({ default: m.LoginPage })),
);
const CodexLoginPage = lazy(() =>
  import('./pages/CodexLoginPage').then((m) => ({ default: m.CodexLoginPage })),
);
const AboutDeploymentPage = lazy(() =>
  import('./pages/AboutDeploymentPage').then((m) => ({ default: m.AboutDeploymentPage })),
);
const SharePage = lazy(() =>
  import('./pages/SharePage').then((m) => ({ default: m.SharePage })),
);
const UsagePage = lazy(() =>
  import('./pages/UsagePage').then((m) => ({ default: m.UsagePage })),
);
const CoordinatorPage = lazy(() =>
  import('./pages/CoordinatorPage').then((m) => ({ default: m.CoordinatorPage })),
);
const TerminalPage = lazy(() =>
  import('./pages/TerminalPage').then((m) => ({ default: m.TerminalPage })),
);
const LlmLanguagePage = lazy(() =>
  import('./pages/LlmLanguagePage').then((m) => ({ default: m.LlmLanguagePage })),
);
const GroundingPanelFixturePage = import.meta.env.DEV
  ? lazy(() => import('./pages/GroundingPanelFixturePage').then((m) => ({ default: m.GroundingPanelFixturePage })))
  : null;
const MobileConversationListFixturePage = import.meta.env.DEV
  ? lazy(() => import('./pages/MobileConversationListFixturePage').then((m) => ({ default: m.MobileConversationListFixturePage })))
  : null;

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
          {GroundingPanelFixturePage && (
            <Route path="/__qa/grounding-panel" element={<GroundingPanelFixturePage />} />
          )}
          {MobileConversationListFixturePage && (
            <Route path="/__qa/mobile-conversation-list" element={<MobileConversationListFixturePage />} />
          )}
          {/* Share view: minimal layout, no sidebar, no auth required */}
          <Route path="/s/:token" element={<SharePage />} />
          {/* Main app routes: full layout with sidebar */}
          <Route path="*" element={
            <DesktopLayout>
              <Routes>
                <Route path="/" element={<ConversationListPage />} />
                <Route path="/new" element={<NewConversationPage />} />
                <Route path="/terminal" element={<TerminalPage />} />
                <Route path="/c/:slug" element={<ConversationRouteRedirect />} />
                <Route path="/product-conversations/:productConversationId" element={<ProductConversationPage />} />
                <Route path="/chains/:rootConvId" element={<ChainRouteRedirect />} />
                <Route path="/codex/login" element={<CodexLoginPage />} />
                <Route path="/about" element={<AboutDeploymentPage />} />
                <Route path="/usage" element={<UsagePage />} />
                <Route path="/global" element={<CoordinatorPage />} />
                <Route path="/global/:slug" element={<CoordinatorPage />} />
                <Route path="/settings/llm-language" element={<LlmLanguagePage />} />
              </Routes>
            </DesktopLayout>
          } />
        </Routes>
      </Suspense>
      <ShortcutHelpPanel visible={showHelp} onClose={() => setShowHelp(false)} />
    </>
  );
}

function ProductConversationAliasRedirect({ reference }: { reference: string | undefined }) {
  const navigate = useNavigate();
  const location = useLocation();
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (!reference) {
      setFailed(true);
      return;
    }
    let cancelled = false;
    api.getProductConversationSnapshot(reference, { message_limit: 1 })
      .then((snapshot) => {
        if (!cancelled) {
          navigate({
            pathname: snapshot.canonical_route,
            search: location.search,
            hash: location.hash,
          }, { replace: true });
        }
      })
      .catch(() => {
        if (!cancelled) setFailed(true);
      });
    return () => { cancelled = true; };
  }, [location.hash, location.search, navigate, reference]);

  return failed ? <main role="alert">Failed to resolve product conversation route</main> : <RouteFallback />;
}

function ConversationRouteRedirect() {
  const { slug } = useParams<{ slug: string }>();
  return <ProductConversationAliasRedirect reference={slug} />;
}

function ChainRouteRedirect() {
  const { rootConvId } = useParams<{ rootConvId: string }>();
  return <ProductConversationAliasRedirect reference={rootConvId} />;
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
      <DensityProvider>
        <BrowserRouter>
          <FocusScopeProvider>
            <ConversationProvider>
              <ConversationReadinessProvider>
                <AppRoutes />
              </ConversationReadinessProvider>
            </ConversationProvider>
          </FocusScopeProvider>
        </BrowserRouter>
      </DensityProvider>
    </ThemeProvider>
  );
}

export default App;
