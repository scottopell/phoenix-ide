import { ProductConversationAliasRedirect } from '../../App';
import { CoordinatorPage } from '../../pages/CoordinatorPage';
import { FocusScopeProvider } from '../../hooks/useFocusScope';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { HashRouter, MemoryRouter, Navigate, Route, Routes, useParams, useLocation } from 'react-router-dom';
import { ChainProvider } from '../../chain';
import { useDocumentViewportOwnership } from '../../components/viewportRoutes';
import { FileExplorerProvider } from '../../components/FileExplorer';
import { ConversationReadinessProvider } from '../../contexts/ConversationReadinessContext';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import { ConversationProvider } from '../../conversation';
import { useDraftActions } from '../../hooks/useDraft';
import { ProductConversationPage } from '../../pages/ProductConversationPage';
import '../../index.css';
import { installProductConversationFixtureApi } from './mockApi';
import type { ProductConversationScenario } from './types';

function FixtureRouter({ initialEntry, children }: { initialEntry: string; children: ReactNode }) {
  return new URLSearchParams(window.location.search).get('fixtureBrowserHistory') === '1'
    ? <HashRouter>{children}</HashRouter>
    : <MemoryRouter initialEntries={[initialEntry]}>{children}</MemoryRouter>;
}

function FixtureLocation() {
  const location = useLocation();
  useEffect(() => { document.documentElement.dataset['sourceFixtureLocation'] = `${location.pathname}${location.search}${location.hash}`; }, [location]);
  return null;
}
function FixtureAlias() { const { slug } = useParams(); return <ProductConversationAliasRedirect reference={slug} />; }

interface Props {
  scenario: ProductConversationScenario;
  observer?: ReactNode;
}

function SeedReactionDraft({ slug, text }: { slug: string; text: string }) {
  const { setDraftIfEmpty } = useDraftActions(slug);
  useEffect(() => setDraftIfEmpty(text), [setDraftIfEmpty, text]);
  return null;
}

function pageHasSettled(root: HTMLElement, scenario: ProductConversationScenario): boolean {
  if (scenario.state === 'loading') {
    return root.querySelector('.skeleton-message') !== null
      || root.querySelector('[data-testid="product-conversation-page"] #messages') !== null;
  }
  if (scenario.state === 'error') {
    return root.textContent?.includes(scenario.snapshotError ?? 'Fixture failed to fetch product conversation snapshot') ?? false;
  }
  if (scenario.initialEntry?.startsWith('/global/')) {
    return root.querySelector('.coordinator-conversation #chat-view .conversation-event-card') !== null;
  }
  const page = root.querySelector('[data-testid="product-conversation-page"]');
  if (!page) return false;
  const hasConversation = page.querySelector('#chat-view') !== null;
  if (scenario.kind !== 'coordinator' && scenario.snapshot?.ordinary_lifecycle === 'history') {
    return hasConversation
      && page.querySelector('[data-testid="product-conversation-history"]') !== null;
  }
  return hasConversation
    && page.querySelector('.product-conversation-page__composer .embedded-conversation-shell') !== null;
}

function ProductConversationFixtureBody({ scenario, observer }: Props) {
  useDocumentViewportOwnership(true);
  const rootRef = useRef<HTMLElement>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    const previousTheme = document.documentElement.getAttribute('data-theme');
    const theme = new URLSearchParams(window.location.search).get('fixtureTheme') === 'light' ? 'light' : 'dark';
    document.documentElement.dataset['theme'] = theme;
    document.documentElement.dataset['productConversationFixtureTheme'] = theme;
    return () => {
      delete document.documentElement.dataset['productConversationFixtureTheme'];
      if (previousTheme === null) document.documentElement.removeAttribute('data-theme');
      else document.documentElement.dataset['theme'] = previousTheme;
    };
  }, []);

  useEffect(() => {
    delete document.documentElement.dataset['productConversationFixtureReady'];
    const root = rootRef.current;
    if (!root) return undefined;

    let frame = 0;
    const checkUntilSettled = () => {
      if (pageHasSettled(root, scenario)) {
        setReady(true);
        document.documentElement.dataset['productConversationFixtureReady'] = scenario.id;
        return;
      }
      frame = requestAnimationFrame(checkUntilSettled);
    };
    setReady(false);
    checkUntilSettled();
    return () => {
      cancelAnimationFrame(frame);
      delete document.documentElement.dataset['productConversationFixtureReady'];
    };
  }, [scenario]);

  const fixtureHash = new URLSearchParams(window.location.search).get('fixtureHash') ?? '';

  return (
    <main
      ref={rootRef}
      data-product-conversation-fixture={scenario.id}
      {...(ready ? { 'data-product-conversation-fixture-ready': scenario.id } : {})}
    >
      <FixtureRouter initialEntry={new URLSearchParams(window.location.search).get('fixtureEntry') ?? (scenario.initialEntry ?? (scenario.kind === 'coordinator' ? `/global/${scenario.snapshot.latest_transcript_row_id}${fixtureHash}` : `/product-conversations/fixture-product-conversation${fixtureHash}`))}>
        <FixtureLocation />
        <FocusScopeProvider>
          <ConversationProvider>
            {observer}
            {scenario.initialDraft && scenario.snapshot?.latest_transcript_row_id && (
              <SeedReactionDraft slug={scenario.snapshot.latest_transcript_row_id} text={scenario.initialDraft} />
            )}
            <ConversationReadinessProvider>
              <ChainProvider>
                <ViewerSlotProvider scopeKey="fixture-product-conversation" browserSessionActive={false}>
                  <FileExplorerProvider>
                    <Routes>
                      <Route path="/" element={<Navigate replace to={new URLSearchParams(window.location.search).get('fixtureEntry') ?? (scenario.kind === 'coordinator' ? `/global/${scenario.snapshot.latest_transcript_row_id}${fixtureHash}` : `/product-conversations/fixture-product-conversation${fixtureHash}`)} />} />
                      <Route path="/c/:slug" element={<FixtureAlias />} />
                      <Route path="/global" element={<CoordinatorPage />} />
                      <Route path="/global/:slug" element={<CoordinatorPage />} />
                      <Route path="/product-conversations/:productConversationId" element={<ProductConversationPage />} />
                    </Routes>
                  </FileExplorerProvider>
                </ViewerSlotProvider>
              </ChainProvider>
            </ConversationReadinessProvider>
          </ConversationProvider>
        </FocusScopeProvider>
      </FixtureRouter>
    </main>
  );
}

export function ProductConversationFixture({ scenario, observer }: Props) {
  const [installed, setInstalled] = useState(false);

  useEffect(() => {
    const restoreApi = installProductConversationFixtureApi(scenario);
    const originalUrl = window.location.href;
    const params = new URLSearchParams(window.location.search);
    setInstalled(true);
    return () => {
      restoreApi();
      if (params.get('fixtureBrowserHistory') === '1') window.history.replaceState(null, '', originalUrl);
      setInstalled(false);
    };
  }, [scenario]);

  if (!installed) return null;
  return <ProductConversationFixtureBody scenario={scenario} observer={observer} />;
}
