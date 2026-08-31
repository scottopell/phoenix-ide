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
    return root.querySelector('.message-list-skeleton') !== null || root.textContent?.includes('Product conversation') === true;
  }
  if (scenario.state === 'error') {
    return root.textContent?.includes(scenario.snapshotError ?? 'Fixture failed to fetch product conversation snapshot') ?? false;
  }
  const page = root.querySelector<HTMLElement>('[data-testid="product-conversation-page"]');
  if (!page) return false;
  const title = page.querySelector('h1')?.textContent;
  const route = page.querySelector('.product-conversation-page__route')?.textContent;
  const metadata = page.querySelector('[aria-label="Product conversation metadata"]');
  return title === scenario.snapshot?.presentation.display_name
    && route === scenario.snapshot?.canonical_route
    && metadata !== null;
}

function ProductConversationFixtureBody({ scenario }: Props) {
  useDocumentViewportOwnership(true);
  const rootRef = useRef<HTMLElement>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    const previousTheme = document.documentElement.getAttribute('data-theme');
    document.documentElement.dataset['theme'] = 'dark';
    return () => {
      if (previousTheme === null) document.documentElement.removeAttribute('data-theme');
      else document.documentElement.dataset['theme'] = previousTheme;
    };
  }, []);

  useEffect(() => {
    delete document.documentElement.dataset['productConversationFixtureReady'];
    const root = rootRef.current;
    if (!root) return undefined;

    let interval = 0;
    const checkUntilSettled = () => {
      if (!pageHasSettled(root, scenario)) return;
      setReady(true);
      document.documentElement.dataset['productConversationFixtureReady'] = scenario.id;
      window.clearInterval(interval);
    };
    setReady(false);
    checkUntilSettled();
    interval = window.setInterval(checkUntilSettled, 16);
    return () => {
      window.clearInterval(interval);
      delete document.documentElement.dataset['productConversationFixtureReady'];
    };
  }, [scenario]);

  return (
    <main
      ref={rootRef}
      data-product-conversation-fixture={scenario.id}
      data-product-conversation-scenario={scenario.id}
      data-product-conversation-viewport={scenario.viewport}
      data-product-conversation-state={scenario.state}
      data-product-conversation-surface="product-conversation"
      {...(ready ? { 'data-product-conversation-fixture-ready': scenario.id } : {})}
    >
      <MemoryRouter initialEntries={['/product-conversations/fixture-product-conversation']}>
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
