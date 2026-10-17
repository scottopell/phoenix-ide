import { ApplicationCapabilitiesProvider, hasLiveApplication, useApplicationCapabilities } from './contexts/ApplicationCapabilities';
import { lazy, Suspense, useState, useEffect, useLayoutEffect, useCallback, useRef } from 'react';
import { useConversationViewJourney } from './hooks/useConversationViewJourney';
import { generateUUID } from './utils/uuid';
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
function RouteFallback({ viewRef }: { viewRef?: ((root: HTMLDivElement | null) => void) | undefined }) {
  return <div ref={viewRef} style={{ minHeight: '100vh' }} />;
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
  const journey = useConversationViewJourney('ordinary');
  const journeyRef = useRef(journey);
  useLayoutEffect(() => { journeyRef.current = journey; }, [journey]);
  const navigate = useNavigate();
  const location = useLocation();
  const [fallbackSnapshot, setFallbackSnapshot] = useState<{
    reference: string;
    rowSlug: string;
    aggregateResolutionUnavailable: boolean;
  } | null>(null);
  const [resolvedProduct, setResolvedProduct] = useState<{ reference: string; id: string } | null>(null);
  const [validatedSelection, setValidatedSelection] = useState<string | null>(null);
  const [pinError, setPinError] = useState<string | null>(null);
  const [retryToken, setRetryToken] = useState(0);
  const locationRef = useRef(location);
  locationRef.current = location;
  const sourcePins = new URLSearchParams(location.search).getAll('source_transcript');
  const pinned = sourcePins[0];
  const invalidPin = sourcePins.length > 1 || (pinned !== undefined && (!pinned || pinned.trim() !== pinned));
  const activeFallback = fallbackSnapshot?.reference === reference ? fallbackSnapshot : null;
  const selectionKey = JSON.stringify([reference, sourcePins, location.pathname]);

  useEffect(() => {
    if (!reference) {
      setFallbackSnapshot(null);
      return;
    }
    setValidatedSelection(null);
    setFallbackSnapshot(null);
    setPinError(null);
    if (invalidPin) {
      setPinError('Invalid exact transcript reference.');
      return;
    }
    let cancelled = false;
    const requestJourney = journeyRef.current;
    const peek = (id: string) => {
      const openId = generateUUID();
      const observation = requestJourney.requestStarted(openId);
      return api.getProductConversationSnapshot(id, {
        message_limit: 1,
        ...(observation ? { open_id: openId, onTelemetry: observation.response } : {}),
      });
    };
    api.resolveCoordinatorRoute(reference)
      .then(async ({ coordinator_id }) => {
        if (cancelled) return null;
        if (coordinator_id) {
          if (pinned !== undefined) {
            const [source, exact] = await Promise.all([api.resolveCoordinatorRoute(pinned), api.getConversation(pinned)]);
            if (exact.conversation.id !== pinned || source.coordinator_id !== coordinator_id) throw new Error('Transcript is not a member of this Global conversation');
          }
          if (!cancelled) navigate({ pathname: `/global/${pinned ?? reference}`, search: locationRef.current.search, hash: locationRef.current.hash }, { replace: true });
          return null;
        }
        return peek(reference);
      })
      .then(async (snapshot) => {
        if (!snapshot) return;
        if (!cancelled) {
          if (pinned !== undefined) {
            const source = await peek(pinned);
            if (source.product_conversation_id !== snapshot.product_conversation_id || source.requested_transcript_row_id !== pinned) {
              throw new Error('Transcript is not a member of this conversation');
            }
            if (!cancelled) {
              setResolvedProduct({ reference, id: snapshot.product_conversation_id });
              setValidatedSelection(selectionKey);
            }
            return;
          }
          if (!locationRef.current.pathname.startsWith('/product-conversations/') &&
              snapshot.requested_transcript_row_id === reference && reference !== snapshot.product_conversation_id
              && (locationRef.current.state as { chapterSelection?: unknown } | null)?.chapterSelection !== 'pending') {
            setResolvedProduct({ reference, id: snapshot.product_conversation_id });
            const query = new URLSearchParams(locationRef.current.search);
            query.set('source_transcript', reference);
            navigate({ pathname: locationRef.current.pathname, search: query.toString(), hash: locationRef.current.hash }, { replace: true, state: locationRef.current.state });
            return;
          }
          setResolvedProduct({ reference, id: snapshot.product_conversation_id });
          setValidatedSelection(selectionKey);
          if (locationRef.current.pathname !== snapshot.canonical_route) {
            navigate({
              pathname: snapshot.canonical_route,
              search: locationRef.current.search,
              hash: locationRef.current.hash,
            }, { replace: true, state: locationRef.current.state });
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
  }, [invalidPin, location.pathname, navigate, pinned, reference, retryToken, selectionKey]);

  useEffect(() => {
    if (pinError || activeFallback) journey.finish('failed');
  }, [journey, pinError, activeFallback]);

  if (resolvedProduct && !activeFallback) {
    return <>
      {pinError && <main ref={journey.attach}><div role="alert">{pinError}</div></main>}
      <div ref={journey.attach} hidden={!!pinError || validatedSelection !== selectionKey} style={{ height: '100%' }}>
        <ProductConversationPage productId={resolvedProduct.id} journey={journey} />
      </div>
    </>;
  }
  if (pinError) return <main ref={journey.attach}><div role="alert">{pinError}</div></main>;

  if (activeFallback) {
    return (
      <main ref={journey.attach} className="conversation-alias-fallback">
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
  return <RouteFallback viewRef={journey.attach} />;
}

function ConversationRouteRedirect() {
  const { slug } = useParams<{ slug: string }>();
  return <ProductConversationAliasRedirect reference={slug} />;
}

function ChainRouteRedirect() {
  const { rootConvId } = useParams<{ rootConvId: string }>();
  return <ProductConversationAliasRedirect reference={rootConvId} />;
}

function LiveApp() {
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

const PersistedObservation = lazy(() => import('./pages/PersistedObservation').then(m => ({ default: m.PersistedObservation })));

function CapabilitySelectedApp() {
  const capabilities = useApplicationCapabilities();
  if (hasLiveApplication(capabilities)) return <LiveApp />;
  if (capabilities.persisted_member_read !== 'available') return <div role="alert">Persisted conversation reads unavailable.</div>;
  return <ThemeProvider><DensityProvider><BrowserRouter><FocusScopeProvider>
    <ConversationProvider refreshEnabled={false}><ChainProvider><ConversationReadinessProvider>
      <Suspense fallback={<RouteFallback />}><PersistedObservation /></Suspense>
    </ConversationReadinessProvider></ChainProvider></ConversationProvider>
  </FocusScopeProvider></BrowserRouter></DensityProvider></ThemeProvider>;
}

function App() {
  return <ApplicationCapabilitiesProvider><CapabilitySelectedApp /></ApplicationCapabilitiesProvider>;
}

export default App;
