import { liveCapabilities } from './contexts/ApplicationCapabilities';
import type { ReactNode } from 'react';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useLocation } from 'react-router-dom';
import ReactMarkdown from 'react-markdown';
import { afterEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { api } from './api';
import { FileExplorerProvider } from './components/FileExplorer';
import { createConversationMarkdownComponents } from './components/conversationMarkdownImages';
import { ViewerSlotProvider } from './contexts/ViewerSlotContext';
import { installProductConversationFixtureApi } from './fixtures/productConversation/mockApi';
import { getProductConversationScenario } from './fixtures/productConversation/scenarios';

vi.mock('./hooks/conversationViewTelemetry', async importOriginal => {
  const actual = await importOriginal<typeof import('./hooks/conversationViewTelemetry')>();
  return { ...actual, ViewJourneyReporter: class extends actual.ViewJourneyReporter {
    constructor() { super({ sampleRate: 1, send: async () => new Response(null, { status: 204 }) }); }
  } };
});

const origin = '/global/coordinator?reference=origin%3Arow#message-origin';
const suffix = '?reference=destination%3Arow#message-history-4';
const route = vi.hoisted(() => ({ destination: '' }));

vi.mock('./pages/CoordinatorPage', () => ({
  CoordinatorPage: () => (
    <ReactMarkdown components={createConversationMarkdownComponents({})}>
      {`[Conversation citation](${route.destination})`}
    </ReactMarkdown>
  ),
}));

vi.mock('./cache', () => ({ cacheDB: { getConversation: vi.fn().mockResolvedValue(null), putConversation: vi.fn().mockResolvedValue(undefined) } }));

vi.mock('./components/DesktopLayout', () => ({
  DesktopLayout: ({ children }: { children: ReactNode }) => {
    const location = useLocation();
    return (
      <ViewerSlotProvider browserSessionActive={false}>
        <FileExplorerProvider>
          <output data-testid="route">{location.pathname}{location.search}{location.hash}</output>
          <output data-testid="route-state">{JSON.stringify(location.state)}</output>
          {children}
        </FileExplorerProvider>
      </ViewerSlotProvider>
    );
  },
}));

let restoreFixture: (() => void) | undefined;
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  restoreFixture?.();
  restoreFixture = undefined;
  window.history.replaceState(null, '', '/');
});

describe('App Markdown alias navigation', () => {
  it.each(['/product-conversations/legacy-alias', '/c/product-alpha'])(
    'retains the Coordinator breadcrumb and complete state through canonicalization of %s', async (alias) => {
      const scenario = getProductConversationScenario('mobile-open');
      const snapshot = { ...scenario.snapshot!, canonical_route: '/c/product-alpha', product_conversation_id: 'product-alpha' };
      restoreFixture = installProductConversationFixtureApi({ ...scenario, snapshot });
      const originalFetch = globalThis.fetch;
      vi.spyOn(globalThis, 'fetch').mockImplementation((input, init) => String(input) === '/api/capabilities'
        ? Promise.resolve(new Response(JSON.stringify(liveCapabilities)))
        : originalFetch(input, init));
      vi.spyOn(api, 'authStatus').mockResolvedValue({ auth_required: false, authenticated: true });
      const lookup = vi.spyOn(api, 'getProductConversationSnapshot');
      route.destination = `${alias}${suffix}`;
      window.history.replaceState(null, '', origin);
      render(<App />);

      fireEvent.click(await screen.findByRole('link', { name: 'Conversation citation' }));
      await waitFor(() => expect(screen.getByTestId('route').textContent).toBe(`/c/product-alpha${suffix}`));
      expect(lookup).toHaveBeenCalledWith(alias.split('/').at(-1), { message_limit: 1, open_id: expect.any(String), onTelemetry: expect.any(Function) });
      expect(await screen.findByTestId('product-conversation-page', {}, { timeout: 5_000 })).toBeInTheDocument();
      const breadcrumb = await screen.findByRole('link', { name: '← from: Coordinator' });
      expect(breadcrumb).toHaveAttribute('href', origin);
      expect(screen.getByTestId('route-state').textContent).toBe(JSON.stringify({
        conversationReturnOrigin: { kind: 'coordinator', href: origin },
      }));

      fireEvent.click(breadcrumb);
      await screen.findByRole('link', { name: 'Conversation citation' });
      expect(screen.getByTestId('route').textContent).toBe(origin);
    },
  );
});
