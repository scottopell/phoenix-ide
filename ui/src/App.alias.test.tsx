import { Suspense } from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ProductConversationAliasRedirect } from './App';
import { api, ApiResponseError } from './api';

const embeddedSpy = vi.fn();

vi.mock('./api', async () => {
  const actual = await vi.importActual<typeof import('./api')>('./api');
  return {
    ...actual,
    api: { ...actual.api, getProductConversationSnapshot: vi.fn(), resolveCoordinatorRoute: vi.fn() },
  };
});

vi.mock('./pages/ConversationPage', () => ({
  EmbeddedConversationPage: (props: unknown) => {
    embeddedSpy(props);
    return <div data-testid="embedded-fallback" />;
  },
}));

vi.mock('./pages/ProductConversationPage', () => ({
  ProductConversationPage: ({ productId }: { productId: string }) => <div data-testid="product-page">{productId}</div>,
}));

function Location() {
  const location = useLocation();
  return <div data-testid="location">{location.pathname}{location.search}{location.hash}</div>;
}

function renderAlias(reference: string, entry = `/c/${reference}`) {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <Location />
      <Suspense fallback={null}>
        <Routes>
          <Route path="/c/:slug" element={<ProductConversationAliasRedirect reference={reference} />} />
          <Route path="/product-conversations/:id" element={entry.startsWith('/product-conversations/') ? <ProductConversationAliasRedirect reference={reference} /> : null} />
          <Route path="/global/:slug" element={null} />
        </Routes>
      </Suspense>
    </MemoryRouter>,
  );
}

describe('ProductConversationAliasRedirect', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.resolveCoordinatorRoute).mockResolvedValue({ coordinator_id: null });
  });

  it('routes a historical Global member before ordinary snapshot lookup and retains its anchor', async () => {
    vi.mocked(api.resolveCoordinatorRoute).mockResolvedValue({ coordinator_id: 'current-global' });
    renderAlias('historical-global', '/c/historical-global#tool-source-call');
    expect(await screen.findByTestId('location')).toHaveTextContent('/global/historical-global#tool-source-call');
    expect(api.getProductConversationSnapshot).not.toHaveBeenCalled();
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it.each([
    { alias: 'deep-link', lifecycle: 'open', archived: true },
    { alias: 'search-result', lifecycle: 'history', archived: false },
    { alias: 'segment-alias', lifecycle: 'open', archived: true },
  ] as const)('redirects $alias to authoritative aggregate lifecycle ownership', async ({ alias, lifecycle }) => {
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValueOnce({
      canonical_route: '/product-conversations/product-1',
      ordinary_lifecycle: lifecycle,
    } as never);

    renderAlias(alias, `/c/${alias}?from=search#message-m-1`);

    expect(await screen.findByTestId('location')).toHaveTextContent(
      '/product-conversations/product-1?from=search#message-m-1',
    );
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it('renders a bare canonical product route without redirecting to a historical row', async () => {
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      latest_transcript_row_id: 'successor', requested_transcript_row_id: 'root',
    } as never);
    renderAlias('product-1');
    expect(await screen.findByTestId('product-page')).toHaveTextContent('product-1');
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it('pins an explicit transcript source on direct load instead of rendering the latest aggregate', async () => {
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      latest_transcript_row_id: 'successor', requested_transcript_row_id: 'historical',
    } as never);
    renderAlias('historical', '/c/historical?source_transcript=historical&source_tool=send#message-source');
    await screen.findByTestId('embedded-fallback');
    expect(embeddedSpy.mock.lastCall?.[0]).toEqual(expect.objectContaining({ slug: 'historical', suppressCanonicalization: true }));
    expect(screen.queryByTestId('product-page')).toBeNull();
  });

  it.each(['/c/product-1', '/product-conversations/product-1', '/c/legacy-slug'])('honors an encoded predecessor pin on %s', async (path) => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async (reference) => ({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      requested_transcript_row_id: reference === 'old:member' ? 'old:member' : 'root', latest_transcript_row_id: 'successor',
    } as never));
    renderAlias(path.split('/').at(-1)!, `${path}?source_transcript=old%3Amember&viewer=inspect#message-old%3Amsg`);
    await screen.findByTestId('embedded-fallback');
    expect(embeddedSpy.mock.lastCall?.[0]).toEqual(expect.objectContaining({ slug: 'old:member' }));
    expect(screen.getByTestId('location')).toHaveTextContent('source_transcript=old%3Amember&viewer=inspect#message-old%3Amsg');
  });

  it.each(['', 'alien', 'old&source_transcript=other'])('rejects invalid or nonmember pin %s without opening latest', async (pin) => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async (reference) => ({
      product_conversation_id: reference === 'product-1' ? 'product-1' : 'another-product',
      canonical_route: '/c/product-1', ordinary_lifecycle: 'open', requested_transcript_row_id: reference,
    } as never));
    renderAlias('product-1', `/c/product-1?source_transcript=${pin}`);
    await screen.findByRole('alert');
    expect(embeddedSpy).not.toHaveBeenCalled();
    expect(screen.queryByTestId('product-page')).toBeNull();
  });

  it('retains ordinary non-aggregate direct-route behavior after an authoritative 404', async () => {
    vi.mocked(api.getProductConversationSnapshot)
      .mockRejectedValueOnce(new ApiResponseError('not aggregate', 404));

    renderAlias('ordinary-row');

    await screen.findByTestId('embedded-fallback');
    await waitFor(() => expect(embeddedSpy).toHaveBeenCalled());
    expect(embeddedSpy.mock.lastCall?.[0]).toEqual(expect.objectContaining({
      slug: 'ordinary-row',
      mutationEnabled: true,
    }));
    expect(embeddedSpy.mock.lastCall?.[0]).not.toHaveProperty('aggregateLifecycleOpen');
  });

  it('keeps an unresolved aggregate fallback read-only on transport failure', async () => {
    vi.mocked(api.getProductConversationSnapshot).mockRejectedValueOnce(new Error('offline'));

    renderAlias('unknown-row');

    await screen.findByTestId('embedded-fallback');
    expect(embeddedSpy.mock.lastCall?.[0]).toEqual(expect.objectContaining({
      mutationEnabled: false,
      aggregateLifecycleOpen: false,
    }));
  });
});
