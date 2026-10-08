import { lazy, Suspense, useState, useEffect, useCallback } from 'react';
import { BrowserRouter, Routes, Route, useNavigate, useParams, useLocation } from 'react-router-dom';
import { DesktopLayout } from './components/DesktopLayout';
import { ShortcutHelpPanel } from './components/ShortcutHelpPanel';
import { useGlobalKeyboardShortcuts, FocusScopeProvider } from './hooks';
import { ThemeProvider } from './components/ThemeProvider';
import { DensityProvider } from './components/DensityProvider';
import { ConversationProvider } from './conversation';
import { ChainProvider } from './chain';
import { api, ApiResponseError } from './api';
import { ConversationReadinessProvider } from './contexts/ConversationReadinessContext';
import './index.css';
import './ConversationAliasFallback.css';

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
const EmbeddedConversationPage = lazy(() =>
  import('./pages/ConversationPage').then((m) => ({ default: m.EmbeddedConversationPage })),
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
                <Route path="/product-conversations/:slug" element={<ConversationRouteRedirect />} />
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

export function ProductConversationAliasRedirect({ reference }: { reference: string | undefined }) {
  const navigate = useNavigate();
  const location = useLocation();
  const [fallbackSnapshot, setFallbackSnapshot] = useState<{
    reference: string;
    rowSlug: string;
    aggregateResolutionUnavailable: boolean;
  } | null>(null);
  const [resolvedProduct, setResolvedProduct] = useState<{ reference: string; id: string } | null>(null);
  const [exactMember, setExactMember] = useState<{ reference: string; transcript: string; open: boolean } | null>(null);
  const [pinError, setPinError] = useState<string | null>(null);
  const [retryToken, setRetryToken] = useState(0);
  const activeFallback = fallbackSnapshot?.reference === reference ? fallbackSnapshot : null;

  useEffect(() => {
    if (!reference) {
      setFallbackSnapshot(null);
      return;
    }
    setExactMember(null);
    setResolvedProduct(null);
    setFallbackSnapshot(null);
    setPinError(null);
    const pins = new URLSearchParams(location.search).getAll('source_transcript');
    const pinned = pins[0];
    if (pins.length > 1 || (pinned !== undefined && (!pinned || pinned.trim() !== pinned))) {
      setPinError('Invalid exact transcript reference.');
      return;
    }
    let cancelled = false;
    api.resolveCoordinatorRoute(reference)
      .then(async ({ coordinator_id }) => {
        if (cancelled) return null;
        if (coordinator_id) {
          if (pinned !== undefined) {
            const [source, exact] = await Promise.all([api.resolveCoordinatorRoute(pinned), api.getConversation(pinned)]);
            if (exact.conversation.id !== pinned || source.coordinator_id !== coordinator_id) throw new Error('Transcript is not a member of this Global conversation');
          }
          if (!cancelled) navigate({ pathname: `/global/${pinned ?? reference}`, search: location.search, hash: location.hash }, { replace: true });
          return null;
        }
        return api.getProductConversationSnapshot(reference, { message_limit: 1 });
      })
      .then(async (snapshot) => {
        if (!snapshot) return;
        if (!cancelled) {
          if (pinned !== undefined) {
            const source = await api.getProductConversationSnapshot(pinned, { message_limit: 1 });
            if (source.product_conversation_id !== snapshot.product_conversation_id || source.requested_transcript_row_id !== pinned) {
              throw new Error('Transcript is not a member of this conversation');
            }
            if (!cancelled) setExactMember({ reference, transcript: pinned, open: snapshot.ordinary_lifecycle === 'open' && pinned === snapshot.latest_transcript_row_id });
            return;
          }
          if (snapshot.requested_transcript_row_id === reference && reference !== snapshot.product_conversation_id) {
            setExactMember({ reference, transcript: reference, open: snapshot.ordinary_lifecycle === 'open' && reference === snapshot.latest_transcript_row_id });
            return;
          }
          setResolvedProduct({ reference, id: snapshot.product_conversation_id });
          if (location.pathname !== snapshot.canonical_route) {
            navigate({
              pathname: snapshot.canonical_route,
              search: location.search,
              hash: location.hash,
            }, { replace: true });
          }
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          if (pinned !== undefined) {
            setPinError('Exact transcript unavailable or not a member of this conversation.');
            return;
          }
          setFallbackSnapshot({
            reference,
            rowSlug: reference,
            aggregateResolutionUnavailable: !(error instanceof ApiResponseError && error.status === 404),
          });
        }
      });
    return () => { cancelled = true; };
  }, [location.hash, location.pathname, location.search, navigate, reference, retryToken]);

  if (pinError) return <main><div role="alert">{pinError}</div></main>;

  if (exactMember && exactMember.reference === reference) {
    return <EmbeddedConversationPage slug={exactMember.transcript} suppressCanonicalization routePrefix="/c"
      aggregateLifecycleOpen={exactMember.open} mutationEnabled={exactMember.open} />;
  }

  if (resolvedProduct && resolvedProduct.reference === reference && !activeFallback) {
    return <ProductConversationPage productId={resolvedProduct.id} />;
  }

  if (activeFallback) {
    return (
      <main className="conversation-alias-fallback">
        <div role="alert" className="conversation-alias-fallback__status">
          Conversation route unavailable.
          <button type="button" onClick={() => { setFallbackSnapshot(null); setRetryToken((n) => n + 1); }}>Retry</button>
        </div>
        <EmbeddedConversationPage
          slug={activeFallback.rowSlug}
          suppressCanonicalization
          routePrefix="/c"
          mutationEnabled={!activeFallback.aggregateResolutionUnavailable}
          {...(activeFallback.aggregateResolutionUnavailable
            ? { aggregateLifecycleOpen: false }
            : {})}
        />
      </main>
    );
  }
  return <RouteFallback />;
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
              <ChainProvider>
                <ConversationReadinessProvider>
                  <AppRoutes />
                </ConversationReadinessProvider>
              </ChainProvider>
            </ConversationProvider>
          </FocusScopeProvider>
        </BrowserRouter>
      </DensityProvider>
    </ThemeProvider>
  );
}

export default App;
