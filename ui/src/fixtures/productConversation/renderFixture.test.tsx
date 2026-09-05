import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api } from '../../api';
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

afterEach(() => cleanup());

const productConversationCss = readFileSync(`${process.cwd()}/src/pages/ProductConversationPage.css`, 'utf8');

describe('ProductConversationFixture', () => {
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
