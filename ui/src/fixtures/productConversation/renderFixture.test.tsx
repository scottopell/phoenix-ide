import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { api, streamApi } from '../../api';
import { ProductConversationFixture } from './renderFixture';
import { getProductConversationScenario } from './scenarios';

afterEach(() => cleanup());

describe('ProductConversationFixture', () => {
  it('renders the real desktop page with metadata, Q&A history, and the read-only composer placeholder', async () => {
    const scenario = getProductConversationScenario('desktop-multi-segment-qa-work');
    const { container } = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(screen.getByTestId('product-conversation-page')).toBeInTheDocument();
    expect(screen.getAllByText('Product Alpha').length).toBeGreaterThan(0);
    expect(screen.getByText('Presentation')).toBeInTheDocument();
    expect(screen.getByText('History is read-only.')).toBeInTheDocument();
    expect(screen.getByText('What user-visible surfaces must remain stable?')).toBeInTheDocument();
    expect(screen.getByText(/The route, title, lineage metadata/)).toBeInTheDocument();
  });

  it('restores mutable API hooks after unmount', async () => {
    const scenario = getProductConversationScenario('mobile-open');
    const originalGetSnapshot = api.getProductConversationSnapshot;
    const originalGetChain = api.getChain;
    const originalSubmit = api.submitChainQuestion;
    const originalSubscribe = streamApi.subscribeToChainStream;
    const { container, unmount } = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(api.getProductConversationSnapshot).not.toBe(originalGetSnapshot);
    expect(streamApi.subscribeToChainStream).not.toBe(originalSubscribe);

    unmount();

    expect(api.getProductConversationSnapshot).toBe(originalGetSnapshot);
    expect(api.getChain).toBe(originalGetChain);
    expect(api.submitChainQuestion).toBe(originalSubmit);
    expect(streamApi.subscribeToChainStream).toBe(originalSubscribe);
  });
});
