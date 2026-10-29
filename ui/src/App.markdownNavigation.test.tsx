import type { ReactNode } from 'react';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useLocation } from 'react-router-dom';
import { MarkdownRenderer } from './components/MarkdownRenderer';
import { afterEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { ViewJourneyLifecycle } from './hooks/conversationViewLifecycle';
import { api } from './api';
import { FileExplorerProvider } from './components/FileExplorer';
import { CONVERSATION_MARKDOWN_URL_TRANSFORM, createConversationMarkdownComponents } from './components/conversationMarkdownImages';
import { ViewerSlotProvider } from './contexts/ViewerSlotContext';
import { installProductConversationFixtureApi } from './fixtures/productConversation/mockApi';
import { coordinatorSnapshot, getProductConversationScenario } from './fixtures/productConversation/scenarios';

vi.mock('./hooks/conversationViewTelemetry', async importOriginal => {
  const actual = await importOriginal<typeof import('./hooks/conversationViewTelemetry')>();
  return { ...actual, ViewJourneyReporter: class extends actual.ViewJourneyReporter {
    constructor() { super({ sampleRate: 1, send: async () => new Response(null, { status: 204 }) }); }
  } };
});

const origin = '/global/coordinator?reference=origin%3Arow#message-origin';
const suffix = '?reference=destination%3Arow#message-history-4';
const route = vi.hoisted(() => ({ destination: '', realGlobal: false }));

vi.mock('./pages/CoordinatorPage', async importOriginal => {
  const actual = await importOriginal<typeof import('./pages/CoordinatorPage')>();
  return {
    CoordinatorPage: () => {
      const location = useLocation();
      return route.realGlobal && location.pathname !== '/global/coordinator'
        ? <actual.CoordinatorPage />
        : <MarkdownRenderer mode="conversation" components={createConversationMarkdownComponents({})} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>
          {`[Conversation citation](<${route.destination}>)`}
        </MarkdownRenderer>;
    },
  };
});

vi.mock('./cache', () => ({ cacheDB: {
  init: vi.fn().mockResolvedValue(undefined),
  getPendingOps: vi.fn().mockResolvedValue([]),
  getConversation: vi.fn().mockResolvedValue(null),
  putConversation: vi.fn().mockResolvedValue(undefined),
} }));

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
  route.realGlobal = false;
  window.history.replaceState(null, '', '/');
});

describe('App Markdown alias navigation', () => {
  it.each((['ordinary', 'global'] as const).flatMap(domain =>
    ['café', 'caf%C3%A9', '%', '%25', 'old%2Frow', 'old%2frow', 'x/y?z=1', '[x]', '.', '..'].flatMap(id => [false, true].map(withMessage => ({ domain, id, withMessage }))),
  ))('carries opaque citation $id into the real $domain reader (message=$withMessage)', async ({ domain, id, withMessage }) => {
    // API fixtures deliberately assert routing-to-reader behavior, not HTTP transport.
    const message = id;
    const scenario = getProductConversationScenario('desktop-multi-segment-qa-work');
    const base = scenario.snapshot!;
    const originalRow = base.segments[0]!.transcript_row_id;
    const snapshot = {
      ...base, requested_transcript_row_id: id,
      canonical_root: { ...base.canonical_root, transcript_row_id: id },
      segments: base.segments.map((segment, index) => index === 0 ? {
        ...segment, transcript_row_id: id,
        messages: segment.messages.map((item, ordinal) => ({ ...item, conversation_id: id, message_id: ordinal === 0 ? message : item.message_id })),
      } : segment),
      boundaries: base.boundaries.map(boundary => ({ ...boundary, predecessor_transcript_row_id: boundary.predecessor_transcript_row_id === originalRow ? id : boundary.predecessor_transcript_row_id })),
    };
    restoreFixture = installProductConversationFixtureApi(domain === 'ordinary'
      ? { ...scenario, snapshot }
      : { ...getProductConversationScenario('global-lazy-history'), snapshot: coordinatorSnapshot(snapshot), olderPages: {} });
    vi.spyOn(api, 'authStatus').mockResolvedValue({ auth_required: false, authenticated: true });
    const resolve = vi.spyOn(api, 'resolveCoordinatorRoute');
    const exact = vi.spyOn(api, 'getConversation');
    const lookup = vi.spyOn(api, 'getProductConversationSnapshot');
    const hydration = vi.spyOn(ViewJourneyLifecycle.prototype, 'hydrationReady');
    const target = vi.spyOn(ViewJourneyLifecycle.prototype, 'expectTarget');
    route.realGlobal = domain === 'global';
    route.destination = `@transcript:${id}${withMessage ? `#message-${message}` : ''}`;
    window.history.replaceState(null, '', origin);
    const view = render(<App />);
    const citation = await screen.findByRole('link', { name: 'Conversation citation' });
    const href = new URL(citation.getAttribute('href')!, window.location.origin);
    expect(href.pathname).toBe('/transcript');
    expect(href.searchParams.getAll('source_transcript')).toEqual([id]);
    fireEvent.click(citation);
    const destination = `${domain === 'ordinary' ? snapshot.canonical_route : '/global'}${href.search}${href.hash}`;
    await waitFor(() => expect(screen.getByTestId('route').textContent).toBe(destination));
    expect(await screen.findByTestId('product-conversation-page', {}, { timeout: 5_000 })).toBeInTheDocument();
    await waitFor(() => expect(hydration).toHaveBeenCalledWith(snapshot.product_conversation_id, id, expect.any(Number)));
    expect(target).toHaveBeenCalledWith(true, message);
    expect(screen.queryByText('Original source transcript unavailable.')).not.toBeInTheDocument();
    expect(screen.queryByText('The linked message is not available in this conversation history.')).not.toBeInTheDocument();
    expect(resolve).toHaveBeenCalledWith(id);
    if (domain === 'ordinary') expect(lookup).toHaveBeenCalledWith(id, expect.objectContaining({ message_limit: 1 }));
    else expect(exact).toHaveBeenCalledWith(id);
    expect(JSON.parse(screen.getByTestId('route-state').textContent!)).toEqual({
      conversationReturnOrigin: { kind: 'coordinator', href: origin },
    });
    if (domain === 'ordinary') expect(await screen.findByRole('link', { name: '← from: Coordinator' })).toHaveAttribute('href', origin);
    view.unmount();
    vi.restoreAllMocks();
    restoreFixture();
    restoreFixture = undefined;
  });

  it.each(['old%2Frow', 'old%2frow'])('hydrates an unpinned Global literal encoded path through the real route owner: %s', async id => {
    const scenario = getProductConversationScenario('global-lazy-history');
    const base = getProductConversationScenario('desktop-multi-segment-qa-work').snapshot!;
    const snapshot = coordinatorSnapshot({
      ...base,
      requested_transcript_row_id: id,
      canonical_root: { ...base.canonical_root, transcript_row_id: id },
      boundaries: base.boundaries.map(boundary => ({ ...boundary, predecessor_transcript_row_id: boundary.predecessor_transcript_row_id === base.segments[0]!.transcript_row_id ? id : boundary.predecessor_transcript_row_id })),
      segments: base.segments.map((segment, index) => index === 0 ? {
        ...segment, transcript_row_id: id, slug: id,
        messages: segment.messages.map(message => ({ ...message, conversation_id: id })),
      } : segment),
    });
    restoreFixture = installProductConversationFixtureApi({ ...scenario, snapshot, olderPages: {} });
    vi.spyOn(api, 'authStatus').mockResolvedValue({ auth_required: false, authenticated: true });
    const hydration = vi.spyOn(ViewJourneyLifecycle.prototype, 'hydrationReady');
    const resolve = vi.spyOn(api, 'resolveCoordinatorRoute');
    route.realGlobal = true;
    const entry = `/global/${encodeURIComponent(id)}`;
    window.history.replaceState(null, '', entry);
    render(<App />);
    expect(await screen.findByTestId('product-conversation-page')).toBeVisible();
    await waitFor(() => expect(hydration).toHaveBeenCalledWith(snapshot.product_conversation_id, id, expect.any(Number)));
    expect(resolve).toHaveBeenCalledWith(id);
    expect(screen.getByTestId('route').textContent).toBe(entry);
    expect(screen.queryByText('Original source transcript unavailable.')).not.toBeInTheDocument();
  });

  it.each(['/product-conversations/legacy-alias', '/c/product-alpha'])(
    'retains the Coordinator breadcrumb and complete state through canonicalization of %s', async (alias) => {
      const scenario = getProductConversationScenario('mobile-open');
      const snapshot = { ...scenario.snapshot!, canonical_route: '/c/product-alpha', product_conversation_id: 'product-alpha' };
      restoreFixture = installProductConversationFixtureApi({ ...scenario, snapshot });
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
