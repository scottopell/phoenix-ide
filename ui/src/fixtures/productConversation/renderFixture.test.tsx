import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
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

describe('ProductConversationFixture', () => {
  it('renders the real aggregate transcript and latest-row ordinary composer runtime', async () => {
    const scenario = getProductConversationScenario('desktop-multi-segment-qa-work');
    const { container } = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(screen.getByTestId('product-conversation-page')).toBeInTheDocument();
    expect(container.querySelector('#chat-view')).not.toBeNull();
    expect(container.querySelectorAll('#app')).toHaveLength(0);
    expect(container.querySelector('.embedded-conversation-shell')).not.toBeNull();
    expect(container.querySelector('[data-testid="product-conversation-composer"]')).not.toBeNull();
    expect(container).not.toHaveTextContent('Presentation');
    expect(container).not.toHaveTextContent('Q&A history');
  });

  it('restores mutable API hooks after unmount', async () => {
    const scenario = getProductConversationScenario('mobile-open');
    const originalGetSnapshot = api.getProductConversationSnapshot;
    const originalGetRoute = api.getConversationRoute;
    const originalGetRouteBySlug = api.getConversationRouteBySlug;
    const originalGetConversation = api.getConversation;
    const { container, unmount } = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(api.getProductConversationSnapshot).not.toBe(originalGetSnapshot);
    expect(api.getConversationRoute).not.toBe(originalGetRoute);
    expect(api.getConversationRouteBySlug).not.toBe(originalGetRouteBySlug);
    expect(api.getConversation).not.toBe(originalGetConversation);

    unmount();

    expect(api.getProductConversationSnapshot).toBe(originalGetSnapshot);
    expect(api.getConversationRoute).toBe(originalGetRoute);
    expect(api.getConversationRouteBySlug).toBe(originalGetRouteBySlug);
    expect(api.getConversation).toBe(originalGetConversation);
  });
});
