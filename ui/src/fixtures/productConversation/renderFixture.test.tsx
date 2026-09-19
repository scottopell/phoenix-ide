import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api } from '../../api';
import { InlineReactionStore } from '../../conversation/InlineReactionStore';
import { ProductConversationFixture } from './renderFixture';
import { getProductConversationScenario } from './scenarios';

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
  it('appends two source-bound reactions to the actual latest composer without submitting and opens the older reviewer', async () => {
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
      const range = document.createRange();
      range.selectNodeContents(paragraph);
      window.getSelection()!.removeAllRanges();
      window.getSelection()!.addRange(range);
      fireEvent(document, new Event('selectionchange'));
      const reactionInput = await screen.findByRole('textbox', { name: 'Your reaction' });
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
      reactionStore.dispatch(snapshot.product_conversation_id, { type: 'select', source: {
        messageId: 'reaction-answer-older', sequenceId: 2,
        occurrenceToken: 'reaction-history:reaction-answer-older', quote,
        textAnchor: {
          start: { fragmentId: 'agent-text-0', offset: 0 },
          end: { fragmentId: 'agent-text-0', offset: quote.length },
        },
      } });
      reactionStore.dispatch(snapshot.product_conversation_id, { type: 'edit', body: 'Keep this guarantee.' });
    });
    const fetchSnapshot = vi.spyOn(api, 'getProductConversationSnapshot');
    fireEvent.click(await screen.findByRole('button', { name: /Return to passage/ }));
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
    const originalGetSnapshot = api.getProductConversationSnapshot;
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

    expect(api.getProductConversationSnapshot).not.toBe(originalGetSnapshot);
    expect(api.getPrStatus).not.toBe(originalGetPrStatus);
    expect(api.getChain).not.toBe(originalGetChain);
    expect(api.submitChainQuestion).not.toBe(originalSubmitChainQuestion);
    expect(api.getConversationRoute).not.toBe(originalGetRoute);
    expect(api.getConversationRouteBySlug).not.toBe(originalGetRouteBySlug);
    expect(api.getConversation).not.toBe(originalGetConversation);

    unmount();

    expect(api.getProductConversationSnapshot).toBe(originalGetSnapshot);
    expect(api.getPrStatus).toBe(originalGetPrStatus);
    expect(api.getChain).toBe(originalGetChain);
    expect(api.submitChainQuestion).toBe(originalSubmitChainQuestion);
    expect(api.getConversationRoute).toBe(originalGetRoute);
    expect(api.getConversationRouteBySlug).toBe(originalGetRouteBySlug);
    expect(api.getConversation).toBe(originalGetConversation);
  });
});
