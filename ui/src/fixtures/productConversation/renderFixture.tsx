import { useEffect, useRef, useState } from 'react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
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
  const page = root.querySelector('[data-testid="product-conversation-page"]');
  if (!page) return false;
  const title = page.querySelector('h1')?.textContent;
  const route = page.querySelector('.product-conversation-page__route')?.textContent;
  const historyLabel = page.querySelector('.product-conversation-meta');
  return title === scenario.snapshot?.presentation.display_name
    && route === scenario.snapshot?.canonical_route
    && historyLabel !== null;
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

  return (
    <main
      ref={rootRef}
      data-product-conversation-fixture={scenario.id}
      {...(ready ? { 'data-product-conversation-fixture-ready': scenario.id } : {})}
    >
      <MemoryRouter initialEntries={['/product-conversations/fixture-product-conversation']}>
        <ConversationProvider>
          <ConversationReadinessProvider>
            <ViewerSlotProvider scopeKey="fixture-product-conversation" browserSessionActive={false}>
              <FileExplorerProvider>
                <Routes>
                  <Route path="/product-conversations/:productConversationId" element={<ProductConversationPage />} />
                </Routes>
              </FileExplorerProvider>
            </ViewerSlotProvider>
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
