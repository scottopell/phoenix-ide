import { Suspense, useState } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ProductConversationAliasRedirect, ConversationRouteRedirect } from './App';
import { api, ApiResponseError } from './api';
import { ViewJourneyLifecycle } from './hooks/conversationViewLifecycle';
import { MarkdownRenderer } from './components/MarkdownRenderer';
import { CONVERSATION_MARKDOWN_URL_TRANSFORM, createConversationMarkdownComponents } from './components/conversationMarkdownImages';

function ReplaceRoute({ target }: { target: string }) {
  const navigate = useNavigate();
  return <><button onClick={() => navigate(target)}>Replace route</button><button onClick={() => navigate(-1)}>Back</button><button onClick={() => navigate(1)}>Forward</button></>;
}

const embeddedSpy = vi.fn();
const productSpy = vi.fn();

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
  ProductConversationPage: ({ productId, journey }: { productId: string; journey: ViewJourneyLifecycle }) => {
    productSpy(productId, journey);
    const [localState, setLocalState] = useState(0);
    return <div data-testid="product-page">{productId}<button type="button" onClick={() => setLocalState((value) => value + 1)}>Local {localState}</button></div>;
  },
}));

function Location() {
  const location = useLocation();
  const navigate = useNavigate();
  return <><div data-testid="location">{location.pathname}{location.search}{location.hash}</div>
    <output data-testid="location-state">{JSON.stringify(location.state)}</output>
    <button type="button" onClick={() => navigate({ pathname: location.pathname, search: '?viewer=file', hash: '#message-m-1' })}>Open viewer</button>
    <button type="button" onClick={() => navigate(location.pathname, { state: { chapterSelection: 'pending' } })}>Select pending prompt</button></>;
}

function renderAlias(reference: string, entry = `/c/${reference}`) {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <Location />
      <Suspense fallback={null}>
        <Routes>
          <Route path="/c/:slug" element={<ConversationRouteRedirect />} /><Route path="/transcript" element={<ConversationRouteRedirect />} />
          <Route path="/product-conversations/:id" element={entry.startsWith('/product-conversations/') ? <ProductConversationAliasRedirect reference={reference} /> : null} />
          <Route path="/global/:slug?" element={null} />
        </Routes>
      </Suspense>
    </MemoryRouter>,
  );
}

describe('ProductConversationAliasRedirect', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 204 })));
    vi.spyOn(Math, 'random').mockReturnValue(0);
    vi.mocked(api.resolveCoordinatorRoute).mockResolvedValue({ coordinator_id: null });
  vi.spyOn(api, 'getConversation').mockImplementation(async (id) => ({ conversation: { id } } as never));
  });

  afterEach(async () => {
    cleanup();
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('reports a failed admission before any product page mounts without leaking the exact pin', async () => {
    renderAlias('private-reference', '/c/private-reference?source_transcript=private-pin&source_transcript=another-private-pin');
    await screen.findByRole('alert');
    await waitFor(() => expect(fetch).toHaveBeenCalled());
    const call = vi.mocked(fetch).mock.calls.find(([url]) => url === '/api/telemetry/conversation-view');
    expect(call).toBeDefined();
    const report = JSON.parse(call![1]!.body as string);
    expect(report).toMatchObject({ route: 'ordinary', outcome: 'failed', marks: { meaningful_transcript: null, anchor_settled: null } });
    expect(call![1]!.body).not.toMatch(/private-reference|private-pin|source_transcript/);
    expect(api.getProductConversationSnapshot).not.toHaveBeenCalled();
  });

  it('detaches the preceding product from the replacement journey while admission is pending', async () => {
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({ product_conversation_id: 'a', canonical_route: '/c/a' } as never);
    render(<MemoryRouter initialEntries={['/c/a']}><ReplaceRoute target="/c/b" /><Suspense fallback={null}><Routes><Route path="/c/:slug" element={<ConversationRouteRedirect />} /><Route path="/transcript" element={<ConversationRouteRedirect />} /></Routes></Suspense></MemoryRouter>);
    const retained = await screen.findByTestId('product-page');
    const precedingJourney = productSpy.mock.lastCall![1];
    let release!: (value: { coordinator_id: null }) => void;
    vi.mocked(api.resolveCoordinatorRoute).mockReturnValueOnce(new Promise(resolve => { release = resolve; }));
    fireEvent.click(screen.getByText('Replace route'));
    expect(retained).not.toBeVisible();
    expect(productSpy.mock.lastCall).toEqual(['a', precedingJourney]);
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({ product_conversation_id: 'b', canonical_route: '/c/b' } as never);
    await act(async () => release({ coordinator_id: null }));
    expect(await screen.findByTestId('product-page')).toHaveTextContent('b');
    expect(productSpy.mock.lastCall![1]).not.toBe(precedingJourney);
  });

  it.each([
    { target: '/c/member?source_transcript=member#message-source', explicitPin: true },
    { target: '/c/member#message-source', explicitPin: false },
    { target: '/transcript?source_transcript=member#message-source', explicitPin: true },
    { target: '/%74ranscript?source_transcript=member#message-source', explicitPin: true },
    { target: '/transcrip%74?source_transcript=member#message-source', explicitPin: true },
    { target: '/product-conversations/a?source_transcript=member#message-source', explicitPin: true },
    { target: '/chains/member?source_transcript=member#message-source', explicitPin: true },
  ])('retains the same product reader across $target and Back/Forward without lending unresolved hydration', async ({ target, explicitPin }) => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async id => ({ product_conversation_id: 'a', canonical_route: '/c/a', requested_transcript_row_id: id } as never));
    render(<MemoryRouter initialEntries={['/c/a']}><ReplaceRoute target={target} /><Suspense fallback={null}><Routes>
      <Route path="/c/:slug" element={<ConversationRouteRedirect />} />
      <Route path="/transcript" element={<ConversationRouteRedirect />} />
      <Route path="/product-conversations/:slug" element={<ConversationRouteRedirect />} />
      <Route path="/chains/:rootConvId" element={<ConversationRouteRedirect />} />
    </Routes></Suspense></MemoryRouter>);
    const owner = await screen.findByTestId('product-page');
    fireEvent.click(screen.getByRole('button', { name: 'Local 0' }));
    const oldJourney = productSpy.mock.lastCall![1];
    let release!: (value: { coordinator_id: null }) => void;
    vi.mocked(api.resolveCoordinatorRoute).mockReturnValueOnce(new Promise(resolve => { release = resolve; }));
    let releasePin!: (value: { coordinator_id: null }) => void;
    if (!explicitPin) vi.mocked(api.resolveCoordinatorRoute).mockReturnValueOnce(new Promise(resolve => { releasePin = resolve; }));
    fireEvent.click(screen.getByText('Replace route'));
    expect(screen.getByTestId('product-page')).toBe(owner);
    expect(owner).not.toBeVisible();
    expect(productSpy.mock.lastCall![1]).toBe(oldJourney);
    await act(async () => release({ coordinator_id: null }));
    if (!explicitPin) {
      expect(screen.getByTestId('product-page')).toBe(owner);
      expect(owner).not.toBeVisible();
      expect(productSpy.mock.lastCall![1]).toBe(oldJourney);
      await act(async () => releasePin({ coordinator_id: null }));
    }
    await waitFor(() => expect(owner).toBeVisible());
    expect(screen.getByRole('button', { name: 'Local 1' })).toBeInTheDocument();
    expect(productSpy.mock.lastCall![1]).not.toBe(oldJourney);
    fireEvent.click(screen.getByText('Back'));
    await waitFor(() => expect(owner).toBeVisible());
    expect(screen.getByTestId('product-page')).toBe(owner);
    expect(screen.getByRole('button', { name: 'Local 1' })).toBeInTheDocument();
    fireEvent.click(screen.getByText('Forward'));
    await waitFor(() => expect(owner).toBeVisible());
    expect(screen.getByTestId('product-page')).toBe(owner);
    expect(screen.getByRole('button', { name: 'Local 1' })).toBeInTheDocument();
  });

  it.each(['pin', 'fallback'])('does not fail a replacement journey with the preceding %s error', async kind => {
    const finish = vi.spyOn(ViewJourneyLifecycle.prototype, 'finish');
    vi.mocked(api.getProductConversationSnapshot).mockRejectedValue(new Error('offline'));
    const initial = kind === 'pin' ? '/c/a?source_transcript=bad&source_transcript=other' : '/c/a';
    render(<MemoryRouter initialEntries={[initial]}><ReplaceRoute target="/c/a?source_transcript=valid" /><Suspense fallback={null}><Routes><Route path="/c/:slug" element={<ConversationRouteRedirect />} /><Route path="/transcript" element={<ConversationRouteRedirect />} /></Routes></Suspense></MemoryRouter>);
    await screen.findByRole('alert');
    expect(finish.mock.calls.filter(([outcome]) => outcome === 'failed')).toHaveLength(1);
    finish.mockClear();
    let release!: (value: { coordinator_id: null }) => void;
    vi.mocked(api.resolveCoordinatorRoute).mockReturnValueOnce(new Promise(resolve => { release = resolve; }));
    fireEvent.click(screen.getByText('Replace route'));
    expect(finish.mock.calls.filter(([outcome]) => outcome === 'failed')).toHaveLength(0);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async id => ({ product_conversation_id: 'a', canonical_route: '/c/a', requested_transcript_row_id: id } as never));
    await act(async () => release({ coordinator_id: null }));
    await screen.findByTestId('product-page');
    expect(finish.mock.calls.filter(([outcome]) => outcome === 'failed')).toHaveLength(0);
  });

  it.each(['available', 'missing'] as const)('routes the reported Proposal Markdown citation through alias admission (%s target)', async availability => {
    const transcript = 'aceb02b4-0735-491e-bd5c-854e844f1591';
    const origin = '/global/coordinator?viewer=inspect#message-origin';
    const destination = `/transcript?source_transcript=${transcript}#message-ee6445ef-fc3e-451b-989a-1fa18225aaa2`;
    const returnState = { conversationReturnOrigin: { kind: 'coordinator', href: origin } };
    let resolveRoute!: (route: { coordinator_id: null }) => void;
    vi.mocked(api.resolveCoordinatorRoute).mockReturnValueOnce(new Promise(resolve => { resolveRoute = resolve; }));
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async reference => {
      if (availability === 'missing') throw new ApiResponseError('Transcript not found', 404);
      return {
        product_conversation_id: 'proposal-product', canonical_route: '/c/proposal-product', ordinary_lifecycle: 'open',
        requested_transcript_row_id: reference, latest_transcript_row_id: 'successor', writable_transcript_row_id: 'successor',
      } as never;
    });
    render(<MemoryRouter initialEntries={[origin]}>
      <Location />
      <Suspense fallback={null}><Routes>
        <Route path="/global/:slug" element={
          <MarkdownRenderer mode="conversation" components={createConversationMarkdownComponents()} urlTransform={CONVERSATION_MARKDOWN_URL_TRANSFORM}>
            {'[Proposal](@transcript:aceb02b4-0735-491e-bd5c-854e844f1591#message-ee6445ef-fc3e-451b-989a-1fa18225aaa2)'}
          </MarkdownRenderer>
        } />
        <Route path="/c/:slug" element={<ConversationRouteRedirect />} /><Route path="/transcript" element={<ConversationRouteRedirect />} />
      </Routes></Suspense>
    </MemoryRouter>);
    const citation = screen.getByRole('link', { name: 'Proposal' });
    expect(citation).toHaveAttribute('href', destination);
    expect(citation).not.toHaveAttribute('target');
    fireEvent.click(citation);
    expect(screen.getByTestId('location').textContent).toBe(destination);
    expect(JSON.parse(screen.getByTestId('location-state').textContent!)).toEqual(returnState);
    expect(api.resolveCoordinatorRoute).toHaveBeenCalledWith(transcript);
    expect(api.getProductConversationSnapshot).not.toHaveBeenCalled();
    await act(async () => resolveRoute({ coordinator_id: null }));
    if (availability === 'available') {
      expect(await screen.findByTestId('product-page')).toHaveTextContent('proposal-product');
      await waitFor(() => expect(screen.getByTestId('location').textContent).toBe(destination.replace('/transcript', '/c/proposal-product')));
      expect(vi.mocked(api.getProductConversationSnapshot).mock.calls.map(([reference]) => reference)).toEqual([
        transcript, transcript, 'proposal-product', transcript,
      ]);
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    } else {
      expect(await screen.findByRole('alert')).toHaveTextContent('Exact transcript unavailable or not a member of this conversation.');
      expect(screen.queryByTestId('product-page')).not.toBeInTheDocument();
      expect(screen.queryByRole('link', { name: 'Proposal' })).not.toBeInTheDocument();
    }
    expect(embeddedSpy).not.toHaveBeenCalled();
    expect(screen.getByTestId('location').textContent).toBe(availability === 'available' ? destination.replace('/transcript', '/c/proposal-product') : destination);
    expect(JSON.parse(screen.getByTestId('location-state').textContent!)).toEqual(returnState);
  });

  it.each(['café', 'caf%C3%A9', '%', '%25', 'old%2Frow', 'old%2frow', 'row/other?x=y', '[brackets]', '.', '..'])('admits opaque exact query entry without changing its identity: %s', async id => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async reference => ({
      product_conversation_id: 'exact-product', canonical_route: '/c/readable-name', requested_transcript_row_id: reference,
    } as never));
    const entry = `/transcript?source_transcript=${encodeURIComponent(id)}&viewer=inspect#message-${encodeURIComponent(id)}`;
    const url = new URL(entry, 'https://phoenix.test');
    renderAlias(id, `${url.pathname}${url.search}${url.hash}`);
    expect(await screen.findByTestId('product-page')).toHaveTextContent('exact-product');
    await waitFor(() => expect(screen.getByTestId('location').textContent).toBe(entry.replace('/transcript', '/c/exact-product')));
    expect(vi.mocked(api.getProductConversationSnapshot).mock.calls.map(([reference]) => reference)).toEqual([
      id, id, 'exact-product', id,
    ]);
    expect(api.resolveCoordinatorRoute).toHaveBeenCalledWith(id);
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it.each(['/TRANSCRIPT', '/transcript//', '/%74ranscript', '/transcrip%74'])('uses exact admission for the router-matched spelling %s', async pathname => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async reference => ({
      product_conversation_id: 'exact-product', canonical_route: '/c/readable-name', requested_transcript_row_id: reference,
    } as never));
    renderAlias('unused', `${pathname}?source_transcript=exact-member&viewer=inspect#message-source`);
    expect(await screen.findByTestId('product-page')).toHaveTextContent('exact-product');
    await waitFor(() => expect(screen.getByTestId('location').textContent).toBe('/c/exact-product?source_transcript=exact-member&viewer=inspect#message-source'));
    expect(vi.mocked(api.resolveCoordinatorRoute).mock.calls.map(([reference]) => reference)).toEqual(['exact-member', 'exact-product']);
    expect(vi.mocked(api.getProductConversationSnapshot).mock.calls.map(([reference]) => reference)).toEqual([
      'exact-member', 'exact-member', 'exact-product', 'exact-member',
    ]);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it.each(['old%2Frow', 'old%2frow', 'café', 'caf%C3%A9', 'x/y?z=1', '%'])('reads the actual raw route once instead of trusting React Router param decoding: %s', async id => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async reference => ({
      product_conversation_id: 'product', canonical_route: '/c/product', requested_transcript_row_id: reference,
    } as never));
    renderAlias(id, `/c/${encodeURIComponent(id)}?source_transcript=${encodeURIComponent(id)}`);
    await screen.findByTestId('product-page');
    expect(api.resolveCoordinatorRoute).toHaveBeenCalledWith(id);
    for (const [reference] of vi.mocked(api.getProductConversationSnapshot).mock.calls) expect(reference).toBe(id);
  });

  it.each(['', '?source_transcript=', '?source_transcript=a&source_transcript=a', '?source_transcript=has%20space', '?source_transcript=has%23hash', '?source_transcript=%FF', '?source_transcript=%ZZ'])('rejects malformed/missing dedicated entry %s before resolution', async search => {
    renderAlias('unused', `/transcript${search}`);
    expect(await screen.findByRole('alert')).toHaveTextContent('Invalid exact transcript reference.');
    expect(api.resolveCoordinatorRoute).not.toHaveBeenCalled();
    expect(api.getProductConversationSnapshot).not.toHaveBeenCalled();
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  describe.each(['/%74ranscript', '/transcrip%74'])('router-matched exact entry %s', pathname => {
    it.each(['', '?source_transcript=', '?source_transcript=a&source_transcript=a', '?source_transcript=a&source_transcript=b', '?source_transcript=%FF', '?source_transcript=%ZZ', '?source_transcript=has%20space'])('visibly rejects missing or invalid selector %s before admission', async search => {
      renderAlias('unused', `${pathname}${search}`);
      expect(await screen.findByRole('alert')).toBeVisible();
      expect(screen.getByRole('alert')).toHaveTextContent('Invalid exact transcript reference.');
      expect(api.resolveCoordinatorRoute).not.toHaveBeenCalled();
      expect(api.getProductConversationSnapshot).not.toHaveBeenCalled();
      expect(api.getConversation).not.toHaveBeenCalled();
      expect(productSpy).not.toHaveBeenCalled();
      expect(embeddedSpy).not.toHaveBeenCalled();
      expect(screen.getByTestId('location').textContent).toBe(`${pathname}${search}`);
    });
  });

  it.each(['', '?source_transcript=a&source_transcript=a', '?source_transcript=%FF'])('does not reuse the admitted product for an invalid external exact entry %s', async search => {
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({ product_conversation_id: 'a', canonical_route: '/c/a' } as never);
    render(<MemoryRouter initialEntries={['/c/a']}><ReplaceRoute target={`/transcript${search}`} /><Suspense fallback={null}>
      <Routes>
        <Route path="/c/:slug" element={<ConversationRouteRedirect />} />
        <Route path="/transcript" element={<ConversationRouteRedirect />} />
      </Routes>
    </Suspense></MemoryRouter>);
    await screen.findByTestId('product-page');
    vi.mocked(api.getProductConversationSnapshot).mockClear();
    vi.mocked(api.resolveCoordinatorRoute).mockClear();
    fireEvent.click(screen.getByText('Replace route'));
    expect(await screen.findByRole('alert')).toHaveTextContent('Invalid exact transcript reference.');
    expect(screen.queryByTestId('product-page')).not.toBeInTheDocument();
    expect(api.resolveCoordinatorRoute).not.toHaveBeenCalled();
    expect(api.getProductConversationSnapshot).not.toHaveBeenCalled();
  });

  it('does not treat an external pending marker as authority for a missing exact selector', async () => {
    render(<MemoryRouter initialEntries={[{ pathname: '/transcript', state: { chapterSelection: 'pending' } }]}>
      <ConversationRouteRedirect />
    </MemoryRouter>);
    expect(await screen.findByRole('alert')).toHaveTextContent('Invalid exact transcript reference.');
    expect(api.resolveCoordinatorRoute).not.toHaveBeenCalled();
    expect(api.getProductConversationSnapshot).not.toHaveBeenCalled();
  });

  it('does not let an independently valid opaque pin override an unrelated route reference', async () => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async reference => ({
      product_conversation_id: reference === 'unrelated' ? 'product-a' : 'product-b',
      requested_transcript_row_id: reference, canonical_route: '/c/product-a',
    } as never));
    renderAlias('unrelated', '/c/unrelated?source_transcript=valid%252Fpin');
    await screen.findByRole('alert');
    expect(api.getProductConversationSnapshot).toHaveBeenCalledWith('unrelated', expect.anything());
    expect(api.getProductConversationSnapshot).toHaveBeenCalledWith('valid%2Fpin', expect.anything());
    expect(screen.queryByTestId('product-page')).toBeNull();
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it('routes a historical Global member before ordinary snapshot lookup and retains its anchor', async () => {
    let finishRoute!: (route: { coordinator_id: string }) => void;
    vi.mocked(api.resolveCoordinatorRoute).mockImplementation(() => new Promise((resolve) => { finishRoute = resolve; }));
    renderAlias('historical-global', '/c/historical-global#tool-source-call');
    expect(screen.getByTestId('location')).toHaveTextContent('/c/historical-global#tool-source-call');
    await act(async () => { finishRoute({ coordinator_id: 'current-global' }); });
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent('/global/historical-global#tool-source-call'));
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

    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent(
      '/product-conversations/product-1?from=search#message-m-1',
    ));
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it('preserves the canonical product page across viewer and anchor navigation', async () => {
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      latest_transcript_row_id: 'latest', writable_transcript_row_id: 'latest', requested_transcript_row_id: 'latest',
    } as never);
    renderAlias('product-1');
    const local = await screen.findByRole('button', { name: 'Local 0' });
    fireEvent.click(local);
    expect(screen.getByRole('button', { name: 'Local 1' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Open viewer' }));
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent('/c/product-1?viewer=file#message-m-1'));
    expect(screen.getByRole('button', { name: 'Local 1' })).toBeInTheDocument();
    expect(api.getProductConversationSnapshot).toHaveBeenCalledTimes(1);
  });

  it.each(['root-global', 'historical-global', 'current-global'])('pins Global member %s instead of ordinary navigation', async (pin) => {
    vi.mocked(api.resolveCoordinatorRoute).mockResolvedValue({ coordinator_id: 'current-global' });
    renderAlias('global-alias', `/c/global-alias?source_transcript=${pin}&viewer=inspect#message-old%3Amsg`);
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent(`/global?source_transcript=${pin}&viewer=inspect#message-old%3Amsg`));
    expect(api.getProductConversationSnapshot).not.toHaveBeenCalled();
  });

  it('rejects a foreign Global pin rather than opening latest or an ordinary page', async () => {
    vi.mocked(api.resolveCoordinatorRoute).mockImplementation(async (reference) => ({ coordinator_id: reference === 'global-alias' ? 'current-global' : null }));
    renderAlias('global-alias', '/c/global-alias?source_transcript=ordinary-row');
    await screen.findByRole('alert');
    expect(api.getProductConversationSnapshot).not.toHaveBeenCalled();
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it('revalidates a copied historical URL on remount instead of retaining latest state', async () => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async (reference) => ({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      requested_transcript_row_id: reference, latest_transcript_row_id: 'successor',
    } as never));
    const entry = '/c/product-1?source_transcript=old-member#message-old%3Amessage';
    const first = renderAlias('product-1', entry);
    await screen.findByTestId('product-page');
    first.unmount();
    renderAlias('product-1', entry);
    await screen.findByTestId('product-page');
    expect(api.getProductConversationSnapshot).toHaveBeenCalledWith('old-member', expect.objectContaining({ message_limit: 1, open_id: expect.any(String), onTelemetry: expect.any(Function) }));
    expect(embeddedSpy).not.toHaveBeenCalled();
    expect(screen.getByTestId('location')).toHaveTextContent(entry);
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

  it.each(['historical', 'legacy-slug'])('preserves aggregate navigation for long compatibility reference %s', async (reference) => {
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      latest_transcript_row_id: 'successor', writable_transcript_row_id: 'successor',
      requested_transcript_row_id: reference === 'historical' ? reference : 'historical',
    } as never);
    renderAlias(reference, `/product-conversations/${reference}?viewer=inspect#details`);
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent('/c/product-1?viewer=inspect#details'));
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it('pins an explicit transcript source on direct load instead of rendering the latest aggregate', async () => {
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      latest_transcript_row_id: 'successor', writable_transcript_row_id: 'successor', requested_transcript_row_id: 'historical',
    } as never);
    renderAlias('historical', '/c/historical?source_transcript=historical&source_tool=send#message-source');
    await screen.findByTestId('product-page');
    expect(embeddedSpy).not.toHaveBeenCalled();
    expect(screen.getByTestId('location')).toHaveTextContent('/c/historical?source_transcript=historical&source_tool=send#message-source');
  });

  it.each(['/c/historical', '/transcript'])('does not restore an exact historical alias pin after explicit pending prompt selection from %s', async entry => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async reference => ({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      latest_transcript_row_id: 'successor', writable_transcript_row_id: 'successor', requested_transcript_row_id: reference,
    } as never));
    renderAlias('historical', `${entry}?source_transcript=historical#message-old`);
    const owner = await screen.findByTestId('product-page');
    fireEvent.click(screen.getByRole('button', { name: 'Select pending prompt' }));
    await waitFor(() => expect(screen.getByTestId('location')).toHaveTextContent('/c/product-1'));
    expect(screen.getByTestId('location')).not.toHaveTextContent('source_transcript');
    expect(screen.getByTestId('product-page')).toBe(owner);
  });

  it('keeps an open current member live while direct messages are temporarily blocked', async () => {
    vi.mocked(api.getProductConversationSnapshot).mockResolvedValue({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      latest_transcript_row_id: 'current-member', writable_transcript_row_id: null, requested_transcript_row_id: 'current-member',
    } as never);
    renderAlias('current-member', '/c/current-member?source_transcript=current-member');
    await screen.findByTestId('product-page');
    expect(embeddedSpy).not.toHaveBeenCalled();
  });

  it.each(['/c/product-1', '/product-conversations/product-1', '/c/legacy-slug'])('honors an encoded predecessor pin on %s', async (path) => {
    vi.mocked(api.getProductConversationSnapshot).mockImplementation(async (reference) => ({
      product_conversation_id: 'product-1', canonical_route: '/c/product-1', ordinary_lifecycle: 'open',
      requested_transcript_row_id: reference === 'old:member' ? 'old:member' : 'root', latest_transcript_row_id: 'successor',
    } as never));
    renderAlias(path.split('/').at(-1)!, `${path}?source_transcript=old%3Amember&viewer=inspect#message-old%3Amsg`);
    await screen.findByTestId('product-page');
    expect(embeddedSpy).not.toHaveBeenCalled();
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
