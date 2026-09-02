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
    api: { ...actual.api, getProductConversationSnapshot: vi.fn() },
  };
});

vi.mock('./pages/ConversationPage', () => ({
  EmbeddedConversationPage: (props: unknown) => {
    embeddedSpy(props);
    return <div data-testid="embedded-fallback" />;
  },
}));

function Location() {
  const location = useLocation();
  return <div data-testid="location">{location.pathname}{location.search}{location.hash}</div>;
}

function renderAlias(reference: string, entry = `/c/${reference}`) {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <Suspense fallback={null}>
        <Routes>
          <Route path="/c/:slug" element={<ProductConversationAliasRedirect reference={reference} />} />
          <Route path="/product-conversations/:id" element={<Location />} />
        </Routes>
      </Suspense>
    </MemoryRouter>,
  );
}

describe('ProductConversationAliasRedirect', () => {
  beforeEach(() => {
    vi.clearAllMocks();
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
