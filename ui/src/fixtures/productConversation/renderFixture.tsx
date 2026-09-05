import { useEffect, useRef, useState } from 'react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { ChainProvider } from '../../chain';
import { useDocumentViewportOwnership } from '../../components/viewportRoutes';
import { FileExplorerProvider } from '../../components/FileExplorer';
import { ConversationReadinessProvider } from '../../contexts/ConversationReadinessContext';
import { ViewerSlotProvider } from '../../contexts/ViewerSlotContext';
import { ConversationProvider } from '../../conversation';
import { ProductConversationPage } from '../../pages/ProductConversationPage';
import '../../index.css';
import { installProductConversationFixtureApi } from './mockApi';
import type { ProductConversationScenario } from './types';

interface Props {
  scenario: ProductConversationScenario;
}

function pageHasSettled(root: HTMLElement, scenario: ProductConversationScenario): boolean {
  if (scenario.state === 'loading') {
    return root.querySelector('.skeleton-message') !== null
      || root.querySelector('[data-testid="product-conversation-page"] #messages') !== null;
  }
  if (scenario.state === 'error') {
    return root.textContent?.includes(scenario.snapshotError ?? 'Fixture failed to fetch product conversation snapshot') ?? false;
  }
  const page = root.querySelector('[data-testid="product-conversation-page"]');
  if (!page) return false;
  const hasConversation = page.querySelector('#chat-view') !== null;
  if (scenario.snapshot?.ordinary_lifecycle === 'history') {
    return hasConversation
      && page.querySelector('[data-testid="product-conversation-history"]') !== null;
  }
  return hasConversation
    && page.querySelector('.product-conversation-page__composer .embedded-conversation-shell') !== null;
}

function ProductConversationFixtureBody({ scenario }: Props) {
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
      <MemoryRouter initialEntries={[`/product-conversations/fixture-product-conversation${fixtureHash}`]}>
        <ConversationProvider>
          <ConversationReadinessProvider>
            <ChainProvider>
              <ViewerSlotProvider scopeKey="fixture-product-conversation" browserSessionActive={false}>
                <FileExplorerProvider>
                  <Routes>
                    <Route path="/product-conversations/:productConversationId" element={<ProductConversationPage />} />
                  </Routes>
                </FileExplorerProvider>
              </ViewerSlotProvider>
            </ChainProvider>
          </ConversationReadinessProvider>
        </ConversationProvider>
      </MemoryRouter>
    </main>
  );
}

export function ProductConversationFixture({ scenario }: Props) {
  const [installed, setInstalled] = useState(false);

  useEffect(() => {
    const restoreApi = installProductConversationFixtureApi(scenario);
    setInstalled(true);
    return () => {
      restoreApi();
      setInstalled(false);
    };
  }, [scenario]);

  if (!installed) return null;
  return <ProductConversationFixtureBody scenario={scenario} />;
}
