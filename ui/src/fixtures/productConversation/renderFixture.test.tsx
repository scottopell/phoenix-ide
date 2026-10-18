import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api } from '../../api';
import { InlineReactionStore } from '../../conversation/InlineReactionStore';
import { ProductConversationFixture } from './renderFixture';
import { getProductConversationScenario } from './scenarios';
import { makeCompactEventsScenario } from './eventCards';
import { installProductConversationFixtureApi } from './mockApi';

vi.mock('../../cache', () => ({
  cacheDB: {
    init: vi.fn(() => Promise.resolve()),
    getPendingOps: vi.fn(() => Promise.resolve([])),
    getAllConversations: vi.fn(() => Promise.resolve([])),
    getConversation: vi.fn(() => Promise.resolve(null)),
    getConversationBySlug: vi.fn(() => Promise.resolve(null)),
    putConversation: vi.fn(() => Promise.resolve()),
    syncConversations: vi.fn(() => Promise.resolve()),
  },
}));

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const productConversationCss = readFileSync(`${process.cwd()}/src/pages/ProductConversationPage.css`, 'utf8');

describe('ProductConversationFixture', () => {
  it.each(['ordinary-lazy-history', 'global-lazy-history'] as const)('keeps an explicit loaded prompt selection when the %s unloaded-boundary read resolves', async (id) => {
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(1000);
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({ x: 0, y: 0, top: 0, left: 0, right: 960, bottom: 1000, width: 960, height: 1000, toJSON: () => ({}) });
    const { container } = render(<ProductConversationFixture scenario={getProductConversationScenario(id)} />);
    await waitFor(() => expect(container.querySelector(`[data-product-conversation-fixture-ready="${id}"]`)).not.toBeNull());
    act(() => window.dispatchEvent(new Event('product-conversation-fixture-hold-older')));
    fireEvent.click(screen.getByRole('button', { name: 'Load context boundary 2' }));
    await waitFor(() => expect(document.documentElement.dataset['productConversationFixtureOlderSnapshotRequests']).toBe('1'));
    fireEvent.click(await screen.findByRole('button', { name: /Member 3 request$/ }));
    await waitFor(() => expect(document.documentElement.dataset['sourceFixtureLocation']).toContain('#message-history-prompt-3'));
    act(() => window.dispatchEvent(new Event('product-conversation-fixture-release-older')));
    await screen.findByRole('button', { name: 'Context boundary 2' });
    expect(document.documentElement.dataset['sourceFixtureLocation']).toContain('#message-history-prompt-3');
    expect(document.documentElement.dataset['productConversationFixtureOlderSnapshotRequests']).toBe('1');
    expect(screen.getByPlaceholderText('Type a message...')).toBeEnabled();
  });

  it.each(['source-call-ordinary', 'source-call-global'] as const)('resolves the real %s alias and expands its exact source call', async (id) => {
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(1000);
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({ x: 0, y: 0, top: 0, left: 0, right: 960, bottom: 1000, width: 960, height: 1000, toJSON: () => ({}) });
    render(<ProductConversationFixture scenario={getProductConversationScenario(id)} />);
    fireEvent.click(await screen.findByRole('link', { name: /transcript ID source-member · source call$/ }));
    await waitFor(() => expect(document.documentElement.dataset['sourceFixtureLocation']).toContain(id === 'source-call-global' ? '/global/source-member?' : '/c/source-member?'));
    expect(document.documentElement.dataset['sourceFixtureLocation']).toContain('source_transcript=source-member');
    await waitFor(() => expect(document.querySelector('[data-tool-id="source-send-call"]')?.textContent).toContain('Source jump fixture payload'));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('keeps the fixture default metadata projection distinct from web history without live API fallback', async () => {
    const scenario = getProductConversationScenario('ordinary-lazy-history');
    const { container } = render(<ProductConversationFixture scenario={scenario} />);
    await waitFor(() => expect(container.querySelector('[data-product-conversation-fixture-ready="ordinary-lazy-history"]')).not.toBeNull());
    const snapshot = await api.getProductConversationSnapshot('fixture-product-conversation', { message_limit: 1 });
    expect(snapshot).not.toHaveProperty('boundaries');
    expect(snapshot.segments[0]?.handoff?.summary).toBe('Preserved summary from member 1');
    expect(snapshot.segments[1]?.handoff?.summary).toBe('Preserved summary from member 2');
    expect(snapshot.segments.flatMap(segment => segment.messages).some(message => message.message_type === 'continuation')).toBe(false);
  });

  it.each(['ordinary-lazy-history', 'global-lazy-history'] as const)('loads both older cursor pages through the real %s adapter and keeps newest composer ownership', async (id) => {
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(1000);
    const scenario = getProductConversationScenario(id);
    const { container } = render(<ProductConversationFixture scenario={scenario} />);
    await waitFor(() => expect(container.querySelector(`[data-product-conversation-fixture-ready="${id}"]`)).not.toBeNull());
    const history = vi.spyOn(api, id === 'global-lazy-history' ? 'getCoordinatorHistory' : 'getProductConversationHistorySnapshot');
    const legacy = vi.spyOn(api, 'getProductConversationSnapshot');
    expect(document.documentElement.dataset['productConversationFixtureOlderSnapshotRequests']).toBeUndefined();
    fireEvent.click(screen.getByRole('button', { name: 'Load context boundary 1' }));
    await waitFor(() => expect(document.documentElement.dataset['productConversationFixtureOlderSnapshotRequests']).toBe('2'));
    expect(history.mock.calls.map(call => call[1]?.before)).toEqual(['history-before-3', 'history-before-2']);
    expect(legacy).not.toHaveBeenCalled();
    expect(await screen.findByRole('button', { name: 'Context boundary 1' })).toBeInTheDocument();
    expect(document.documentElement.dataset['sourceFixtureLocation']).toContain('#message-continue-history-1');
    expect(document.documentElement.dataset['productConversationFixtureEventSourceLastUrl']).toContain('history-3');
    expect(screen.getByPlaceholderText('Type a message...')).toBeEnabled();
    if (id === 'global-lazy-history') {
      expect(scenario.snapshot).not.toHaveProperty('ordinary_lifecycle');
      expect(scenario.snapshot).not.toHaveProperty('close');
    }
  });

  it('keeps the Global event receiver distinct from the ordinary source and serves only exercised status reads', async () => {
    const scenario = makeCompactEventsScenario(true);
    const originalFetch = globalThis.fetch;
    const restore = installProductConversationFixtureApi(scenario);
    try {
      expect(scenario.initialEntry).toBe('/global/event-latest');
      expect(scenario.alignedLatestMessages).toHaveLength(4);
      expect(await api.resolveCoordinatorRoute('event-latest')).toEqual({ coordinator_id: 'event-latest' });
      expect(await api.resolveCoordinatorRoute('source-member')).toEqual({ coordinator_id: null });
      const source = await api.getProductConversationSnapshot('source-member');
      expect(source.product_conversation_id).toBe('source-member');
      expect(source.segments[0]!.messages[0]!.content).toEqual({ text: 'Source conversation for compact event acceptance.' });
      expect(await api.listActiveCoordinatorWatches()).toEqual([]);
      expect(await api.listLiveCoordinatorBashHandles()).toEqual([]);
      expect(document.documentElement.dataset['productConversationFixtureWatchRequests']).toBe('1');
      expect(document.documentElement.dataset['productConversationFixtureLiveBashRequests']).toBe('1');
      await expect(fetch('/api/coordinator/bash/live', { method: 'POST' })).rejects.toThrow('Unexpected fixture API request');
      await expect(fetch('/api/unexpected')).rejects.toThrow('Unexpected fixture API request');
    } finally {
      restore();
    }
    expect(globalThis.fetch).toBe(originalFetch);
  });

  it('preserves origin-aware navigation after mounting a continued transcript again', async () => {
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(1000);
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this.id === 'messages' || this.classList.contains('render-unit')) return { x: 0, y: 0, top: 0, left: 0, right: 900, bottom: 1000, width: 900, height: 1000, toJSON: () => ({}) };
      return originalRect.call(this);
    });
    for (let mount = 0; mount < 2; mount += 1) {
      const fixture = render(<ProductConversationFixture scenario={getProductConversationScenario('input-provenance-continuation')} />);
      await waitFor(() => expect(fixture.container.querySelector('[data-product-conversation-fixture-ready="input-provenance-continuation"]')).not.toBeNull());
      expect(screen.getByRole('button', { name: 'User · API: API request in original transcript' })).toHaveClass('user');
      expect(screen.getByRole('button', { name: /From conversation ID source-product.*Forwarded into continued transcript/ })).toHaveClass('meta');
      expect(await screen.findByRole('button', { name: /From conversation ID source-product.*Queued from source conversation/ })).toHaveClass('meta');
      expect(screen.queryByTitle('Your message')).not.toBeInTheDocument();
      fixture.unmount();
    }
  });

  it('appends two touch-docked source-bound reactions to the actual latest composer without submitting and opens the older reviewer', async () => {
    vi.spyOn(window, 'matchMedia').mockImplementation((query) => ({
      matches: query === '(any-pointer: coarse)', media: query, onchange: null,
      addListener: vi.fn(), removeListener: vi.fn(), addEventListener: vi.fn(), removeEventListener: vi.fn(), dispatchEvent: vi.fn(),
    }));
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (this: HTMLElement) {
      return this.id === 'messages' ? 800 : 0;
    });
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains('virtual-transcript__row')) return new DOMRect(0, 0, 390, 120);
      if (this.id === 'messages') return new DOMRect(0, 0, 390, 800);
      return originalRect.call(this);
    });
    Object.defineProperty(Range.prototype, 'getBoundingClientRect', {
      configurable: true,
      value: () => ({ left: 20, top: 100, bottom: 130, right: 250, width: 230, height: 30 }),
    });
    const scenario = getProductConversationScenario('inline-message-reactions');
    const { container } = render(<ProductConversationFixture scenario={scenario} />);
    const draft = await screen.findByPlaceholderText('Type a message...');
    await waitFor(() => expect(draft).toHaveValue('Let’s keep the first version focused.'));
    const send = vi.spyOn(api, 'sendMessage');
    for (const [messageId, reaction] of [
      ['reaction-answer-older', 'Preserve this guarantee.'],
      ['reaction-answer', 'Strong idea; test deterministic state patterns.'],
    ]) {
      const paragraph = await waitFor(() => {
        const found = container.querySelector(`[data-inline-reaction-message="${messageId}"] .agent-text-block p`);
        expect(found).not.toBeNull();
        return found!;
      });
      fireEvent.pointerDown(paragraph, { pointerType: 'touch' });
      const range = document.createRange();
      range.selectNodeContents(paragraph);
      window.getSelection()!.removeAllRanges();
      window.getSelection()!.addRange(range);
      fireEvent(document, new Event('selectionchange'));
      const dock = await screen.findByRole('region', { name: 'Docked reaction' });
      const reactionInput = screen.getByRole('textbox', { name: 'Your reaction' });
      expect(dock).toBeInTheDocument();
      expect(reactionInput).not.toHaveFocus();
      fireEvent.change(reactionInput, { target: { value: reaction } });
      fireEvent.click(screen.getByRole('button', { name: 'Add to draft' }));
    }
    expect((draft as HTMLTextAreaElement).value).toContain('Let’s keep the first version focused.\n\nRegarding message #2 (reaction-history:reaction-answer-older):');
    expect((draft as HTMLTextAreaElement).value).toContain('Regarding message #2 (reaction-work:reaction-answer):');
    expect(send).not.toHaveBeenCalled();
    fireEvent.click(screen.getAllByRole('button', { name: 'Open message reviewer' })[0]!);
    const viewer = await screen.findByRole('dialog', { name: 'Message viewer: Agent message #2' });
    expect(viewer).toHaveTextContent('Preserve the user’s draft and every completed tool result.');
    expect(screen.getByRole('button', { name: 'Add note to line 1' })).toBeInTheDocument();
    send.mockRestore();
    window.getSelection()?.removeAllRanges();
  });

  it('returns a retained reaction through the production older-history loader after the source cache is lost', async () => {
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function (this: HTMLElement) {
      return this.id === 'messages' ? 800 : 0;
    });
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return new DOMRect(0, 0, 390, this.id === 'messages' ? 800 : 120);
    });
    Object.defineProperty(Range.prototype, 'getBoundingClientRect', {
      configurable: true,
      value: () => new DOMRect(20, 100, 230, 30),
    });
    const originalSnapshot = InlineReactionStore.prototype.getSnapshot;
    const observedStores = new Set<InlineReactionStore>();
    vi.spyOn(InlineReactionStore.prototype, 'getSnapshot').mockImplementation(function (this: InlineReactionStore, key) {
      observedStores.add(this);
      return originalSnapshot.call(this, key);
    });
    const base = getProductConversationScenario('inline-message-reactions');
    const snapshot = base.snapshot!;
    const scenario = {
      ...base,
      snapshot: { ...snapshot, segments: snapshot.segments.slice(1), has_older: true, before: 'reaction-page' },
      olderSnapshot: { ...snapshot, segments: snapshot.segments.slice(0, 1), has_older: false, before: null },
    };
    const { container } = render(<ProductConversationFixture scenario={scenario} />);
    await screen.findByPlaceholderText('Type a message...');
    await waitFor(() => expect(observedStores.size).toBe(1));
    const reactionStore = [...observedStores][0]!;
    expect(container.querySelector('[data-inline-reaction-message="reaction-answer-older"]')).toBeNull();
    const quote = 'Preserve the user’s draft';
    act(() => {
      reactionStore.dispatch(snapshot.product_conversation_id, { type: 'select', presentation: 'floating', source: {
        messageId: 'reaction-answer-older', sequenceId: 2,
        occurrenceToken: 'reaction-history:reaction-answer-older', quote,
        textAnchor: {
          start: { fragmentId: 'agent-text-0', offset: 0 },
          end: { fragmentId: 'agent-text-0', offset: quote.length },
        },
      } });
      reactionStore.dispatch(snapshot.product_conversation_id, { type: 'edit', body: 'Keep this guarantee.' });
    });
    const fetchSnapshot = vi.spyOn(api, 'getProductConversationHistorySnapshot');
    const returnButton = await screen.findByRole('button', { name: /Return to passage/ });
    await act(async () => { fireEvent.click(returnButton); });
    await waitFor(() => expect(container.querySelector('[data-inline-reaction-message="reaction-answer-older"]')).not.toBeNull());
    await act(async () => { await new Promise<void>(resolve => requestAnimationFrame(() => resolve())); });
    const input = await screen.findByRole('textbox', { name: 'Your reaction' });
    expect(input).toHaveValue('Keep this guarantee.');
    expect(input).not.toHaveFocus();
    expect(fetchSnapshot).toHaveBeenCalledWith('fixture-product-conversation', expect.objectContaining({ before: 'reaction-page' }));
    await waitFor(() => expect(window.getSelection()?.toString()).toBe(quote));
    expect(container.querySelector('[data-inline-reaction-message="reaction-answer-older"]')).not.toBeNull();
    window.getSelection()?.removeAllRanges();
  });

  it('makes the active transcript the bounded flex owner instead of inheriting .view.active block layout', () => {
    const activeTranscriptRule = productConversationCss.match(/\.product-conversation-page__transcript\.view\.active\s*{([^}]*)}/s)?.[1];

    expect(activeTranscriptRule).toMatch(/display:\s*flex/);
    expect(activeTranscriptRule).toMatch(/flex-direction:\s*column/);
    expect(activeTranscriptRule).toMatch(/min-height:\s*0/);
    expect(activeTranscriptRule).toMatch(/overflow:\s*hidden/);
  });

  it('renders the real aggregate transcript and latest-row ordinary composer runtime', async () => {
    const scenario = getProductConversationScenario('desktop-multi-segment-qa-work');
    const { container } = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(screen.getByTestId('product-conversation-page')).toBeInTheDocument();
    expect(container.querySelectorAll('#chat-view')).toHaveLength(1);
    expect(container.querySelector('[data-testid="product-conversation-transcript"]')).not.toBeNull();
    expect(container.querySelectorAll('#app')).toHaveLength(0);
    expect(container.querySelector('.embedded-conversation-shell')).not.toBeNull();
    expect(container.querySelector('[data-testid="product-conversation-composer"]')).not.toBeNull();
    expect(screen.getByRole('heading', { name: 'Product Alpha' })).toBeInTheDocument();
    expect(screen.getByTestId('product-conversation-source')).toHaveTextContent('Approved task from source conversation');
    expect(screen.getByTestId('product-conversation-work')).not.toHaveAttribute('open');
    expect(screen.getByRole('button', { name: 'Recall' })).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('dialog', { name: 'Recall' })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Recall' }));
    expect(await screen.findByText('Which invariants carried across the whole conversation?')).toBeInTheDocument();
    expect(screen.getByTestId('product-conversation-composer')).toBeInTheDocument();

    expect(container).not.toHaveTextContent('Presentation');
    expect(container).not.toHaveTextContent('Q&A history');
    expect(container).not.toHaveTextContent('Aggregate diagnostics');
  });

  it.each([
    ['mobile-context-exhausted', 'Context Window Full'],
    ['awaiting-continuation', 'Compacting conversation...'],
  ] as const)('mounts latest-row continuation presentation for %s', async (scenarioId, expectedText) => {
    const scenario = getProductConversationScenario(scenarioId);
    const { container } = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(screen.getByText(expectedText)).toBeInTheDocument();
    if (scenarioId === 'mobile-context-exhausted') {
      expect(screen.getByRole('button', { name: 'Continue' })).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Edit first' })).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Copy handoff' })).toBeInTheDocument();
    }
  });

  it('restores mutable API hooks after unmount', async () => {
    const scenario = getProductConversationScenario('mobile-open');
    const originalGetSnapshot = api.getProductConversationHistorySnapshot;
    const originalGetPrStatus = api.getPrStatus;
    const originalGetChain = api.getChain;
    const originalSubmitChainQuestion = api.submitChainQuestion;
    const originalGetRoute = api.getConversationRoute;
    const originalGetRouteBySlug = api.getConversationRouteBySlug;
    const originalGetConversation = api.getConversation;
    const { container, unmount } = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(api.getProductConversationHistorySnapshot).not.toBe(originalGetSnapshot);
    expect(api.getPrStatus).not.toBe(originalGetPrStatus);
    expect(api.getChain).not.toBe(originalGetChain);
    expect(api.submitChainQuestion).not.toBe(originalSubmitChainQuestion);
    expect(api.getConversationRoute).not.toBe(originalGetRoute);
    expect(api.getConversationRouteBySlug).not.toBe(originalGetRouteBySlug);
    expect(api.getConversation).not.toBe(originalGetConversation);

    unmount();

    expect(api.getProductConversationHistorySnapshot).toBe(originalGetSnapshot);
    expect(api.getPrStatus).toBe(originalGetPrStatus);
    expect(api.getChain).toBe(originalGetChain);
    expect(api.submitChainQuestion).toBe(originalSubmitChainQuestion);
    expect(api.getConversationRoute).toBe(originalGetRoute);
    expect(api.getConversationRouteBySlug).toBe(originalGetRouteBySlug);
    expect(api.getConversation).toBe(originalGetConversation);
  });
});
