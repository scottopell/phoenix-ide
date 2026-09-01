import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { api, streamApi } from '../../api';
import { cacheDB } from '../../cache';
import { COORDINATOR_FIXTURE_SURFACE } from '../coordinator';
import { ProductConversationFixture } from './renderFixture';
import {
  FIXTURE_HANDOFF_SUMMARY,
  FIXTURE_SUCCESSOR_FIRST_MESSAGE,
  getProductConversationScenario,
} from './scenarios';

class FixtureEventSource {
  readonly url: string;
  onopen: ((this: EventSource, ev: Event) => unknown) | null = null;
  onmessage: ((this: EventSource, ev: MessageEvent) => unknown) | null = null;
  onerror: ((this: EventSource, ev: Event) => unknown) | null = null;
  constructor(url: string | URL) {
    this.url = String(url);
  }
  addEventListener() {}
  removeEventListener() {}
  close() {}
}

beforeEach(() => {
  vi.spyOn(cacheDB, 'putConversation').mockResolvedValue(undefined);
  Object.assign(globalThis, { EventSource: FixtureEventSource });
});

afterEach(() => {
  cleanup();
  // @ts-expect-error test-only cleanup
  delete globalThis.EventSource;
  vi.restoreAllMocks();
});

function handoffSummaries(scenario: ReturnType<typeof getProductConversationScenario>): string[] {
  return scenario.snapshot?.segments.flatMap((segment) => segment.handoff ? [segment.handoff.summary] : []) ?? [];
}

describe('ProductConversationFixture', () => {
  it('renders the real desktop open page with metadata, Q&A history, the latest-row runtime, and one exact handoff', async () => {
    const scenario = getProductConversationScenario('desktop-open-multi-segment-qa-work');
    const { container } = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    const page = screen.getByTestId('product-conversation-page');
    expect(page).toBeInTheDocument();
    expect(screen.getAllByText('Product Alpha').length).toBeGreaterThan(0);
    expect(screen.getByText('Presentation')).toBeInTheDocument();
    expect(container.querySelector('[data-testid="product-conversation-composer"]')).not.toBeNull();
    expect(screen.getByText('What user-visible surfaces must remain stable?')).toBeInTheDocument();
    expect(screen.getByText(/The route, title, lineage metadata/)).toBeInTheDocument();
    expect(handoffSummaries(scenario)).toEqual([FIXTURE_HANDOFF_SUMMARY]);
    expect(scenario.snapshot?.segments.at(-1)?.messages[0]?.content).toEqual({ text: FIXTURE_SUCCESSOR_FIRST_MESSAGE });
    expect(container.querySelectorAll('a[href*="product-handoff"]').length).toBe(1);
  });

  it.each(['desktop-history-read-only', 'mobile-history-read-only'] as const)(
    'renders %s as read-only with a stable semantic ready marker',
    async (id) => {
      const scenario = getProductConversationScenario(id);
      const view = render(<ProductConversationFixture scenario={scenario} />);

      await waitFor(() => {
        expect(view.container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
      });

      expect(screen.getByTestId('product-conversation-page')).toBeInTheDocument();
      expect(view.container.querySelector(`[data-product-conversation-viewport="${scenario.viewport}"]`)).not.toBeNull();
      expect(scenario.snapshot?.ordinary_lifecycle).toBe('history');
      expect(scenario.snapshot?.writable_transcript_row_id).toBeNull();
      expect(handoffSummaries(scenario)).toEqual([FIXTURE_HANDOFF_SUMMARY]);
      expect(view.container.querySelectorAll('a[href*="product-handoff"]').length).toBe(1);
    },
  );

  it('keeps exactly one handoff marker through rerender and long scrolling', async () => {
    const scenario = getProductConversationScenario('long-history-110-messages');
    const view = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(view.container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(handoffSummaries(scenario)).toEqual([FIXTURE_HANDOFF_SUMMARY]);
    expect(view.container.querySelectorAll('a[href*="product-handoff"]').length).toBe(1);

    view.rerender(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(view.container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });
    expect(handoffSummaries(scenario)).toEqual([FIXTURE_HANDOFF_SUMMARY]);
    expect(view.container.querySelectorAll('a[href*="product-handoff"]').length).toBe(1);
    expect(screen.getAllByText('Long fixture conversation')).toHaveLength(2);
  });

  it('keeps handoff identities aligned with adjacent transcript segments', () => {
    for (const id of ['desktop-open-multi-segment-qa-work', 'desktop-history-read-only', 'mobile-history-read-only', 'long-history-110-messages'] as const) {
      const segments = getProductConversationScenario(id).snapshot?.segments ?? [];
      segments.forEach((segment, index) => {
        if (!segment.handoff) return;
        expect(segment.handoff.predecessor_transcript_row_id).toBe(segments[index - 1]?.transcript_row_id);
        expect(segment.handoff.successor_transcript_row_id).toBe(segment.transcript_row_id);
      });
    }
  });

  it('renders the error scenario through the mocked snapshot failure', async () => {
    const scenario = getProductConversationScenario('error');
    const { container } = render(<ProductConversationFixture scenario={scenario} />);
    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });
    expect(screen.getByRole('alert').textContent).toContain('Fixture failed to fetch product conversation snapshot');
  });

  it('resolves embedded mobile routes to the latest transcript identity', async () => {
    const scenario = getProductConversationScenario('mobile-open');
    render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(document.documentElement.dataset['productConversationFixtureReady']).toBe(scenario.id);
    });

    const bySlug = await api.getConversationRouteBySlug('fixture-mobile');
    const byId = await api.getConversationRoute('row-mobile-1');
    expect(bySlug.id).toBe('row-mobile-1');
    expect(byId.id).toBe('row-mobile-1');
    expect(scenario.snapshot?.latest_transcript_row_id).toBe('row-mobile-1');
  });

  it('keeps the product fixture surface distinct from the coordinator fixture surface and restores mutable API hooks after unmount', async () => {
    const scenario = getProductConversationScenario('mobile-open');
    const originalGetSnapshot = api.getProductConversationSnapshot;
    const originalGetChain = api.getChain;
    const originalSubmit = api.submitChainQuestion;
    const originalSubscribe = streamApi.subscribeToChainStream;
    const { container, unmount } = render(<ProductConversationFixture scenario={scenario} />);

    await waitFor(() => {
      expect(container.querySelector(`[data-product-conversation-fixture-ready="${scenario.id}"]`)).not.toBeNull();
    });

    expect(container.querySelector('[data-product-conversation-surface="product-conversation"]')).not.toBeNull();
    expect(container.querySelector(`[data-product-conversation-surface="${COORDINATOR_FIXTURE_SURFACE}"]`)).toBeNull();
    expect(document.documentElement.dataset['coordinatorFixtureReady']).toBeUndefined();
    expect(api.getProductConversationSnapshot).not.toBe(originalGetSnapshot);
    expect(streamApi.subscribeToChainStream).not.toBe(originalSubscribe);

    unmount();

    expect(api.getProductConversationSnapshot).toBe(originalGetSnapshot);
    expect(api.getChain).toBe(originalGetChain);
    expect(api.submitChainQuestion).toBe(originalSubmit);
    expect(streamApi.subscribeToChainStream).toBe(originalSubscribe);
  });
});
